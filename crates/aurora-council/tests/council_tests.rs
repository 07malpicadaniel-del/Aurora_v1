use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use aurora_cortex::DeterministicMockBob;
use aurora_council::{
    BobLocalFallbackDriver, ConfigCuotasConsejo, CouncilError, DeterministicMockCouncilDriver,
    MiembroConsejo, OrquestadorConsejo, PeticionDelegacion, PiiFirewall, PoliticaCuotaProveedor,
    QuotaGovernor, TipoTareaConsejo,
};
use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    TipoGatilloVital, Ubicacion,
};
use chrono::Utc;
use uuid::Uuid;

fn directorio_temporal(prefijo: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn prospecto_ejemplo_mx() -> Prospecto {
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Carlos Alberto Mendoza Ríos".to_string(),
        aliases: vec![
            "Grupo Logístico Mendoza S.A. de C.V.".to_string(),
            "carlos.mendoza@logmendoza.mx".to_string(),
        ],
        ubicacion: Ubicacion {
            ciudad: "Monterrey".to_string(),
            estado: "Nuevo León".to_string(),
            zona: Some("San Pedro Garza García".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 18_500_000,
            patrimonio_estimado_mxn: Some(65_000_000),
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
            descripcion: "Apertura de centro de distribución en Apodaca".to_string(),
            ocurrido_en: Utc::now(),
            peso_urgencia: 0.92,
        }],
    }
}

