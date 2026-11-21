use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use aurora_daemon::{
    crear_router, AprobarOrdenRequest, AprobarOrdenResponse, AuroraDaemonState,
    DpoConsolidarResponse, GatekeeperPendientesResponse, HealthResponse, HuntRequest, HuntResponse,
    OptOutRequest, OptOutResponse, OrdenPendienteDto, PulseEstadoResponse, RechazarOrdenRequest,
    TelaranaResponse,
};
use aurora_domain::{
    EstadoAprobacion, SubverticalInmobiliaria, Ubicacion, VerticalComercial,
};
use aurora_memory::{IslaVertical, SubgrafoProspecto};
use aurora_osint::{CoMencionCruda, FormatoCrudo, RegistroCrudo};
use aurora_pulse::{EventoTelemetria, TipoEventoTelemetria};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use tokio::net::TcpListener;
use uuid::Uuid;

fn dir_temporal_daemon(prefijo: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()))
}

async fn levantar_servidor_efimero(
    base_dir: &std::path::Path,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let state = AuroraDaemonState::para_pruebas_offline(base_dir)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{addr}");

    let router = crear_router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Ok((base_url, handle))
}

#[tokio::test]
async fn flujo_http_completo_hunt_gatekeeper_factory_pulse_dpo_y_opt_out(
) -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = dir_temporal_daemon("aurora_daemon_api_test");
    let (base_url, server_handle) = levantar_servidor_efimero(&base_dir).await?;
    let client = Client::new();

    // 1. Health inicial + DPO idempotente sin eventos
    let health_resp = client.get(format!("{base_url}/api/v1/health")).send().await?;
    assert_eq!(health_resp.status(), StatusCode::OK);
    let health: HealthResponse = health_resp.json().await?;
    assert_eq!(health.status, "ok");
    assert!(health.bob_online);
    assert_eq!(health.nodos_4d, 0);

    let dpo_vacio_resp = client
        .post(format!("{base_url}/api/v1/dpo/consolidar"))
        .send()
        .await?;
    assert_eq!(dpo_vacio_resp.status(), StatusCode::OK);
    let dpo_vacio: DpoConsolidarResponse = dpo_vacio_resp.json().await?;
    assert!(dpo_vacio.modo_noop);
    assert_eq!(dpo_vacio.campanas_auditadas, 0);
    assert_eq!(
        dpo_vacio.mensaje,
        "Sin eventos de telemetría nuevos para consolidar"
    );

    // 2. Lanzar cacería OSINT con 2 prospectos co-mencionados -> ingestión 4D -> Córtex BOB -> Gatekeeper
    let ahora = Utc::now();
    let ubicacion = Ubicacion {
        ciudad: "Monterrey".to_string(),
        estado: "Nuevo León".to_string(),
        zona: Some("Santa Catarina Industrial".to_string()),
    };

    let hunt_req = HuntRequest {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        ciudad: "Monterrey".to_string(),
        estado: "Nuevo León".to_string(),
        zona: Some("Santa Catarina Industrial".to_string()),
        palabras_clave: vec!["bodega".to_string(), "logistica".to_string()],
        limite_resultados: Some(10),
        requiere_js: Some(false),
        fuente_url: None,
        fuente_formato: None,
        registros_semilla: Some(vec![
            RegistroCrudo {
                fuente_origen: "Boletín Parques Industriales NL".to_string(),
                url_evidencia: "https://datos.ejemplo.mx/nl/parque-701".to_string(),
                capturado_en: ahora,
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Roberto Elizondo Trevino dirige expansion logistica y construccion de nueva bodega industrial con TIR proyectada en Santa Catarina Monterrey".to_string(),
                nombre_detectado: Some("Roberto Elizondo Trevino".to_string()),
                empresa_detectada: Some("Logistica Regia Norte SA".to_string()),
                ubicacion_detectada: Some(ubicacion.clone()),
                co_mencionados: vec![CoMencionCruda {
                    nombre: "Mariana Garza Leal".to_string(),
                    contexto: "Socia operativa Clúster Industrial Monterrey".to_string(),
                    empresa: Some("Logistica Regia Norte SA".to_string()),
                }],
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            },
            RegistroCrudo {
                fuente_origen: "Boletín Parques Industriales NL".to_string(),
                url_evidencia: "https://datos.ejemplo.mx/nl/parque-702".to_string(),
                capturado_en: ahora,
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Mariana Garza Leal inaugura centro de distribucion y compra de activo inmueble en Monterrey Nuevo Leon".to_string(),
                nombre_detectado: Some("Mariana Garza Leal".to_string()),
                empresa_detectada: Some("Logistica Regia Norte SA".to_string()),
                ubicacion_detectada: Some(ubicacion),
                co_mencionados: Vec::new(),
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            },
        ]),
    };

    let hunt_resp = client
        .post(format!("{base_url}/api/v1/osint/hunt"))
        .json(&hunt_req)
        .send()
        .await?;
    assert_eq!(hunt_resp.status(), StatusCode::OK);
    let hunt: HuntResponse = hunt_resp.json().await?;
    assert_eq!(hunt.prospectos_ingestados, 2);
    assert!(hunt.relaciones_creadas >= 1);
    assert_eq!(hunt.planes_encolados.len(), 2);

    // 3. Consultar Telaraña 4D y Subgrafo detallado
    let telarana_resp = client
        .get(format!("{base_url}/api/v1/memory/telarana"))
        .send()
        .await?;
    assert_eq!(telarana_resp.status(), StatusCode::OK);
    let telarana: TelaranaResponse = telarana_resp.json().await?;
    assert_eq!(telarana.total_nodos, 2);
    assert!(telarana.total_aristas >= 1);

    let primer_prospecto = telarana
        .nodos
        .first()
        .ok_or("no se encontraron nodos en telaraña")?
        .prospecto_id;
    let subgrafo_resp = client
        .get(format!(
            "{base_url}/api/v1/memory/subgrafo/{primer_prospecto}"
        ))
        .send()
        .await?;
    assert_eq!(subgrafo_resp.status(), StatusCode::OK);
    let subgrafo: SubgrafoProspecto = subgrafo_resp.json().await?;
    assert_eq!(subgrafo.prospecto.id, primer_prospecto);
    assert!(!subgrafo.vecinos_circulo_cercano.is_empty());

    // 4. Consultar pendientes Gatekeeper
    let gk_resp = client
        .get(format!("{base_url}/api/v1/gatekeeper/pendientes"))
        .send()
        .await?;
    assert_eq!(gk_resp.status(), StatusCode::OK);
    let gk: GatekeeperPendientesResponse = gk_resp.json().await?;
    assert_eq!(gk.total_pendientes, 2);
    assert!(gk
        .ordenes
        .iter()
        .all(|o| o.estado == EstadoAprobacion::PendienteGatekeeper));

    let orden_aprobar = gk
        .ordenes
        .iter()
        .find(|o| o.prospecto_id == primer_prospecto)
        .ok_or("falta orden para primer_prospecto")?
        .clone();
    let orden_rechazar = gk
        .ordenes
        .iter()
        .find(|o| o.prospecto_id != primer_prospecto)
        .ok_or("falta segunda orden para prueba de rechazo")?
        .clone();

    // 5. Aprobar orden #1 -> FabricaMultimodalAurora crea los 4 artefactos en NVMe -> Pulse calienta y despacha
    let apr_resp = client
        .post(format!(
            "{base_url}/api/v1/gatekeeper/aprobar/{}",
            orden_aprobar.orden_id
        ))
        .json(&AprobarOrdenRequest {
            despachar_inmediatamente: Some(true),
            correo_destino: None,
        })
        .send()
        .await?;
    assert_eq!(apr_resp.status(), StatusCode::OK);
    let aprobada: AprobarOrdenResponse = apr_resp.json().await?;
    assert_eq!(aprobada.estado, EstadoAprobacion::Despachado);
    assert!(aprobada.calentamiento_ejecutado.is_some());
    assert!(aprobada.comprobante_despacho.is_some());

    // Verificar existencia física de los 4 archivos en NVMe
    assert!(aprobada.paquete_manufacturado.dossier_pdf.ruta_nvme.exists());
    assert!(aprobada.paquete_manufacturado.nota_voz_wav.ruta_nvme.exists());
    assert!(aprobada
        .paquete_manufacturado
        .reel_vertical_mp4
        .ruta_nvme
        .exists());
    assert!(aprobada
        .paquete_manufacturado
        .subtitulos_srt
        .ruta_nvme
        .exists());

    // 6. Rechazar orden #2 en Gatekeeper
    let rech_resp = client
        .post(format!(
            "{base_url}/api/v1/gatekeeper/rechazar/{}",
            orden_rechazar.orden_id
        ))
        .json(&RechazarOrdenRequest {
            motivo: "Ajustar gancho financiero previo a manufactura".to_string(),
        })
        .send()
        .await?;
    assert_eq!(rech_resp.status(), StatusCode::OK);
    let rechazada: OrdenPendienteDto = rech_resp.json().await?;
    assert_eq!(rechazada.estado, EstadoAprobacion::Rechazado);

    // Pendientes debe quedar en 0
    let gk_post: GatekeeperPendientesResponse = client
        .get(format!("{base_url}/api/v1/gatekeeper/pendientes"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(gk_post.total_pendientes, 0);

    // 6b. Generar una nueva orden directa para prospecto existente desde Telaraña 4D
    let gen_resp = client
        .post(format!(
            "{base_url}/api/v1/cortex/generar-plan/{primer_prospecto}"
        ))
        .send()
        .await?;
    assert_eq!(gen_resp.status(), StatusCode::OK);
    let nueva_orden: OrdenPendienteDto = gen_resp.json().await?;
    assert_eq!(nueva_orden.prospecto_id, primer_prospecto);
    assert_eq!(nueva_orden.estado, EstadoAprobacion::PendienteGatekeeper);
    assert_ne!(nueva_orden.orden_id, orden_aprobar.orden_id);

    let gk_regen: GatekeeperPendientesResponse = client
        .get(format!("{base_url}/api/v1/gatekeeper/pendientes"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(gk_regen.total_pendientes, 1);

    // 7. Consultar /api/v1/pulse/estado e inyectar telemetría contrastante para consolidación DPO
    let pulse_resp = client
        .get(format!("{base_url}/api/v1/pulse/estado"))
        .send()
        .await?;
    assert_eq!(pulse_resp.status(), StatusCode::OK);
    let pulse: PulseEstadoResponse = pulse_resp.json().await?;
    assert_eq!(pulse.campanas_activas.len(), 1);
    assert!(!pulse.calentamientos_recientes.is_empty());
    assert!(!pulse.despachos_recientes.is_empty());

    let tracking_id = aprobada
        .comprobante_despacho
        .as_ref()
        .map(|c| c.tracking_id)
        .ok_or("comprobante_despacho ausente")?;

    // Evento de alta retención (85%) + evento secundario con gancho perdedor (8%) para contraste DPO
    client
        .post(format!("{base_url}/api/v1/pulse/telemetria"))
        .json(&EventoTelemetria::nuevo(
            aprobada.campana_id,
            aprobada.prospecto_id,
            tracking_id,
            Some(IslaVertical::InmobiliariaComercialIndustrial),
            orden_aprobar.brief_creativo.gancho_3s.clone(),
            orden_aprobar.brief_creativo.guion_tts.clone(),
            TipoEventoTelemetria::RetencionReel {
                segundos_vistos: 25.5,
                duracion_total_s: 30.0,
                completado: true,
            },
        ))
        .send()
        .await?
        .error_for_status()?;

    client
        .post(format!("{base_url}/api/v1/pulse/telemetria"))
        .json(&EventoTelemetria::nuevo(
            Uuid::new_v4(),
            orden_rechazar.prospecto_id,
            Uuid::new_v4(),
            Some(IslaVertical::InmobiliariaComercialIndustrial),
            "¿Buscas bodega genérica?".to_string(),
            "Mensaje genérico sin TIR ni datos".to_string(),
            TipoEventoTelemetria::RetencionReel {
                segundos_vistos: 2.4,
                duracion_total_s: 30.0,
                completado: false,
            },
        ))
        .send()
        .await?
        .error_for_status()?;

    let dpo_resp = client
        .post(format!("{base_url}/api/v1/dpo/consolidar"))
        .send()
        .await?;
    assert_eq!(dpo_resp.status(), StatusCode::OK);
    let dpo: DpoConsolidarResponse = dpo_resp.json().await?;
    assert!(dpo.dataset.ruta_jsonl.exists());
    assert!(!dpo.dataset.entradas.is_empty());

    // 8. Ejecutar purga LFPDPPP opt-out y validar 404 posterior en subgrafo/:id
    let opt_out_resp = client
        .post(format!("{base_url}/api/v1/lfpdppp/opt-out"))
        .json(&OptOutRequest {
            prospecto_id: primer_prospecto,
            campana_id: Some(aprobada.campana_id),
            motivo: "Titular solicitó baja ARCO vía Cockpit".to_string(),
        })
        .send()
        .await?;
    assert_eq!(opt_out_resp.status(), StatusCode::OK);
    let opt_out: OptOutResponse = opt_out_resp.json().await?;
    assert!(opt_out.purgado_de_wal);
    assert!(opt_out.en_blacklist);

    let subgrafo_purgado = client
        .get(format!(
            "{base_url}/api/v1/memory/subgrafo/{primer_prospecto}"
        ))
        .send()
        .await?;
    assert_eq!(subgrafo_purgado.status(), StatusCode::NOT_FOUND);

    server_handle.abort();
    let _ = fs::remove_dir_all(&base_dir);
    Ok(())
}
