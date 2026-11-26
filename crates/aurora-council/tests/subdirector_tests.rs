use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use aurora_cortex::DeterministicMockBob;
use aurora_council::{
    BobLocalFallbackDriver, ConfigContextoAislado, ConfigCuotasConsejo, CouncilAgentDriver,
    CouncilError, CouncilResult, DeterministicMockCouncilDriver, EstadoSubtarea, MiembroConsejo,
    OrquestadorConsejo, PoliticaCuotaProveedor, QuotaGovernor, RespuestaConsejo, RolEspecialista,
    SolicitudConsejo, SolicitudMisionProyecto, SubdirectorProyectosBob,
};
use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    TipoGatilloVital, Ubicacion, VerticalComercial,
};
use chrono::Utc;
use uuid::Uuid;

fn directorio_temporal(prefijo: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn prospecto_activo_mx() -> Prospecto {
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Valeria Salgado Montes".to_string(),
        aliases: vec![
            "Desarrollos Salgado S.A. de C.V.".to_string(),
            "valeria.salgado@salgado-corp.mx".to_string(),
        ],
        ubicacion: Ubicacion {
            ciudad: "Querétaro".to_string(),
            estado: "Querétaro".to_string(),
            zona: Some("Juriquilla".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 14_200_000,
            patrimonio_estimado_mxn: Some(48_000_000),
        },
        perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "DENUE_INEGI".to_string(),
            url_evidencia: "https://www.inegi.org.mx/app/mapa/denue/".to_string(),
            capturado_en: Utc::now(),
            opt_out: false,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::ExpansionLogistica,
            descripcion: "Búsqueda de parque industrial en El Marqués".to_string(),
            ocurrido_en: Utc::now(),
            peso_urgencia: 0.88,
        }],
    }
}

/// Driver espía para inspeccionar en vivo el contenido del directorio staging durante la ejecución.
struct SpySandboxDriver {
    miembro: MiembroConsejo,
    staging_capturado: Mutex<Option<PathBuf>>,
    archivos_vistos: Mutex<Vec<String>>,
}

impl SpySandboxDriver {
    fn new(miembro: MiembroConsejo) -> Self {
        Self {
            miembro,
            staging_capturado: Mutex::new(None),
            archivos_vistos: Mutex::new(Vec::new()),
        }
    }

    fn staging_dir(&self) -> Option<PathBuf> {
        self.staging_capturado
            .lock()
            .ok()
            .and_then(|g| g.clone())
    }

    fn archivos_en_staging(&self) -> Vec<String> {
        self.archivos_vistos
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl CouncilAgentDriver for SpySandboxDriver {
    fn miembro(&self) -> MiembroConsejo {
        self.miembro
    }

    fn disponible(&self) -> bool {
        true
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        if let Some(ref dir) = solicitud.directorio_trabajo {
            if let Ok(mut g) = self.staging_capturado.lock() {
                *g = Some(dir.clone());
            }
            let mut lista = Vec::new();
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    lista.push(entry.file_name().to_string_lossy().to_string());
                }
            }
            if let Ok(mut g) = self.archivos_vistos.lock() {
                *g = lista;
            }
        }
        Ok(RespuestaConsejo {
            miembro: self.miembro,
            contenido: "Entorno aislado auditado correctamente.".to_string(),
            latencia_ms: 4,
            modelo_reportado: Some("spy-sandbox".to_string()),
        })
    }
}