#[tokio::test]
async fn cascada_completa_claude_agotado_codex_429_gemini_y_degradacion_bob_local(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_council_cascade");
    let ruta_cuotas = tmp.join("quota_state.json");

    let config = ConfigCuotasConsejo::con_ruta(&ruta_cuotas)
        .con_politica(
            MiembroConsejo::ClaudePro,
            PoliticaCuotaProveedor {
                max_tareas_ventana_5h: 1,
                max_tareas_diarias: 2,
                cooldown_rate_limit_segundos: 1_800,
                ilimitado: false,
            },
        )
        .con_politica(
            MiembroConsejo::CodexPlus,
            PoliticaCuotaProveedor {
                max_tareas_ventana_5h: 3,
                max_tareas_diarias: 10,
                cooldown_rate_limit_segundos: 1_800,
                ilimitado: false,
            },
        )
        .con_politica(
            MiembroConsejo::GeminiEstudiante,
            PoliticaCuotaProveedor {
                max_tareas_ventana_5h: 1,
                max_tareas_diarias: 5,
                cooldown_rate_limit_segundos: 900,
                ilimitado: false,
            },
        );

    let governor = Arc::new(QuotaGovernor::abrir(config)?);
    // Agotamos intencionalmente la cuota de ventana 5h de ClaudePro (1/1).
    governor.registrar_uso(MiembroConsejo::ClaudePro).await?;
    assert!(!governor.puede_ejecutar(MiembroConsejo::ClaudePro).await);

    let mock_claude = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::ClaudePro,
        "No debería invocarse porque la cuota está agotada",
    ));
    let mock_codex = Arc::new(DeterministicMockCouncilDriver::rate_limit_429(
        MiembroConsejo::CodexPlus,
        "HTTP 429 Too Many Requests: 5-hour usage limit reached",
    ));
    let mock_gemini = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::GeminiEstudiante,
        "Propuesta consultiva de alto ticket lista para revisión Gatekeeper.",
    ));
    let mock_opencode = Arc::new(DeterministicMockCouncilDriver::no_disponible(
        MiembroConsejo::OpenCodeLocal,
    ));
    let bob_driver = Arc::new(BobLocalFallbackDriver::new(DeterministicMockBob));

    let orquestador = OrquestadorConsejo::new(Arc::clone(&governor))
        .con_driver(mock_claude.clone())
        .con_driver(mock_codex.clone())
        .con_driver(mock_gemini.clone())
        .con_driver(mock_opencode)
        .con_driver(bob_driver);

    // 1. Primera delegación: ClaudePro omitido por cuota, CodexPlus responde 429 -> GeminiEstudiante atiende.
    let res1 = orquestador
        .delegar(PeticionDelegacion {
            tarea: TipoTareaConsejo::CopyAltoTicket,
            contexto: "Diseña propuesta patrimonial para nave industrial en Monterrey.".to_string(),
            prospecto: None,
            forzar_local: false,
            directorio_trabajo: None,
            timeout_ms: Some(5_000),
        })
        .await?;

    assert_eq!(res1.miembro_respondedor, MiembroConsejo::GeminiEstudiante);
    assert!(res1.hubo_fallback);
    assert!(mock_claude.solicitudes_recibidas().is_empty());
    assert_eq!(mock_codex.solicitudes_recibidas().len(), 1);
    assert_eq!(mock_gemini.solicitudes_recibidas().len(), 1);
    assert!(!governor.puede_ejecutar(MiembroConsejo::CodexPlus).await);
    assert!(!governor.puede_ejecutar(MiembroConsejo::GeminiEstudiante).await);

    // 2. Segunda delegación: Claude agotado, Codex en cooldown 429, Gemini agotado, OpenCode no disponible -> BOB Local.
    let res2 = orquestador
        .delegar(PeticionDelegacion {
            tarea: TipoTareaConsejo::RespuestaObjecionCompleja,
            contexto: "Cliente pregunta por deducibilidad fiscal de seguro hombre clave."
                .to_string(),
            prospecto: None,
            forzar_local: false,
            directorio_trabajo: None,
            timeout_ms: Some(5_000),
        })
        .await?;

    assert_eq!(res2.miembro_respondedor, MiembroConsejo::BobLocal);
    assert!(res2.hubo_fallback);
    assert!(res2.consumo_cuota.ilimitado);
    assert!(!res2.contenido.trim().is_empty());

    // 3. Tercera delegación consecutiva: BOB Local nunca se agota.
    let res3 = orquestador
        .delegar(PeticionDelegacion {
            tarea: TipoTareaConsejo::AuditoriaLoteDpo,
            contexto: "Audita rendimiento de ganchos de la semana.".to_string(),
            prospecto: None,
            forzar_local: true,
            directorio_trabajo: None,
            timeout_ms: Some(5_000),
        })
        .await?;

    assert_eq!(res3.miembro_respondedor, MiembroConsejo::BobLocal);
    assert!(!res3.hubo_fallback);
    assert!(governor.puede_ejecutar(MiembroConsejo::BobLocal).await);

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn persistencia_nvme_quota_governor_conserva_contadores_y_cooldowns(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_council_nvme");
    let ruta_cuotas = tmp.join("nvme/quota_state.json");

    let construir_cfg = || {
        ConfigCuotasConsejo::con_ruta(&ruta_cuotas)
            .con_politica(
                MiembroConsejo::ClaudePro,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: 2,
                    max_tareas_diarias: 5,
                    cooldown_rate_limit_segundos: 1_800,
                    ilimitado: false,
                },
            )
            .con_politica(
                MiembroConsejo::CodexPlus,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: 4,
                    max_tareas_diarias: 12,
                    cooldown_rate_limit_segundos: 1_800,
                    ilimitado: false,
                },
            )
    };

    {
        let gov = QuotaGovernor::abrir(construir_cfg())?;
        gov.registrar_uso(MiembroConsejo::ClaudePro).await?;
        gov.registrar_uso(MiembroConsejo::ClaudePro).await?;
        gov.registrar_rate_limit(MiembroConsejo::CodexPlus, "HTTP 429 test")
            .await?;
        gov.registrar_uso(MiembroConsejo::GeminiEstudiante).await?;
    }

    assert!(ruta_cuotas.is_file());

    // Reinstanciamos desde disco NVMe y verificamos persistencia exacta.
    let gov_recargado = QuotaGovernor::abrir(construir_cfg())?;
    let estado_claude = gov_recargado
        .estado_miembro(MiembroConsejo::ClaudePro)
        .await;
    let estado_codex = gov_recargado
        .estado_miembro(MiembroConsejo::CodexPlus)
        .await;
    let estado_gemini = gov_recargado
        .estado_miembro(MiembroConsejo::GeminiEstudiante)
        .await;

    assert_eq!(estado_claude.usado_ventana_5h, 2);
    assert_eq!(estado_claude.usado_diario, 2);
    assert!(!estado_claude.disponible);

    assert!(!estado_codex.disponible);
    assert!(estado_codex.cooldown_hasta.is_some());

    assert_eq!(estado_gemini.usado_ventana_5h, 1);
    assert!(estado_gemini.disponible);

    let cascada = gov_recargado.orden_cascada_vigente().await;
    assert_eq!(
        cascada,
        vec![
            MiembroConsejo::GeminiEstudiante,
            MiembroConsejo::OpenCodeLocal,
            MiembroConsejo::BobLocal,
        ]
    );

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn pii_firewall_bloquea_fugas_a_nube_y_rehidrata_localmente(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = directorio_temporal("aurora_council_pii");
    let ruta_cuotas = tmp.join("quota_state.json");
    let prospecto = prospecto_ejemplo_mx();

    let contexto_con_pii = concat!(
        "Redacta un mensaje ejecutivo para Carlos Alberto Mendoza Ríos, ",
        "director en Grupo Logístico Mendoza S.A. de C.V. ",
        "Su correo es carlos.mendoza@logmendoza.mx, su WhatsApp es +52 81 8345 6789 ",
        "(oficina 81-2345-6789), RFC MERC820514AB9 y oficinas en ",
        "Av. Vasconcelos No. 450, Col. Del Valle, C.P. 66220. ",
        "Menciona a Mendoza el beneficio fiscal de expansión logística."
    );

    // Verificación directa de que el validador rechaza texto sin sanitizar.
    let err_crudo = PiiFirewall::verificar_libre_de_pii(contexto_con_pii, &prospecto);
    assert!(matches!(err_crudo, Err(CouncilError::FalloSanitizacionPii(_))));

    let governor = Arc::new(QuotaGovernor::abrir(ConfigCuotasConsejo::con_ruta(
        &ruta_cuotas,
    ))?);

    let mock_claude = Arc::new(DeterministicMockCouncilDriver::exito(
        MiembroConsejo::ClaudePro,
        "Estimado [PROSPECTO_01] ([EMPRESA_01]), compartimos análisis fiscal a [CORREO_01] y confirmamos llamada al [TEL_01] respecto a [DIRECCION_01].",
    ));
    let bob_driver = Arc::new(BobLocalFallbackDriver::new(DeterministicMockBob));

    let orquestador = OrquestadorConsejo::new(governor)
        .con_driver(mock_claude.clone())
        .con_driver(bob_driver);

    let resultado = orquestador
        .delegar(PeticionDelegacion {
            tarea: TipoTareaConsejo::CopyAltoTicket,
            contexto: contexto_con_pii.to_string(),
            prospecto: Some(prospecto.clone()),
            forzar_local: false,
            directorio_trabajo: None,
            timeout_ms: Some(5_000),
        })
        .await?;

    assert_eq!(resultado.miembro_respondedor, MiembroConsejo::ClaudePro);
    assert!(resultado.pii_sanitizado);
    assert!(resultado.marcadores_pii_aplicados >= 5);

    // Aserción estricta sobre lo que exactamente salió hacia el agente externo.
    let recibidas = mock_claude.solicitudes_recibidas();
    assert_eq!(recibidas.len(), 1);
    let prompt_enviado = &recibidas[0].prompt_usuario;

    for prohibido in [
        "Carlos Alberto Mendoza Ríos",
        "Carlos",
        "Mendoza",
        "Ríos",
        "Grupo Logístico Mendoza S.A. de C.V.",
        "carlos.mendoza@logmendoza.mx",
        "+52 81 8345 6789",
        "8345 6789",
        "81-2345-6789",
        "MERC820514AB9",
        "Av. Vasconcelos No. 450",
    ] {
        assert!(
            !prompt_enviado.contains(prohibido),
            "Fuga PII detectada en prompt enviado al agente externo: '{prohibido}' dentro de:\n{prompt_enviado}"
        );
    }

    assert!(prompt_enviado.contains("[PROSPECTO_01]"));
    assert!(prompt_enviado.contains("[EMPRESA_01]"));
    assert!(prompt_enviado.contains("[CORREO_01]"));
    assert!(prompt_enviado.contains("[TEL_01]"));
    PiiFirewall::verificar_libre_de_pii(prompt_enviado, &prospecto)?;

    // Aserción de rehidratación isomórfica local en el resultado devuelto al caller.
    assert!(resultado.contenido.contains("Carlos Alberto Mendoza Ríos"));
    assert!(resultado
        .contenido
        .contains("Grupo Logístico Mendoza S.A. de C.V."));
    assert!(resultado.contenido.contains("carlos.mendoza@logmendoza.mx"));
    assert!(resultado.contenido.contains("+52 81 8345 6789"));
    assert!(resultado.contenido.contains("Av. Vasconcelos No. 450"));
    assert!(!resultado.contenido.contains("[PROSPECTO_01]"));
    assert!(!resultado.contenido.contains("[CORREO_01]"));
    assert!(!resultado.contenido.contains("[TEL_01]"));

    // Regresión UAT: teléfono MX seguido de coma en prosa (", ") también se sanitiza.
    let (san_coma, mapa_coma) = PiiFirewall::sanitizar_texto_generico(
        "Contacto contacto.uat@ejemplo.mx, tel +52 229 555 0199, RFC XAXX010101000 en Veracruz.",
    );
    assert_eq!(mapa_coma.len(), 3);
    assert!(!san_coma.contains("229 555 0199"));
    PiiFirewall::verificar_texto_sin_pii_generica(&san_coma)?;

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}
