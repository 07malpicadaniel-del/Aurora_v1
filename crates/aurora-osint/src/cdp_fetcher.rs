use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, warn};

use crate::error::{OsintError, OsintResult};
use crate::sources::DynamicBrowserFetcher;

/// Configuración del adaptador Chrome DevTools Protocol (CDP) para extracción SPA en Arch Linux.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCdpFetcher {
    /// Endpoint HTTP base de depuración remota de Chromium/Chrome (ej. `http://127.0.0.1:9222`).
    pub cdp_http_url: String,
    /// Tiempo máximo por operación de navegación e hidratación DOM.
    pub timeout_ms: u64,
    /// Espera breve tras `Page.navigate` para hidratación JS antes de leer `outerHTML`.
    pub esperar_hidratacion_ms: u64,
}

impl Default for ConfigCdpFetcher {
    fn default() -> Self {
        Self {
            cdp_http_url: "http://127.0.0.1:9222".to_string(),
            timeout_ms: 15_000,
            esperar_hidratacion_ms: 800,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTargetInfo {
    id: Option<String>,
    #[serde(rename = "type")]
    target_type: Option<String>,
    web_socket_debugger_url: Option<String>,
}

/// Cliente Chrome DevTools Protocol real (`http/json` + `WebSocket`) compatible con
/// Chromium/Chrome local y `bob_engine/router/src/browser_cdp.rs`.
#[derive(Debug, Clone)]
pub struct HttpCdpBrowserFetcher {
    config: ConfigCdpFetcher,
    http: Client,
}

impl HttpCdpBrowserFetcher {
    /// Inicializa el cliente CDP validando URL HTTP base y construyendo pool `rustls-tls`.
    pub fn new(config: ConfigCdpFetcher) -> OsintResult<Self> {
        let base = config.cdp_http_url.trim();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(OsintError::ConfiguracionHttp(
                "cdp_http_url debe iniciar con http:// o https://".to_string(),
            ));
        }

        let http = Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_millis(config.timeout_ms.max(1_000)))
            .build()
            .map_err(|e| OsintError::ConfiguracionHttp(e.to_string()))?;

        Ok(Self { config, http })
    }

    /// Devuelve referencia a la configuración activa.
    #[must_use]
    pub const fn config(&self) -> &ConfigCdpFetcher {
        &self.config
    }

    /// Verifica si el endpoint CDP local responde `/json/version` o `/json`.
    pub async fn probe_disponible(&self) -> bool {
        let base = self.config.cdp_http_url.trim_end_matches('/');
        for suffix in ["/json/version", "/json"] {
            let url = format!("{base}{suffix}");
            if let Ok(resp) = self
                .http
                .get(&url)
                .timeout(Duration::from_millis(600))
                .send()
                .await
            {
                if resp.status().is_success() {
                    return true;
                }
            }
        }
        false
    }

    async fn abrir_o_buscar_pestana(&self, target_url: &str) -> OsintResult<(String, Option<String>)> {
        let base = self.config.cdp_http_url.trim_end_matches('/');
        let new_tab_endpoint = format!("{base}/json/new?{target_url}");

        if let Ok(resp) = self.http.put(&new_tab_endpoint).send().await {
            if resp.status().is_success() {
                if let Ok(info) = resp.json::<CdpTargetInfo>().await {
                    if let Some(ws_url) = info.web_socket_debugger_url {
                        return Ok((ws_url, info.id));
                    }
                }
            }
        }

        let list_url = format!("{base}/json");
        let resp = self
            .http
            .get(&list_url)
            .send()
            .await
            .map_err(|e| OsintError::HttpRequest {
                url: list_url.clone(),
                source: e,
            })?;

        if !resp.status().is_success() {
            return Err(OsintError::HttpStatusInesperado {
                url: list_url,
                codigo: resp.status().as_u16(),
            });
        }

        let body = resp.text().await.map_err(|e| OsintError::HttpRequest {
            url: list_url.clone(),
            source: e,
        })?;
        let tabs: Vec<CdpTargetInfo> =
            serde_json::from_str(&body).map_err(|e| OsintError::ParseoJson {
                fuente: "CDP /json".to_string(),
                source: e,
            })?;

        let ws_url = tabs
            .into_iter()
            .find(|t| {
                t.target_type.as_deref().unwrap_or("page") == "page"
                    && t.web_socket_debugger_url.is_some()
            })
            .and_then(|t| t.web_socket_debugger_url)
            .ok_or_else(|| {
                OsintError::ConfiguracionHttp(
                    "ninguna pestaña 'page' con webSocketDebuggerUrl disponible en endpoint CDP"
                        .to_string(),
                )
            })?;

        Ok((ws_url, None))
    }

    async fn cerrar_pestana_si_propia(&self, tab_id: Option<&str>) {
        let Some(id) = tab_id else {
            return;
        };
        let base = self.config.cdp_http_url.trim_end_matches('/');
        let close_url = format!("{base}/json/close/{id}");
        let _ = self.http.get(&close_url).send().await;
    }

    async fn navegar_y_extraer_html(&self, ws_url: &str, url: &str) -> OsintResult<String> {
        let (mut ws_stream, _) = connect_async(ws_url).await.map_err(|e| {
            OsintError::ConfiguracionHttp(format!("fallo conectando WebSocket CDP '{ws_url}': {e}"))
        })?;

        let page_enable = json!({
            "id": 1,
            "method": "Page.enable"
        })
        .to_string();
        ws_stream
            .send(WsMessage::Text(page_enable.into()))
            .await
            .map_err(|e| {
                OsintError::ConfiguracionHttp(format!("error enviando Page.enable por CDP: {e}"))
            })?;

        let navigate = json!({
            "id": 2,
            "method": "Page.navigate",
            "params": { "url": url }
        })
        .to_string();
        ws_stream
            .send(WsMessage::Text(navigate.into()))
            .await
            .map_err(|e| {
                OsintError::ConfiguracionHttp(format!("error enviando Page.navigate por CDP: {e}"))
            })?;

        esperar_respuesta_cdp(&mut ws_stream, 2).await?;

        if self.config.esperar_hidratacion_ms > 0 {
            sleep(Duration::from_millis(self.config.esperar_hidratacion_ms)).await;
        }

        let eval = json!({
            "id": 3,
            "method": "Runtime.evaluate",
            "params": {
                "expression": "document.documentElement ? document.documentElement.outerHTML : (document.body ? document.body.innerHTML : '')",
                "returnByValue": true,
                "awaitPromise": true
            }
        })
        .to_string();
        ws_stream
            .send(WsMessage::Text(eval.into()))
            .await
            .map_err(|e| {
                OsintError::ConfiguracionHttp(format!(
                    "error enviando Runtime.evaluate por CDP: {e}"
                ))
            })?;

        let eval_reply = esperar_respuesta_cdp(&mut ws_stream, 3).await?;
        let _ = ws_stream.close(None).await;

        let html = eval_reply
            .pointer("/result/result/value")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OsintError::ParseoHtml {
                    fuente: "CDP Runtime.evaluate".to_string(),
                    detalle: format!("respuesta CDP sin cadena HTML en value: {eval_reply}"),
                }
            })?;

