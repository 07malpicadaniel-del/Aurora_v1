use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use aurora_council::{MiembroConsejo, ResultadoDelegacion, TipoTareaConsejo};
use aurora_daemon::{
    crear_router, AprobarOrdenRequest, AprobarOrdenResponse, AuroraDaemonState,
    BrollEstadoResponse, CatalogarProspectosRequest, CatalogarProspectosResponse,
    CouncilCuotasResponse, CrearOrdenForjaRequest, DelegarConsejoRequest, DpoConsolidarResponse,
    ForgeOrdenesResponse, GatekeeperPendientesResponse, GenerarBriefsHiggsfieldRequest,
    GenerarBriefsHiggsfieldResponse, GenerarMonetizacionRequest, GenerarMonetizacionResponse,
    HealthResponse, HuntRequest, HuntResponse, OptOutRequest, OptOutResponse, OrdenPendienteDto,
    ProspectosDbResponse, PublicarOrganicoRequest, PublicarOrganicoResponse, PulseEstadoResponse,
    PulseOrganicoResponse, RechazarOrdenRequest, RegistrarClipBrollRequest, TelaranaResponse,
};
use aurora_domain::{
    EstadoAprobacion, ObjetivoContenidoVideo, PerfilPsicologico, SubverticalInmobiliaria,
    SubverticalSeguros, Ubicacion, VerticalComercial,
};
use aurora_factory::{sha256_bytes, ClipBroll, EstadoBriefHiggsfield};
use aurora_forge::TipoOrdenIngenieria;
use aurora_memory::{IslaVertical, SubgrafoProspecto};
use aurora_osint::{CoMencionCruda, FormatoCrudo, RegistroCrudo};
use aurora_pulse::{
    EventoTelemetria, PlataformaPulso, SolicitudPublicacionOrganica, TipoEventoTelemetria,
};
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

