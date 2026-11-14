use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use aurora_cortex::CanalContacto;
use aurora_daemon::{
    crear_router, AuroraDaemonConfig, AuroraDaemonState, HealthResponse, HuntRequest, ModoEjecucion,
    OptOutRequest, ReporteSubsistemas, SubsystemProbe, AprobarOrdenRequest,
};
use aurora_domain::{EstadoAprobacion, SubverticalInmobiliaria, Ubicacion, VerticalComercial};
use aurora_osint::{CoMencionCruda, FormatoCrudo, RegistroCrudo};
use aurora_pulse::ConfigSmtp;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use tokio::net::TcpListener;
use uuid::Uuid;

fn crear_temp_dir(prefijo: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

async fn levantar_servidor_daemon(
    state: std::sync::Arc<AuroraDaemonState>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let router = crear_router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((addr, handle))
}

#[tokio::test]
async fn test_config_toml_env_y_probe_en_health_http() -> Result<(), Box<dyn std::error::Error>> {
    let dir = crear_temp_dir("aurora_prod_cfg")?;
    let data_dir = dir.join("datos");
    let toml_path = dir.join("aurora.toml");

    fs::write(
        &toml_path,
        format!(
            r#"
bind_addr = "127.0.0.1:8091"
bob_url = "http://127.0.0.1:8085"
bob_slot_id = 0
data_dir = "{}"
modo_ejecucion = "real_con_fallback"
usar_nvenc = true

[smtp]
habilitado = true
host = "127.0.0.1"
puerto = 1025
remitente_correo = "comercial@aurora-mx.local"
usar_tls_starttls = false
"#,
            data_dir.display()
        ),
    )?;

    // Levantar mocks ligeros para BOB /health y CDP /json/version en paralelo
    let bob_listener = TcpListener::bind("127.0.0.1:0").await?;
    let bob_addr = bob_listener.local_addr()?;
    let bob_router = Router::new().route("/health", get(|| async { Json(json!({ "status": "ok" })) }));
    let bob_handle = tokio::spawn(async move {
        let _ = axum::serve(bob_listener, bob_router).await;
    });

    let cdp_listener = TcpListener::bind("127.0.0.1:0").await?;
    let cdp_addr = cdp_listener.local_addr()?;
    let cdp_router = Router::new().route(
        "/json/version",
        get(|| async { Json(json!({ "Browser": "Chromium/Headless" })) }),
    );
    let cdp_handle = tokio::spawn(async move {
        let _ = axum::serve(cdp_listener, cdp_router).await;
    });

    let env_map = HashMap::from([
        ("AURORA_BOB_URL", format!("http://{bob_addr}")),
        ("AURORA_CDP_URL", format!("http://{cdp_addr}")),
        ("AURORA_SMTP_PUERTO", "2525".to_string()),
    ]);

    let cfg = AuroraDaemonConfig::cargar_con_lector_entorno(Some(&toml_path), |k| {
        env_map.get(k).cloned()
    })?;
    assert_eq!(cfg.bob_slot_id, 0);
    assert_eq!(cfg.modo_efectivo(), ModoEjecucion::RealConFallback);
    assert_eq!(cfg.smtp.puerto, 2525);

    // Verificar default XDG ~/.local/share/aurora cuando HOME existe
    let def_cfg = AuroraDaemonConfig::default();
    assert!(
        def_cfg.data_dir.to_string_lossy().contains("aurora"),
        "data_dir por defecto debe resolver ruta XDG de Aurora"
    );

    // Sonda directa contra BOB y CDP en vivo
    let http_probe = Client::new();
    let reporte_vivo = SubsystemProbe::ejecutar(&cfg, &http_probe).await;
    assert!(reporte_vivo.bob_llm.modo_real_activo);
    assert!(reporte_vivo.cdp_browser.modo_real_activo);
    assert!(reporte_vivo.smtp_transport.modo_real_activo);

    // Ahora levantamos servidor axum completo con cfg en SimuladoOffline para validar /health y /health/subsistemas
    let cfg_srv = AuroraDaemonConfig {
        bind_addr: "127.0.0.1:0".to_string(),
        bob_url: format!("http://{bob_addr}"),
        cdp_url: format!("http://{cdp_addr}"),
        data_dir: dir.join("srv"),
        modo_offline: true,
        modo_ejecucion: ModoEjecucion::SimuladoOffline,
        smtp: cfg.smtp.clone(),
        ..AuroraDaemonConfig::default()
    };
    let state = AuroraDaemonState::inicializar_local(cfg_srv)?;
    let (addr, srv_handle) = levantar_servidor_daemon(state).await?;

    let health_resp: HealthResponse = http_probe
        .get(format!("http://{addr}/api/v1/health"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(health_resp.status, "ok");
    assert_eq!(health_resp.modo_ejecucion, ModoEjecucion::SimuladoOffline);
    assert!(health_resp.subsistemas.bob_llm.disponible);
    assert!(health_resp.subsistemas.piper_tts.disponible);
    assert!(health_resp.subsistemas.chromium_headless.disponible);

    let sub_resp: ReporteSubsistemas = http_probe
        .get(format!("http://{addr}/api/v1/health/subsistemas"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(sub_resp.ffmpeg_nvenc.nombre, "FFmpeg + NVENC");

    srv_handle.abort();
    bob_handle.abort();
    cdp_handle.abort();
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn test_apagado_ordenado_compacta_wal_y_restaura_estado_tras_reinicio(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = crear_temp_dir("aurora_wal_restart")?;
    let cfg = AuroraDaemonConfig {
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: dir.clone(),
        modo_offline: true,
        modo_ejecucion: ModoEjecucion::SimuladoOffline,
        ..AuroraDaemonConfig::default()
    };

    let ahora = Utc::now();
    let ubicacion = Ubicacion {
        ciudad: "Guadalajara".to_string(),
        estado: "Jalisco".to_string(),
        zona: Some("Puerta de Hierro".to_string()),
    };

    let hunt_req = HuntRequest {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa),
        ciudad: "Guadalajara".to_string(),
        estado: "Jalisco".to_string(),
        zona: Some("Puerta de Hierro".to_string()),
        palabras_clave: vec!["preventa".to_string(), "plusvalía".to_string()],
        limite_resultados: Some(10),
        requiere_js: Some(false),
        fuente_url: None,
        fuente_formato: None,
        registros_semilla: Some(vec![
            RegistroCrudo {
                fuente_origen: "Registro Propiedad JAL".to_string(),
                url_evidencia: "https://datos.ejemplo.mx/jal/101".to_string(),
                capturado_en: ahora,
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Fernando Ibarra Montes adquiere reserva territorial para torre residencial en Puerta de Hierro Guadalajara".to_string(),
                nombre_detectado: Some("Fernando Ibarra Montes".to_string()),
                empresa_detectada: Some("Grupo Altos Occidente".to_string()),
                ubicacion_detectada: Some(ubicacion.clone()),
                co_mencionados: vec![CoMencionCruda {
                    nombre: "Sofía Ochoa Valdés".to_string(),
                    contexto: "Socia arquitectónica".to_string(),
                    empresa: Some("Grupo Altos Occidente".to_string()),
                }],
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            },
            RegistroCrudo {
                fuente_origen: "Boletín Corporativo GDL".to_string(),
                url_evidencia: "https://datos.ejemplo.mx/jal/102".to_string(),
                capturado_en: ahora,
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Sofía Ochoa Valdés encabeza preventa de lujo en Guadalajara Jalisco".to_string(),
                nombre_detectado: Some("Sofía Ochoa Valdés".to_string()),
                empresa_detectada: Some("Grupo Altos Occidente".to_string()),
                ubicacion_detectada: Some(ubicacion),
                co_mencionados: Vec::new(),
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            },
        ]),
    };

    let (orden_id_inicial, prospecto_inicial) = {
        let state_1 = AuroraDaemonState::inicializar_local(cfg.clone())?;
        let resp = state_1.ejecutar_hunt(hunt_req).await?;
        assert_eq!(resp.prospectos_ingestados, 2);
        assert_eq!(resp.planes_encolados.len(), 2);
        let primera = resp
            .planes_encolados
            .first()
            .ok_or("debió generarse al menos una orden Gatekeeper")?;
        let orden_id = primera.orden_id;
        let prospecto_id = primera.prospecto_id;

        state_1.apagar_ordenadamente().await?;
        (orden_id, prospecto_id)
    };

    // Reiniciar proceso Aurora sobre el mismo directorio WAL + JSON persistente
    let state_2 = AuroraDaemonState::inicializar_local(cfg)?;
    let health_2 = state_2.health().await;
    assert_eq!(health_2.nodos_4d, 2, "deben restaurarse ambos prospectos desde WAL compactado");
    assert_eq!(
        health_2.ordenes_pendientes, 2,
        "deben restaurarse las 2 órdenes pendientes Gatekeeper tras reinicio"
    );

    let subgrafo = state_2.obtener_subgrafo(prospecto_inicial).await?;
    assert_eq!(subgrafo.prospecto.id, prospecto_inicial);

    let gk = state_2.listar_gatekeeper_pendientes();
    assert!(gk
        .ordenes
        .iter()
        .any(|o| o.orden_id == orden_id_inicial && o.estado == EstadoAprobacion::PendienteGatekeeper));

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn test_cdp_hidrata_spa_y_despacho_smtp_rechaza_correo_invalido_y_opt_out(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = crear_temp_dir("aurora_cdp_smtp")?;

    // 1. Servidor web SPA (shell mínimo sin contenido útil) + servidor WebSocket CDP simulado
    let cdp_listener = TcpListener::bind("127.0.0.1:0").await?;
    let cdp_addr = cdp_listener.local_addr()?;

    let spa_html_hidratado = r#"<html><body><article class="senal-abierta">
        <h1>Nueva Reserva Inmobiliaria en Santa Fe</h1>
        <span class="directivo">Lucía Mendoza Salgado</span>
        <span class="empresa">Corporativo Santa Fe</span>
        <p class="cuerpo">Directora en Corporativo Santa Fe prepara compra de activo inmueble residencial con rendimiento patrimonial en CDMX</p>
    </article></body></html>"#
        .to_string();

    #[derive(Clone)]
    struct MockCdpState {
        ws_url: String,
        html: String,
    }

    let cdp_state = MockCdpState {
        ws_url: format!("ws://{cdp_addr}/devtools/page/tab-1"),
        html: spa_html_hidratado,
    };

    let cdp_router = Router::new()
        .route(
            "/json/version",
            get(|| async { Json(json!({ "Browser": "Chromium/CDP" })) }),
        )
        .route(
            "/json/new",
            put(|State(st): State<MockCdpState>| async move {
                Json(json!({
                    "id": "tab-1",
                    "type": "page",
                    "webSocketDebuggerUrl": st.ws_url
                }))
            }),
        )
        .route(
            "/devtools/page/tab-1",
            get(|ws: WebSocketUpgrade, State(st): State<MockCdpState>| async move {
                ws.on_upgrade(move |mut socket| async move {
                    use axum::extract::ws::Message;
                    while let Some(Ok(Message::Text(txt))) = socket.recv().await {
                        if let Ok(req) = serde_json::from_str::<serde_json::Value>(&txt) {
                            let id = req.get("id").and_then(|v| v.as_u64()).unwrap_or(1);
                            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                            let reply = if method == "Runtime.evaluate" {
                                json!({
                                    "id": id,
                                    "result": {
                                        "result": {
                                            "type": "string",
                                            "value": st.html
                                        }
                                    }
                                })
                            } else {
                                json!({ "id": id, "result": {} })
                            };
                            let _ = socket.send(Message::Text(reply.to_string().into())).await;
                        }
                    }
                })
            }),
        )
        .with_state(cdp_state);

    let cdp_handle = tokio::spawn(async move {
        let _ = axum::serve(cdp_listener, cdp_router).await;
    });

    let web_listener = TcpListener::bind("127.0.0.1:0").await?;
    let web_addr = web_listener.local_addr()?;
    let web_router = Router::new().route(
        "/spa",
        get(|| async { (StatusCode::OK, "<html><body><div id='root'></div></body></html>") }),
    );
    let web_handle = tokio::spawn(async move {
        let _ = axum::serve(web_listener, web_router).await;
    });

    // 2. Estado RealConFallback con CDP activo + SMTP habilitado hacia puerto inválido local para validar rechazos
    let cfg = AuroraDaemonConfig {
        bind_addr: "127.0.0.1:0".to_string(),
        bob_url: "http://127.0.0.1:59999".to_string(),
        cdp_url: format!("http://{cdp_addr}"),
        data_dir: dir.clone(),
        modo_offline: false,
        modo_ejecucion: ModoEjecucion::RealConFallback,
        smtp: ConfigSmtp {
            habilitado: true,
            host: "127.0.0.1".to_string(),
            puerto: 59998,
            remitente_correo: "aurora@inmobiliaria-mx.example".to_string(),
            usar_tls_starttls: false,
            usar_tls_implicito: false,
            url_base_opt_out: "http://127.0.0.1:8090/api/v1/lfpdppp/opt-out".to_string(),
            ..ConfigSmtp::default()
        },
        ..AuroraDaemonConfig::default()
    };

    let state = AuroraDaemonState::inicializar_local(cfg)?;

    let hunt = state
        .ejecutar_hunt(HuntRequest {
            vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial),
            ciudad: "CDMX".to_string(),
            estado: "CDMX".to_string(),
            zona: Some("Santa Fe".to_string()),
            palabras_clave: vec!["residencial".to_string()],
            limite_resultados: Some(5),
            requiere_js: Some(true),
            fuente_url: Some(format!("http://{web_addr}/spa")),
            fuente_formato: Some(FormatoCrudo::Html),
            registros_semilla: None,
        })
        .await?;

    assert_eq!(hunt.prospectos_ingestados, 1);
    assert_eq!(hunt.planes_encolados.len(), 1);

    let orden = hunt
        .planes_encolados
        .first()
        .ok_or("debió encolarse orden tras hidratación CDP")?;

    // Si el plan escogió CorreoEjecutivo y el correo destino es inválido, SmtpEmailTransport DEBE rechazarlo.
    if orden.brief_creativo.canal_sugerido == CanalContacto::CorreoEjecutivo {
        let err_correo = state
            .aprobar_orden(
                orden.orden_id,
                Some(AprobarOrdenRequest {
                    despachar_inmediatamente: Some(true),
                    correo_destino: Some("correo-malformado".to_string()),
                }),
            )
            .await;
        assert!(
            err_correo.is_err(),
            "SMTP real habilitado debe rechazar destinatario inválido sin caer a mock"
        );
    }

    // Baja LFPDPPP opt_out purga WAL y bloquea cualquier aprobación/despacho posterior
    let opt = state
        .ejecutar_opt_out(OptOutRequest {
            prospecto_id: orden.prospecto_id,
            campana_id: Some(orden.campana_id),
            motivo: "Ejercicio de oposición LFPDPPP".to_string(),
        })
        .await?;
    assert!(opt.purgado_de_wal);
    assert!(opt.en_blacklist);

    let err_post_opt_out = state
        .aprobar_orden(orden.orden_id, None)
        .await;
    assert!(err_post_opt_out.is_err());

    cdp_handle.abort();
    web_handle.abort();
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}