#[tokio::test]
async fn paralelismo_real_tres_tracks_simultaneos_menor_a_350ms(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_subdirector_parallel");
    let ruta_cuotas = tmp.join("quota_state.json");
    let proyecto_dir = tmp.join("repo");
    let sandbox_root = tmp.join("sandboxes");
    fs::create_dir_all(&proyecto_dir)?;
    fs::write(
        proyecto_dir.join("AURORA_AGENT_CONTEXT.md"),
        "# Aurora v2\nArquitectura limpia Rust en Arch Linux sin PII.\n",
    )?;

    let governor = Arc::new(QuotaGovernor::abrir(ConfigCuotasConsejo::con_ruta(
        &ruta_cuotas,
    ))?);

    let mock_claude = Arc::new(
        DeterministicMockCouncilDriver::exito(
            MiembroConsejo::ClaudePro,
            "Gancho 3s + Guion comercial en español de México listo.",
        )
        .con_latencia_ms(150),
    );
    let mock_codex = Arc::new(
        DeterministicMockCouncilDriver::exito(
            MiembroConsejo::CodexPlus,
            "Conector Rust OSINT propuesto sin unwrap()/expect().",
        )
        .con_latencia_ms(150),
    );
    let mock_antigravity = Arc::new(
        DeterministicMockCouncilDriver::exito(
            MiembroConsejo::AntigravityGemini,
            "Manifiesto de 6 escenas 9:16 (>=61s) para NVENC local + cola manual Higgsfield.",
        )
        .con_latencia_ms(150),
    );
    let bob_driver = Arc::new(BobLocalFallbackDriver::new(DeterministicMockBob));

    let consejo = Arc::new(
        OrquestadorConsejo::new(governor)
            .con_driver(mock_claude.clone())
            .con_driver(mock_codex.clone())
            .con_driver(mock_antigravity.clone())
            .con_driver(bob_driver),
    );

    let subdirector = SubdirectorProyectosBob::new(
        consejo,
        ConfigContextoAislado::por_defecto(&proyecto_dir, &sandbox_root),
    );

    let inicio = Instant::now();
    let reporte = subdirector
        .ejecutar_mision(
            SolicitudMisionProyecto {
                titulo: "Campaña Naves Industriales Bajío Q4".to_string(),
                objetivo: "Producir guion de venta, arquitectura de conector DENUE y escenas de video 9:16."
                    .to_string(),
                vertical: Some(VerticalComercial::Inmobiliaria(
                    aurora_domain::SubverticalInmobiliaria::ComercialIndustrial,
                )),
                tracks: vec![
                    RolEspecialista::GuionesYCopy,
                    RolEspecialista::IngenieriaYConectores,
                    RolEspecialista::DireccionVideoYContexto,
                ],
                incluir_contexto_proyecto: true,
                prospecto_ids: Vec::new(),
                ejecutar_forge_en_codigo: false,
                higgsfield_pagado: false,
                timeout_ms: Some(5_000),
            },
            &[],
            &[],
        )
        .await?;
    let tiempo_pared_ms = inicio.elapsed().as_millis();

    assert_eq!(reporte.subtareas.len(), 3);
    assert!(
        tiempo_pared_ms < 350,
        "Se esperaba ejecución paralela (< 350 ms para 3x150ms), tomó {tiempo_pared_ms} ms"
    );
    assert!(reporte.auditoria_privacidad.libre_de_fugas);
    assert!(reporte.sandbox_limpiado);
    assert_eq!(reporte.higgsfield_modo, "cola_manual_sin_pago");

    // Cada especialista ejecutó su track de forma independiente y simultánea.
    assert_eq!(reporte.subtareas[0].rol, RolEspecialista::GuionesYCopy);
    assert_eq!(
        reporte.subtareas[0].miembro_ejecutor,
        MiembroConsejo::ClaudePro
    );
    assert_eq!(
        reporte.subtareas[1].rol,
        RolEspecialista::IngenieriaYConectores
    );
    assert_eq!(
        reporte.subtareas[1].miembro_ejecutor,
        MiembroConsejo::CodexPlus
    );
    assert_eq!(
        reporte.subtareas[2].rol,
        RolEspecialista::DireccionVideoYContexto
    );
    assert_eq!(
        reporte.subtareas[2].miembro_ejecutor,
        MiembroConsejo::AntigravityGemini
    );

    for st in &reporte.subtareas {
        assert_eq!(st.estado, EstadoSubtarea::Completada);
        assert!(!st.fallback_usado);
    }

    assert_eq!(mock_claude.solicitudes_recibidas().len(), 1);
    assert_eq!(mock_codex.solicitudes_recibidas().len(), 1);
    assert_eq!(mock_antigravity.solicitudes_recibidas().len(), 1);

    // Compatibilidad de alias serde previo `GeminiEstudiante` -> `AntigravityGemini`.
    let compat: MiembroConsejo = serde_json::from_str("\"GeminiEstudiante\"")?;
    assert_eq!(compat, MiembroConsejo::AntigravityGemini);

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn reasignacion_automatica_por_cuota_claude_agotado_a_antigravity(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_subdirector_quota_fallback");
    let ruta_cuotas = tmp.join("quota_state.json");
    let proyecto_dir = tmp.join("repo");
    let sandbox_root = tmp.join("sandboxes");
    fs::create_dir_all(&proyecto_dir)?;

    let config = ConfigCuotasConsejo::con_ruta(&ruta_cuotas).con_politica(
        MiembroConsejo::ClaudePro,
        PoliticaCuotaProveedor {
            max_tareas_ventana_5h: 1,
            max_tareas_diarias: 2,
            cooldown_rate_limit_segundos: 1_800,
            ilimitado: false,
        },
    );
    let governor = Arc::new(QuotaGovernor::abrir(config)?);
    // Agotamos cuota de ClaudePro intencionalmente.
    governor.registrar_uso(MiembroConsejo::ClaudePro).await?;
    assert!(!governor.puede_ejecutar(MiembroConsejo::ClaudePro).await);

    let mock_claude = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::ClaudePro,
        "Claude NO debería invocarse con cuota agotada",
    ));
    let mock_antigravity = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::AntigravityGemini,
        "Guion de seguro Hombre Clave redactado por Antigravity/Gemini como respaldo.",
    ));
    let bob_driver = Arc::new(BobLocalFallbackDriver::new(DeterministicMockBob));

    let consejo = Arc::new(
        OrquestadorConsejo::new(governor)
            .con_driver(mock_claude.clone())
            .con_driver(mock_antigravity.clone())
            .con_driver(bob_driver),
    );

    let subdirector = SubdirectorProyectosBob::new(
        consejo,
        ConfigContextoAislado::por_defecto(&proyecto_dir, &sandbox_root),
    );

    let reporte = subdirector
        .ejecutar_mision(
            SolicitudMisionProyecto {
                titulo: "Campaña Hombre Clave PyMEs".to_string(),
                objetivo: "Redactar guion persuasivo de deducibilidad fiscal.".to_string(),
                vertical: Some(VerticalComercial::Seguros(
                    aurora_domain::SubverticalSeguros::HombreClave,
                )),
                tracks: vec![RolEspecialista::GuionesYCopy],
                incluir_contexto_proyecto: false,
                prospecto_ids: Vec::new(),
                ejecutar_forge_en_codigo: false,
                higgsfield_pagado: false,
                timeout_ms: Some(5_000),
            },
            &[],
            &[],
        )
        .await?;

    assert_eq!(reporte.subtareas.len(), 1);
    let st = &reporte.subtareas[0];
    assert_eq!(st.estado, EstadoSubtarea::Completada);
    assert_eq!(st.miembro_ejecutor, MiembroConsejo::AntigravityGemini);
    assert!(st.fallback_usado);
    assert!(mock_claude.solicitudes_recibidas().is_empty());
    assert_eq!(mock_antigravity.solicitudes_recibidas().len(), 1);
    assert!(st.intentos_fallidos
        .iter()
        .any(|i| i.miembro == MiembroConsejo::ClaudePro));

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn escudo_cero_fugas_aborta_por_secreto_ghp_y_por_correo_prospecto_antes_de_invocar_cli(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_subdirector_zero_leak");
    let ruta_cuotas = tmp.join("quota_state.json");
    let proyecto_dir = tmp.join("repo");
    let sandbox_root = tmp.join("sandboxes");
    fs::create_dir_all(&proyecto_dir)?;

    let governor = Arc::new(QuotaGovernor::abrir(ConfigCuotasConsejo::con_ruta(
        &ruta_cuotas,
    ))?);
    let mock_claude = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::ClaudePro,
        "Nunca debe ejecutarse si falla ContextPrivacyAudit",
    ));
    let mock_antigravity = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::AntigravityGemini,
        "Nunca debe ejecutarse si falla ContextPrivacyAudit",
    ));

    let consejo = Arc::new(
        OrquestadorConsejo::new(governor)
            .con_driver(mock_claude.clone())
            .con_driver(mock_antigravity.clone()),
    );

    // Caso 1: Secreto `ghp_...` infiltrado dentro de un documento de la lista blanca.
    fs::write(
        proyecto_dir.join("AURORA_AGENT_CONTEXT.md"),
        "# Contexto comprometido\nTOKEN_FILTRADO=ghp_1234567890abcdefghijklmnopqrstuvwx\n",
    )?;
    let subdirector = SubdirectorProyectosBob::new(
        Arc::clone(&consejo),
        ConfigContextoAislado::por_defecto(&proyecto_dir, &sandbox_root),
    );
    let err_secreto = subdirector
        .ejecutar_mision(
            SolicitudMisionProyecto {
                titulo: "Revisión arquitectura".to_string(),
                objetivo: "Optimizar módulos de Aurora.".to_string(),
                vertical: None,
                tracks: vec![RolEspecialista::GuionesYCopy],
                incluir_contexto_proyecto: true,
                prospecto_ids: Vec::new(),
                ejecutar_forge_en_codigo: false,
                higgsfield_pagado: false,
                timeout_ms: Some(3_000),
            },
            &[],
            &[],
        )
        .await;
    assert!(
        matches!(err_secreto, Err(CouncilError::FalloSanitizacionPii(_))),
        "Se esperaba aborto inmediato por secreto ghp_, obtenido: {err_secreto:?}"
    );
    assert!(mock_claude.solicitudes_recibidas().is_empty());
    assert!(mock_antigravity.solicitudes_recibidas().is_empty());

    // Caso 2: Documento limpio pero objetivo contiene correo real de prospecto activo.
    fs::write(
        proyecto_dir.join("AURORA_AGENT_CONTEXT.md"),
        "# Aurora v2\nDocumento limpio sin secretos ni PII.\n",
    )?;
    let prospecto = prospecto_activo_mx();
    let err_pii = subdirector
        .ejecutar_mision(
            SolicitudMisionProyecto {
                titulo: "Propuesta directa".to_string(),
                objetivo: "Escribir propuesta para Valeria Salgado Montes al correo valeria.salgado@salgado-corp.mx."
                    .to_string(),
                vertical: None,
                tracks: vec![RolEspecialista::GuionesYCopy],
                incluir_contexto_proyecto: true,
                prospecto_ids: Vec::new(),
                ejecutar_forge_en_codigo: false,
                higgsfield_pagado: false,
                timeout_ms: Some(3_000),
            },
            &[],
            &[prospecto],
        )
        .await;
    assert!(
        matches!(err_pii, Err(CouncilError::FalloSanitizacionPii(_))),
        "Se esperaba aborto inmediato por correo/nombre PII, obtenido: {err_pii:?}"
    );
    assert!(mock_claude.solicitudes_recibidas().is_empty());
    assert!(mock_antigravity.solicitudes_recibidas().is_empty());

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn aislamiento_staging_excluye_archivos_prohibidos_y_limpia_al_terminar(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_subdirector_isolation");
    let ruta_cuotas = tmp.join("quota_state.json");
    let proyecto_dir = tmp.join("repo");
    let sandbox_root = tmp.join("sandboxes");
    fs::create_dir_all(proyecto_dir.join("docs"))?;
    fs::create_dir_all(proyecto_dir.join("wal"))?;
    fs::create_dir_all(proyecto_dir.join("data"))?;

    // Archivos permitidos (lista blanca).
    fs::write(
        proyecto_dir.join("AURORA_AGENT_CONTEXT.md"),
        "# Aurora v2\nResumen limpio de contexto.\n",
    )?;
    fs::write(
        proyecto_dir.join("docs/ARQUITECTURA.md"),
        "# Arquitectura\nMicro-crates en Rust.\n",
    )?;
    // Archivos prohibidos (lista negra absoluta), incluso dentro de docs/.
    fs::write(proyecto_dir.join("aurora.toml"), "smtp_pass = \"secreto\"\n")?;
    fs::write(proyecto_dir.join(".env"), "AURORA_MOBILE_TOKEN=supersecreto\n")?;
    fs::write(proyecto_dir.join("wal/wal.log"), "datos_wal_confidenciales")?;
    fs::write(proyecto_dir.join("state.bin"), [0_u8, 1, 2, 3])?;
    fs::write(proyecto_dir.join("catalogacion_compra.json"), "{}")?;
    fs::write(proyecto_dir.join("docs/aurora.toml"), "no_debe_copiarse")?;
    fs::write(proyecto_dir.join("docs/wal.log"), "no_debe_copiarse")?;
    fs::write(proyecto_dir.join("docs/private.key"), "no_debe_copiarse")?;
    fs::write(proyecto_dir.join("data/clientes.json"), "no_debe_copiarse")?;

    let governor = Arc::new(QuotaGovernor::abrir(ConfigCuotasConsejo::con_ruta(
        &ruta_cuotas,
    ))?);
    let spy_claude = Arc::new(SpySandboxDriver::new(MiembroConsejo::ClaudePro));
    let consejo = Arc::new(OrquestadorConsejo::new(governor).con_driver(spy_claude.clone()));

    let subdirector = SubdirectorProyectosBob::new(
        consejo,
        ConfigContextoAislado::por_defecto(&proyecto_dir, &sandbox_root),
    );

    let reporte = subdirector
        .ejecutar_mision(
            SolicitudMisionProyecto {
                titulo: "Auditoría de arquitectura".to_string(),
                objetivo: "Revisar documentación de crates para monetización orgánica.".to_string(),
                vertical: None,
                tracks: vec![RolEspecialista::GuionesYCopy],
                incluir_contexto_proyecto: true,
                prospecto_ids: Vec::new(),
                ejecutar_forge_en_codigo: false,
                higgsfield_pagado: false,
                timeout_ms: Some(3_000),
            },
            &[],
            &[],
        )
        .await?;

    assert!(reporte.sandbox_limpiado);
    assert!(reporte.auditoria_privacidad.libre_de_fugas);

    let staging_dir = spy_claude
        .staging_dir()
        .ok_or("el driver espía debió recibir directorio_trabajo del sandbox")?;
    let archivos_raiz = spy_claude.archivos_en_staging();

    assert!(archivos_raiz.iter().any(|n| n == "MISSION_BRIEF.md"));
    assert!(archivos_raiz.iter().any(|n| n == "AURORA_AGENT_CONTEXT.md"));
    for prohibido in [
        "aurora.toml",
        ".env",
        "wal",
        "wal.log",
        "state.bin",
        "catalogacion_compra.json",
        "data",
    ] {
        assert!(
            !archivos_raiz.iter().any(|n| n == prohibido),
            "Archivo prohibido '{prohibido}' encontrado en staging: {archivos_raiz:?}"
        );
    }

    // Al finalizar la misión, el directorio temporal de staging DEBE estar borrado.
    assert!(
        !staging_dir.exists(),
        "El directorio efímero de staging '{}' no fue eliminado al cerrar la misión",
        staging_dir.display()
    );

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}