#[tokio::test]
async fn endpoints_http_council_cuotas_delegar_y_forge_ordenes(
) -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = dir_temporal_daemon("aurora_daemon_council_forge_test");
    let (base_url, server_handle) = levantar_servidor_efimero(&base_dir).await?;
    let client = Client::new();

    // 1. /api/v1/health expone la disponibilidad sondeada de los 4 CLIs del consejo
    let health: HealthResponse = client
        .get(format!("{base_url}/api/v1/health"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(health.subsistemas.consejo_clis.len(), 4);

    // 2. GET /api/v1/council/cuotas devuelve los 5 miembros y la cascada terminando en BobLocal
    let cuotas_resp = client
        .get(format!("{base_url}/api/v1/council/cuotas"))
        .send()
        .await?;
    assert_eq!(cuotas_resp.status(), StatusCode::OK);
    let cuotas: CouncilCuotasResponse = cuotas_resp.json().await?;
    assert_eq!(cuotas.miembros.len(), 5);
    assert_eq!(cuotas.cascada_vigente.last(), Some(&MiembroConsejo::BobLocal));
    assert!(cuotas
        .miembros
        .iter()
        .any(|m| m.miembro == MiembroConsejo::BobLocal && m.ilimitado));

    // 3. POST /api/v1/council/delegar sanitiza PII mexicana y actualiza el contador NVMe
    let delegar_resp = client
        .post(format!("{base_url}/api/v1/council/delegar"))
        .json(&DelegarConsejoRequest {
            tarea: TipoTareaConsejo::CopyAltoTicket,
            contexto: "Contactar a direccion@patrimonio-mx.com al +52 81 8345 6789 sobre nave industrial.".to_string(),
            prospecto_id: None,
            prospecto: None,
            forzar_local: Some(false),
            timeout_ms: Some(5_000),
        })
        .send()
        .await?;
    assert_eq!(delegar_resp.status(), StatusCode::OK);
    let delegacion: ResultadoDelegacion = delegar_resp.json().await?;
    assert_eq!(delegacion.miembro_respondedor, MiembroConsejo::ClaudePro);
    assert!(delegacion.pii_sanitizado);
    assert_eq!(delegacion.marcadores_pii_aplicados, 2);
    assert!(delegacion.contenido.contains("direccion@patrimonio-mx.com"));
    assert!(delegacion.contenido.contains("+52 81 8345 6789"));

    let cuotas_post: CouncilCuotasResponse = client
        .get(format!("{base_url}/api/v1/council/cuotas"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let claude_post = cuotas_post
        .miembros
        .iter()
        .find(|m| m.miembro == MiembroConsejo::ClaudePro);
    assert!(claude_post.is_some_and(|c| c.usado_ventana_5h == 1 && c.usado_diario == 1));

    // 4. GET /api/v1/forge/ordenes responde 200 OK con estado habilitado y lista inicial
    let forge_resp = client
        .get(format!("{base_url}/api/v1/forge/ordenes"))
        .send()
        .await?;
    assert_eq!(forge_resp.status(), StatusCode::OK);
    let forge: ForgeOrdenesResponse = forge_resp.json().await?;
    assert!(forge.habilitado);
    assert_eq!(forge.total_ordenes, 0);

    // 5. POST /api/v1/forge/ordenes rechaza con 403 FORBIDDEN intentos sobre rutas protegidas
    let forge_bloqueo = client
        .post(format!("{base_url}/api/v1/forge/ordenes"))
        .json(&CrearOrdenForjaRequest {
            tipo: TipoOrdenIngenieria::SelfHealingScraper,
            descripcion_tecnica: "Intento no permitido sobre validation.rs".to_string(),
            rutas_permitidas: vec!["crates/aurora-domain/src/validation.rs".to_string()],
            comando_validacion: None,
            repo_root: None,
        })
        .send()
        .await?;
    assert_eq!(forge_bloqueo.status(), StatusCode::FORBIDDEN);

    server_handle.abort();
    let _ = fs::remove_dir_all(&base_dir);
    Ok(())
}

#[tokio::test]
async fn endpoints_http_factory_broll_briefs_y_registro_mp4(
) -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = dir_temporal_daemon("aurora_daemon_broll_test");
    let (base_url, server_handle) = levantar_servidor_efimero(&base_dir).await?;
    let client = Client::new();

    // 1. Estado inicial vacío con 8 brechas de cobertura
    let estado_ini: BrollEstadoResponse = client
        .get(format!("{base_url}/api/v1/factory/broll"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(estado_ini.total_clips, 0);
    assert_eq!(estado_ini.brechas_cobertura.len(), 8);

    // 2. Generar briefs cinematográficos para Higgsfield
    let vertical = VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial);
    let briefs_resp: GenerarBriefsHiggsfieldResponse = client
        .post(format!("{base_url}/api/v1/factory/broll/briefs"))
        .json(&GenerarBriefsHiggsfieldRequest {
            vertical,
            perfil_objetivo: PerfilPsicologico::AnaliticoFinanciero,
            cantidad: Some(2),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(briefs_resp.briefs_generados.len(), 2);
    assert_eq!(briefs_resp.total_en_cola, 2);
    let primer_brief_id = briefs_resp.briefs_generados[0].id;

    // 3. Rechazar path traversal en registro B-Roll (400 BAD_REQUEST)
    let resp_traversal = client
        .post(format!("{base_url}/api/v1/factory/broll/registrar"))
        .json(&RegistrarClipBrollRequest {
            ruta_origen: PathBuf::from("../etc/passwd"),
            vertical,
            perfil_objetivo: None,
            etiquetas: Vec::new(),
            duracion_ms: Some(5_000),
            ancho: Some(1080),
            alto: Some(1920),
            movimiento_camara: Some("dolly_in_slow".to_string()),
            brief_id: None,
        })
        .send()
        .await?;
    assert_eq!(resp_traversal.status(), StatusCode::BAD_REQUEST);

    // 4. Registrar clip MP4 válido y enlazar con el brief Higgsfield generado
    let clip_src = base_dir.join("higgsfield_clip_demo.mp4");
    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&24_u32.to_be_bytes());
    mp4.extend_from_slice(b"ftyp");
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(&512_u32.to_be_bytes());
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(b"mp42");
    fs::write(&clip_src, &mp4)?;

    let clip: ClipBroll = client
        .post(format!("{base_url}/api/v1/factory/broll/registrar"))
        .json(&RegistrarClipBrollRequest {
            ruta_origen: clip_src,
            vertical,
            perfil_objetivo: Some(PerfilPsicologico::AnaliticoFinanciero),
            etiquetas: vec!["parque_industrial".to_string()],
            duracion_ms: Some(5_000),
            ancho: Some(1080),
            alto: Some(1920),
            movimiento_camara: Some("fpv_architectural_glide".to_string()),
            brief_id: Some(primer_brief_id),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(clip.ruta_nvme.is_file());
    assert_eq!(clip.sha256.len(), 64);

    // 5. Confirmar inventario actualizado y brief marcado como Importado
    let estado_post: BrollEstadoResponse = client
        .get(format!("{base_url}/api/v1/factory/broll"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(estado_post.total_clips, 1);
    assert_eq!(
        estado_post
            .clips_por_vertical
            .get("inmobiliaria/comercial_industrial"),
        Some(&1)
    );
    assert!(estado_post
        .briefs_higgsfield
        .iter()
        .any(|b| b.id == primer_brief_id && b.estado == EstadoBriefHiggsfield::Importado));

    server_handle.abort();
    let _ = fs::remove_dir_all(&base_dir);
    Ok(())
}

#[tokio::test]
async fn portal_mobile_auth_bearer_y_publicacion_organica_http(
) -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = dir_temporal_daemon("aurora_daemon_mobile_organic_test");
    let token_secreto = "aurora-tailscale-token-2026".to_string();
    let state =
        AuroraDaemonState::para_pruebas_offline_con_token(&base_dir, Some(token_secreto.clone()))?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{addr}");
    let router = crear_router(state);
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let client = Client::new();

    // 1. GET /mobile sirve el HTML autocontenido monocromático sin bloquear navegador móvil
    let mobile_resp = client.get(format!("{base_url}/mobile")).send().await?;
    assert_eq!(mobile_resp.status(), StatusCode::OK);
    let html = mobile_resp.text().await?;
    assert!(html.contains("Aurora v2 · Carril Ámbar"));
    assert!(html.contains("Sala Gatekeeper"));

    // 2. /api/v1/* rechaza solicitudes sin token o con token incorrecto (401 UNAUTHORIZED)
    let sin_token = client.get(format!("{base_url}/api/v1/health")).send().await?;
    assert_eq!(sin_token.status(), StatusCode::UNAUTHORIZED);

    let token_malo = client
        .get(format!("{base_url}/api/v1/pulse/organico"))
        .bearer_auth("token-invalido")
        .send()
        .await?;
    assert_eq!(token_malo.status(), StatusCode::UNAUTHORIZED);

    // 2.5 POST /api/v1/pulse/organico/publicar rechaza con 422 cualquier fuga de PII
    let pii_resp = client
        .post(format!("{base_url}/api/v1/pulse/organico/publicar"))
        .bearer_auth(&token_secreto)
        .json(&PublicarOrganicoRequest {
            plataforma: PlataformaPulso::YouTubeShorts,
            solicitud: SolicitudPublicacionOrganica {
                campana_id: Uuid::new_v4(),
                vertical: VerticalComercial::Inmobiliaria(
                    SubverticalInmobiliaria::ComercialIndustrial,
                ),
                titulo: "Contacto directo: director@empresa-real.mx".to_string(),
                descripcion: "Análisis con correo expuesto".to_string(),
                hashtags: vec!["Monterrey".to_string()],
                ruta_mp4: base_dir.join("reel_organico_carril_verde.mp4"),
                sha256_mp4: "0".repeat(64),
                programado_para: None,
                libre_de_pii: true,
                objetivo: None,
            },
        })
        .send()
        .await?;
    assert_eq!(pii_resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 3. Con Bearer válido, GET /api/v1/pulse/organico y POST /api/v1/pulse/organico/publicar funcionan
    let estado_org: PulseOrganicoResponse = client
        .get(format!("{base_url}/api/v1/pulse/organico"))
        .bearer_auth(&token_secreto)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(estado_org.publicadores.len(), 3);
    assert!(estado_org.publicaciones_realizadas.is_empty());

    let clip_mp4 = base_dir.join("reel_organico_carril_verde.mp4");
    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&24_u32.to_be_bytes());
    mp4.extend_from_slice(b"ftyp");
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(&512_u32.to_be_bytes());
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(b"mp42");
    fs::write(&clip_mp4, &mp4)?;
    let hash_mp4 = sha256_bytes(&mp4);

    let pub_resp: PublicarOrganicoResponse = client
        .post(format!("{base_url}/api/v1/pulse/organico/publicar"))
        .bearer_auth(&token_secreto)
        .json(&PublicarOrganicoRequest {
            plataforma: PlataformaPulso::YouTubeShorts,
            solicitud: SolicitudPublicacionOrganica {
                campana_id: Uuid::new_v4(),
                vertical: VerticalComercial::Inmobiliaria(
                    SubverticalInmobiliaria::ComercialIndustrial,
                ),
                titulo: "Claves de plusvalía industrial en Monterrey 2026".to_string(),
                descripcion: "Análisis educativo sobre parques Clase A y absorción logística."
                    .to_string(),
                hashtags: vec!["Nearshoring".to_string(), "Monterrey".to_string()],
                ruta_mp4: clip_mp4,
                sha256_mp4: hash_mp4.clone(),
                programado_para: None,
                libre_de_pii: true,
                objetivo: None,
            },
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!pub_resp.encolado);
    let comp = pub_resp
        .comprobante
        .ok_or("se esperaba comprobante de publicación orgánica")?;
    assert_eq!(comp.plataforma, PlataformaPulso::YouTubeShorts);
    assert_eq!(comp.sha256_mp4, hash_mp4);

    let estado_org_post: PulseOrganicoResponse = client
        .get(format!("{base_url}/api/v1/pulse/organico"))
        .bearer_auth(&token_secreto)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(estado_org_post.publicaciones_realizadas.len(), 1);

    server_handle.abort();
    let _ = fs::remove_dir_all(&base_dir);
    Ok(())
}

#[tokio::test]
async fn endpoints_http_prospectos_db_catalogacion_opt_out_monetizacion_y_tiktok(
) -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = dir_temporal_daemon("aurora_fase15_db_tiktok");
    let (base_url, server_handle) = levantar_servidor_efimero(&base_dir).await?;
    let client = Client::new();

    let hunt_seg: HuntResponse = client
        .post(format!("{base_url}/api/v1/osint/hunt"))
        .json(&HuntRequest {
            vertical: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
            ciudad: "Boca del Río".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Costa de Oro".to_string()),
            palabras_clave: vec!["hombre clave".to_string(), "expansión logística".to_string()],
            limite_resultados: Some(3),
            requiere_js: Some(false),
            fuente_url: None,
            fuente_formato: None,
            registros_semilla: Some(vec![RegistroCrudo {
                fuente_origen: "Boletín Empresarial Veracruz".to_string(),
                url_evidencia: "https://ejemplo-publico.mx/veracruz/expansion-2026".to_string(),
                capturado_en: Utc::now(),
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Lic. Valeria Montesinos Prado lidera expansión logística y sucesión patrimonial en Boca del Río con inversión de 40 MDP.".to_string(),
                nombre_detectado: Some("Valeria Montesinos Prado".to_string()),
                empresa_detectada: Some("Operadora Portuaria Montesinos S.A. de C.V.".to_string()),
                ubicacion_detectada: Some(Ubicacion {
                    ciudad: "Boca del Río".to_string(),
                    estado: "Veracruz".to_string(),
                    zona: Some("Costa de Oro".to_string()),
                }),
                co_mencionados: Vec::new(),
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            }]),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(hunt_seg.prospectos_ingestados, 1);
    let prospecto_seg_id = hunt_seg.planes_encolados[0].prospecto_id;

    let hunt_inm: HuntResponse = client
        .post(format!("{base_url}/api/v1/osint/hunt"))
        .json(&HuntRequest {
            vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
            ciudad: "Monterrey".to_string(),
            estado: "Nuevo León".to_string(),
            zona: Some("Apodaca".to_string()),
            palabras_clave: vec!["nave industrial".to_string()],
            limite_resultados: Some(3),
            requiere_js: Some(false),
            fuente_url: None,
            fuente_formato: None,
            registros_semilla: Some(vec![RegistroCrudo {
                fuente_origen: "Registro Industrial NL".to_string(),
                url_evidencia: "https://ejemplo-publico.mx/nl/parque-apodaca".to_string(),
                capturado_en: Utc::now(),
                formato: FormatoCrudo::TextoPlano,
                contenido_crudo: "Ing. Ernesto Salgado Garza amplía bodega industrial en Apodaca Nuevo León.".to_string(),
                nombre_detectado: Some("Ernesto Salgado Garza".to_string()),
                empresa_detectada: Some("Logística Regia S.A.".to_string()),
                ubicacion_detectada: Some(Ubicacion {
                    ciudad: "Monterrey".to_string(),
                    estado: "Nuevo León".to_string(),
                    zona: Some("Apodaca".to_string()),
                }),
                co_mencionados: Vec::new(),
                datos_extra: BTreeMap::new(),
                opt_out_detectado: false,
            }]),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(hunt_inm.prospectos_ingestados, 1);

    let db_completa: ProspectosDbResponse = client
        .get(format!(
            "{base_url}/api/v1/prospectos/db?orden=probabilidad_desc"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(db_completa.resumen.total, 2);
    assert_eq!(db_completa.resumen.sin_evaluar, 0);
    assert_eq!(db_completa.total_filtrado, 2);
    let p0 = db_completa.prospectos[0]
        .catalogacion
        .as_ref()
        .ok_or("se esperaba catalogación automática")?
        .probabilidad_compra;
    let p1 = db_completa.prospectos[1]
        .catalogacion
        .as_ref()
        .ok_or("se esperaba catalogación automática")?
        .probabilidad_compra;
    assert!(p0 >= p1);

    let db_seg: ProspectosDbResponse = client
        .get(format!(
            "{base_url}/api/v1/prospectos/db?sector=Seguros&q=Valeria"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(db_seg.total_filtrado, 1);
    assert_eq!(db_seg.prospectos[0].prospecto_id, prospecto_seg_id);

    let cat_ind: CatalogarProspectosResponse = client
        .post(format!("{base_url}/api/v1/prospectos/catalogar"))
        .json(&CatalogarProspectosRequest {
            prospecto_id: Some(prospecto_seg_id),
            solo_sin_evaluar: None,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(cat_ind.total_evaluados, 1);
    assert_eq!(cat_ind.evaluados[0].prospecto_id, prospecto_seg_id);

    let cat_lote: CatalogarProspectosResponse = client
        .post(format!("{base_url}/api/v1/prospectos/catalogar"))
        .json(&CatalogarProspectosRequest {
            prospecto_id: None,
            solo_sin_evaluar: Some(false),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(cat_lote.total_evaluados, 2);

    let ruta_cat = base_dir.join("wal/catalogacion_compra.json");
    assert!(ruta_cat.is_file());
    let contenido_antes = fs::read_to_string(&ruta_cat)?;
    assert!(contenido_antes.contains(&prospecto_seg_id.to_string()));

    let opt_resp: OptOutResponse = client
        .post(format!("{base_url}/api/v1/lfpdppp/opt-out"))
        .json(&OptOutRequest {
            prospecto_id: prospecto_seg_id,
            campana_id: Some(hunt_seg.campana_id),
            motivo: "Prueba Fase 15 purga DB y catalogación".to_string(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(opt_resp.purgado_de_wal);

    let contenido_despues = fs::read_to_string(&ruta_cat)?;
    assert!(!contenido_despues.contains(&prospecto_seg_id.to_string()));

    let db_post_optout: ProspectosDbResponse = client
        .get(format!("{base_url}/api/v1/prospectos/db"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(db_post_optout.resumen.total, 1);

    let resp_mon_pii = client
        .post(format!("{base_url}/api/v1/content/generar-monetizacion"))
        .json(&GenerarMonetizacionRequest {
            vertical: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
            plataforma: PlataformaPulso::TikTok,
            objetivo: Some(ObjetivoContenidoVideo::MonetizacionAlcance),
            tema_opcional: Some("Caso privado de Ernesto Salgado Garza".to_string()),
            perfil_objetivo: None,
            publicar_ahora: Some(true),
            usar_higgsfield_api: Some(false),
        })
        .send()
        .await?;
    assert_eq!(resp_mon_pii.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let resp_mon_ok: GenerarMonetizacionResponse = client
        .post(format!("{base_url}/api/v1/content/generar-monetizacion"))
        .json(&GenerarMonetizacionRequest {
            vertical: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
            plataforma: PlataformaPulso::TikTok,
            objetivo: Some(ObjetivoContenidoVideo::MonetizacionAlcance),
            tema_opcional: Some("Deducibilidad fiscal para PyMEs en México".to_string()),
            perfil_objetivo: Some(PerfilPsicologico::AnaliticoFinanciero),
            publicar_ahora: Some(true),
            usar_higgsfield_api: Some(true),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(resp_mon_ok.libre_de_pii_verificado);
    assert!(resp_mon_ok.paquete.elegible_creator_rewards_tiktok);
    assert!(resp_mon_ok.paquete.duracion_estimada_seg >= 61);
    assert!(resp_mon_ok.publicado);
    let comp_tt = resp_mon_ok
        .comprobante_publicacion
        .ok_or("se esperaba comprobante de publicación en TikTok")?;
    assert_eq!(comp_tt.plataforma, PlataformaPulso::TikTok);
    assert!(comp_tt.url_publica.starts_with("https://www.tiktok.com/"));
    assert!(resp_mon_ok.resultado_higgsfield.is_some());

    server_handle.abort();
    let _ = fs::remove_dir_all(&base_dir);
    Ok(())
}
