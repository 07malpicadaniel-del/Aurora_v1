use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aurora_domain::{SubverticalInmobiliaria, Ubicacion, VerticalComercial};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use aurora_osint::{
    ConsultaObjetivo, DirectorioPublicoAdapter, DynamicBrowserFetcher, EnjambreOsint, FormatoCrudo,
    HttpSwarmClient, OsintError, OsintResult, OsintSource, PoliticaHttp, RegistroCrudo,
    SenalesAbiertasAdapter,
};

async fn spawn_test_server(
    handler: impl Fn(String, usize) -> (u16, &'static str, String) + Send + Sync + 'static,
) -> OsintResult<(String, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| OsintError::Concurrencia(format!("bind local server: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| OsintError::Concurrencia(format!("local_addr: {e}")))?;
    let handler = Arc::new(handler);
    let counter = Arc::new(AtomicUsize::new(0));

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let h = Arc::clone(&handler);
            let idx = counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let (status, ctype, body) = h(path, idx);
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });

    Ok((format!("http://{addr}"), handle))
}

struct MockBrowserFetcher;

#[async_trait]
impl DynamicBrowserFetcher for MockBrowserFetcher {
    async fn fetch_rendered_html(&self, url: &str) -> OsintResult<String> {
        Ok(format!(
            r#"<html><body>
            <article class="senal-abierta">
              <h2>Expansión logística portuaria en 2026</h2>
              <span class="nombre-perfil">Roberto Aguilar Paredes</span>
              <span class="empresa">Terminal Marítima del Sureste</span>
              <a class="fuente-link" href="{url}">Evidencia JS</a>
              <p class="cuerpo">Nueva bodega aduanal e inversión inmobiliaria en marcha.</p>
            </article>
            </body></html>"#
        ))
    }
}

struct FuenteConcurrenciaMedida {
    nombre: String,
    activos: Arc<AtomicUsize>,
    pico: Arc<AtomicUsize>,
}

#[async_trait]
impl OsintSource for FuenteConcurrenciaMedida {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    async fn recolectar(&self, _consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let actual = self.activos.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.pico.fetch_max(actual, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(35)).await;
        self.activos.fetch_sub(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn backoff_exponencial_recupera_de_http_429_y_503() -> OsintResult<()> {
    let fixture_json = r#"[{
        "razon_social": "Logistica del Golfo SA",
        "representante": "Ing. Carlos Mendoza",
        "socios": ["Ing. Carlos Mendoza", "Lic. Sofia Navarro"],
        "giro": "Transporte y bodega aduanal",
        "ciudad": "Veracruz",
        "estado": "Veracruz",
        "correo": "cmendoza@logisticagolfo.mx",
        "descripcion": "Inaugura nueva bodega en zona portuaria"
    }]"#;

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_srv = Arc::clone(&attempts);
    let body_ok = fixture_json.to_string();

    let (base_url, handle) = spawn_test_server(move |path, _| {
        if path == "/directorio" {
            let n = attempts_srv.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return (429, "text/plain", "rate limit".to_string());
            }
            if n == 1 {
                return (503, "text/plain", "busy".to_string());
            }
            return (200, "application/json", body_ok.clone());
        }
        (429, "text/plain", "always limited".to_string())
    })
    .await?;

    let politica = PoliticaHttp {
        max_concurrencia_por_dominio: 2,
        max_reintentos: 3,
        backoff_base_ms: 15,
        timeout: Duration::from_secs(5),
    };
    let http = HttpSwarmClient::new(politica)?;

    let adapter_ok = DirectorioPublicoAdapter::desde_endpoint(
        "DENUE Mock",
        format!("{base_url}/directorio"),
        FormatoCrudo::Json,
        http.clone(),
    );

    let consulta = ConsultaObjetivo {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        ubicacion: Ubicacion {
            ciudad: "Veracruz".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Puerto".to_string()),
        },
        palabras_clave: vec!["bodega".to_string()],
        limite_resultados: 10,
        requiere_js: false,
    };

    let registros = adapter_ok.recolectar(&consulta).await?;
    assert_eq!(registros.len(), 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);

    // Verifica que agotando reintentos retorne RateLimitExcedido
    let adapter_fail = DirectorioPublicoAdapter::desde_endpoint(
        "Fuente Limitada",
        format!("{base_url}/siempre-429"),
        FormatoCrudo::Json,
        http,
    );
    let err = adapter_fail.recolectar(&consulta).await;
    assert!(matches!(
        err,
        Err(OsintError::RateLimitExcedido {
            intentos: 4,
            ultimo_codigo: 429,
            ..
        })
    ));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn enjambre_respeta_semaforo_de_concurrencia_y_browser_fetcher() -> OsintResult<()> {
    let activos = Arc::new(AtomicUsize::new(0));
    let pico = Arc::new(AtomicUsize::new(0));

    let mut enjambre = EnjambreOsint::new(2);
    for idx in 0..6 {
        enjambre.registrar_fuente(Arc::new(FuenteConcurrenciaMedida {
            nombre: format!("fuente_{idx}"),
            activos: Arc::clone(&activos),
            pico: Arc::clone(&pico),
        }));
    }

    let consulta = ConsultaObjetivo {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa),
        ubicacion: Ubicacion {
            ciudad: "Queretaro".to_string(),
            estado: "Queretaro".to_string(),
            zona: None,
        },
        palabras_clave: vec!["preventa".to_string()],
        limite_resultados: 10,
        requiere_js: true,
    };

    let _ = enjambre.ejecutar(&consulta).await?;
    assert!(pico.load(Ordering::SeqCst) <= 2);
    assert!(pico.load(Ordering::SeqCst) >= 1);

    let http = HttpSwarmClient::new(PoliticaHttp::default())?;
    let senales = SenalesAbiertasAdapter::desde_endpoint(
        "Portal JS",
        "https://ejemplo-publico.mx/notas",
        FormatoCrudo::Html,
        http,
    )
    .con_browser_fetcher(Arc::new(MockBrowserFetcher));

    let js_regs = senales.recolectar(&consulta).await?;
    assert_eq!(js_regs.len(), 1);
    assert_eq!(
        js_regs[0].nombre_detectado.as_deref(),
        Some("Roberto Aguilar Paredes")
    );

    Ok(())
}