        Ok(html.to_string())
    }
}

async fn esperar_respuesta_cdp<S>(stream: &mut S, target_id: u64) -> OsintResult<Value>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(frame) = stream.next().await {
        let msg = frame.map_err(|e| {
            OsintError::ConfiguracionHttp(format!("error leyendo trama WebSocket CDP: {e}"))
        })?;

        if let WsMessage::Text(texto) = msg {
            if let Ok(val) = serde_json::from_str::<Value>(texto.as_ref()) {
                if val.get("id").and_then(Value::as_u64) == Some(target_id) {
                    if let Some(err_obj) = val.get("error") {
                        return Err(OsintError::ConfiguracionHttp(format!(
                            "error remoto CDP (id={target_id}): {err_obj}"
                        )));
                    }
                    return Ok(val);
                }
            }
        }
    }

    Err(OsintError::ConfiguracionHttp(format!(
        "conexión WebSocket CDP cerrada antes de responder id={target_id}"
    )))
}

#[async_trait]
impl DynamicBrowserFetcher for HttpCdpBrowserFetcher {
    #[tracing::instrument(skip(self), fields(url = %url, cdp = %self.config.cdp_http_url))]
    async fn fetch_rendered_html(&self, url: &str) -> OsintResult<String> {
        let limpio = url.trim();
        if !(limpio.starts_with("http://") || limpio.starts_with("https://")) {
            return Err(OsintError::ConfiguracionHttp(
                "URL objetivo para CDP debe iniciar con http:// o https://".to_string(),
            ));
        }

        let timeout = Duration::from_millis(self.config.timeout_ms.max(1_000));
        let (ws_url, created_id) = tokio::time::timeout(timeout, self.abrir_o_buscar_pestana(limpio))
            .await
            .map_err(|_| {
                OsintError::ConfiguracionHttp("timeout abriendo pestaña en endpoint CDP".to_string())
            })??;

        let resultado =
            tokio::time::timeout(timeout, self.navegar_y_extraer_html(&ws_url, limpio)).await;

        self.cerrar_pestana_si_propia(created_id.as_deref()).await;

        match resultado {
            Ok(Ok(html)) => {
                debug!(bytes = html.len(), "HTML hidratado recuperado vía CDP");
                Ok(html)
            }
            Ok(Err(err)) => Err(err),
            Err(_) => {
                warn!(url = %limpio, "Timeout ejecutando hidratación DOM en CDP");
                Err(OsintError::ConfiguracionHttp(
                    "timeout esperando Runtime.evaluate en CDP".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[tokio::test]
    async fn cdp_fetcher_extrae_dom_renderizado_sobre_websocket_local(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let ws_listener = TcpListener::bind("127.0.0.1:0").await?;
        let ws_addr = ws_listener.local_addr()?;

        let http_listener = TcpListener::bind("127.0.0.1:0").await?;
        let http_addr = http_listener.local_addr()?;

        let ws_handle = tokio::spawn(async move {
            if let Ok((stream, _)) = ws_listener.accept().await {
                if let Ok(mut ws) = accept_async(stream).await {
                    while let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
                        if let Ok(cmd) = serde_json::from_str::<Value>(txt.as_ref()) {
                            let id = cmd["id"].as_u64().unwrap_or(0);
                            let reply = match cmd["method"].as_str() {
                                Some("Runtime.evaluate") => json!({
                                    "id": id,
                                    "result": {
                                        "result": {
                                            "type": "string",
                                            "value": "<html><body><article class=\"nota-corporativa\"><h1>Expansión Monterrey</h1></article></body></html>"
                                        }
                                    }
                                }),
                                _ => json!({ "id": id, "result": {} }),
                            };
                            let _ = ws.send(WsMessage::Text(reply.to_string().into())).await;
                            if cmd["method"].as_str() == Some("Runtime.evaluate") {
                                break;
                            }
                        }
                    }
                }
            }
        });

        let http_handle = tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = http_listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.starts_with("PUT /json/new") {
                    json!({
                        "id": "tab-aurora-1",
                        "type": "page",
                        "webSocketDebuggerUrl": format!("ws://{ws_addr}/devtools/page/tab-aurora-1")
                    })
                    .to_string()
                } else {
                    "{\"ok\":true}".to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });

        let fetcher = HttpCdpBrowserFetcher::new(ConfigCdpFetcher {
            cdp_http_url: format!("http://{http_addr}"),
            timeout_ms: 5_000,
            esperar_hidratacion_ms: 10,
        })?;

        let html = fetcher
            .fetch_rendered_html("https://datos.ejemplo.mx/spa")
            .await?;
        assert!(html.contains("Expansión Monterrey"));

        let _ = tokio::join!(ws_handle, http_handle);
        Ok(())
    }
}
