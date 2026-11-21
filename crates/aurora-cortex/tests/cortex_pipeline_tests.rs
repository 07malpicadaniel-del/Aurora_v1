use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::{
    gbnf_ajuste_auditor, gbnf_brief_creativo, gbnf_diagnostico_comercial, gbnf_tool_call,
    parsear_respuesta_estricta, validar_sintaxis_gbnf, AuroraToolCall, BobInferenceBackend,
    CanalContacto, ConfigHttpBob, CortexError, DeterministicMockBob, DiagnosticoComercial,
    HttpBobCortexClient, MetricasCampana, OrquestadorComercialBob, PlataformaCalentamiento,
    ResultadoEjecucionTool, RolCognitivo, TipoInteraccionPeriferica, ToolDispatcher,
};
use aurora_domain::{
    CapacidadAdquisitiva, EstadoAprobacion, GatilloVital, MetadatosLfpdppp, PerfilPsicologico,
    Prospecto, SubverticalInmobiliaria, TipoActivoGenerado, TipoGatilloVital, Ubicacion,
    VerticalComercial,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConsultaHibrida, DeterministicSimdEmbedder, IslaVertical,
    MotorMemoriaAurora, MotorRecuperacion,
};
use aurora_osint::{
    CoMencionCruda, ConsultaObjetivo, DestiladorOsint, EnjambreOsint, EntityResolver, FormatoCrudo,
    OsintResult, OsintSource, PipelineOsint, RegistroCrudo,
};
use chrono::Utc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

struct FuenteOsintSimulada {
    nombre: String,
    registros: Vec<RegistroCrudo>,
}

#[async_trait]
impl OsintSource for FuenteOsintSimulada {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    async fn recolectar(&self, _consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        Ok(self.registros.clone())
    }
}

fn construir_prospecto_prueba(
    nombre: &str,
    perfil: PerfilPsicologico,
    ciudad: &str,
    estado: &str,
    descripcion_gatillo: &str,
) -> Prospecto {
    let ahora = Utc::now();
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: nombre.to_string(),
        aliases: vec![format!("Consejo Empresarial {ciudad}")],
        ubicacion: Ubicacion {
            ciudad: ciudad.to_string(),
            estado: estado.to_string(),
            zona: Some("Corredor Industrial Norte".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 6_500_000,
            patrimonio_estimado_mxn: Some(38_000_000),
        },
        perfil_psicologico: perfil,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "Boletin Empresarial Publico".to_string(),
            url_evidencia: "https://ejemplo.gob.mx/directorio/registro-001".to_string(),
            capturado_en: ahora,
            opt_out: false,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::ExpansionLogistica,
            descripcion: descripcion_gatillo.to_string(),
            ocurrido_en: ahora,
            peso_urgencia: 0.92,
        }],
    }
}

#[tokio::test]
async fn pipeline_completo_osint_a_memoria_y_plan_ataque_comercial(
) -> Result<(), Box<dyn std::error::Error>> {
    let ahora = Utc::now();
    let ubicacion = Ubicacion {
        ciudad: "Monterrey".to_string(),
        estado: "Nuevo León".to_string(),
        zona: Some("Apodaca Industrial".to_string()),
    };

    let registros = vec![
        RegistroCrudo {
            fuente_origen: "Registro Publico Comercio NL".to_string(),
            url_evidencia: "https://rpc.ejemplo.mx/nl/folio-901".to_string(),
            capturado_en: ahora,
            formato: FormatoCrudo::TextoPlano,
            contenido_crudo: "Carlos Mendoza Garza anuncia inauguracion de nueva bodega y expansion logistica con TIR proyectada en Apodaca Monterrey".to_string(),
            nombre_detectado: Some("Carlos Mendoza Garza".to_string()),
            empresa_detectada: Some("Logistica Regia SA".to_string()),
            ubicacion_detectada: Some(ubicacion.clone()),
            co_mencionados: vec![CoMencionCruda {
                nombre: "Sofia Villarreal".to_string(),
                contexto: "Socio comercial Consejo Industrial Apodaca".to_string(),
                empresa: Some("Logistica Regia SA".to_string()),
            }],
            datos_extra: BTreeMap::new(),
            opt_out_detectado: false,
        },
        RegistroCrudo {
            fuente_origen: "Registro Publico Comercio NL".to_string(),
            url_evidencia: "https://rpc.ejemplo.mx/nl/folio-902".to_string(),
            capturado_en: ahora,
            formato: FormatoCrudo::TextoPlano,
            contenido_crudo: "Sofia Villarreal lidera ampliacion de flotilla operativa en parque industrial Monterrey".to_string(),
            nombre_detectado: Some("Sofia Villarreal".to_string()),
            empresa_detectada: Some("Logistica Regia SA".to_string()),
            ubicacion_detectada: Some(ubicacion.clone()),
            co_mencionados: Vec::new(),
            datos_extra: BTreeMap::new(),
            opt_out_detectado: false,
        },
    ];

    let mut enjambre = EnjambreOsint::new(2);
    enjambre.registrar_fuente(Arc::new(FuenteOsintSimulada {
        nombre: "FuenteSimuladaNL".to_string(),
        registros,
    }));

    let pipeline_osint = PipelineOsint::new(
        enjambre,
        DestiladorOsint::new(),
        EntityResolver::default(),
    );

    let consulta_osint = ConsultaObjetivo {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        ubicacion,
        palabras_clave: vec!["bodega".to_string(), "logistica".to_string()],
        limite_resultados: 10,
        requiere_js: false,
    };

    let ingesta = pipeline_osint.ejecutar(&consulta_osint).await?;
    assert!(!ingesta.prospectos.is_empty());

    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        ingesta.prospectos,
        ingesta.relaciones,
        IslaVertical::InmobiliariaComercialIndustrial,
    )?;

    let recuperacion = MotorRecuperacion::default();
    let consulta_hibrida = ConsultaHibrida::nueva(
        "expansion logistica bodega industrial Monterrey",
        Some(IslaVertical::InmobiliariaComercialIndustrial),
    );
    let subgrafos = recuperacion.consultar(&memoria, &consulta_hibrida)?;
    let mejor_subgrafo = subgrafos
        .first()
        .ok_or("debería recuperarse al menos un subgrafo desde aurora-memory")?;

    let campana_id = Uuid::new_v4();
    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let plan = orquestador
        .generar_plan_ataque(mejor_subgrafo, campana_id, &memoria)
        .await?;

    assert_eq!(plan.prospecto_id, mejor_subgrafo.prospecto.id);
    assert_eq!(plan.campana_id, campana_id);
    assert!(!plan.diagnostico.producto_gancho.is_empty());
    assert!((0.0..=1.0).contains(&plan.diagnostico.probabilidad_cierre));
    assert!(!plan.brief_creativo.escenas_reel.is_empty());
    assert!(!plan.brief_creativo.secciones_dossier_pdf.is_empty());
    assert_eq!(
        plan.orden_produccion.estado_gatekeeper(),
        EstadoAprobacion::PendienteGatekeeper
    );
    assert_eq!(plan.orden_produccion.activos_iniciales().len(), 4);
    for activo in plan.orden_produccion.activos_iniciales() {
        assert_eq!(activo.estado, EstadoAprobacion::PendienteGatekeeper);
    }

    Ok(())
}

#[tokio::test]
async fn adaptacion_psicologica_analitico_vs_dinamico_visual(
) -> Result<(), Box<dyn std::error::Error>> {
    let p_analitico = construir_prospecto_prueba(
        "Ricardo Dominguez Leal",
        PerfilPsicologico::AnaliticoFinanciero,
        "Querétaro",
        "Querétaro",
        "Apertura de nave logística con evaluación de flujo libre y retorno",
    );
    let p_visual = construir_prospecto_prueba(
        "Valeria Castellanos Rios",
        PerfilPsicologico::DinamicoVisual,
        "Querétaro",
        "Querétaro",
        "Apertura de centro de distribución y expansión comercial rápida",
    );

    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        vec![p_analitico.clone(), p_visual.clone()],
        Vec::new(),
        IslaVertical::InmobiliariaComercialIndustrial,
    )?;

    let dispatcher = ToolDispatcher;
    let campana_id = Uuid::new_v4();

    let sub_analitico = match dispatcher.ejecutar(
        &AuroraToolCall::ConsultarSubgrafo4D {
            prospecto_id: p_analitico.id,
            saltos_max: 1,
        },
        campana_id,
        &memoria,
    )? {
        ResultadoEjecucionTool::SubgrafoObtenido(sg) => *sg,
        _ => return Err("se esperaba SubgrafoObtenido".into()),
    };

    let sub_visual = match dispatcher.ejecutar(
        &AuroraToolCall::ConsultarSubgrafo4D {
            prospecto_id: p_visual.id,
            saltos_max: 1,
        },
        campana_id,
        &memoria,
    )? {
        ResultadoEjecucionTool::SubgrafoObtenido(sg) => *sg,
        _ => return Err("se esperaba SubgrafoObtenido".into()),
    };

    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let plan_analitico = orquestador
        .generar_plan_ataque(&sub_analitico, campana_id, &memoria)
        .await?;
    let plan_visual = orquestador
        .generar_plan_ataque(&sub_visual, campana_id, &memoria)
        .await?;

    // Validación psicológica AnaliticoFinanciero
    assert_eq!(
        plan_analitico.brief_creativo.perfil_psicologico,
        PerfilPsicologico::AnaliticoFinanciero
    );
    assert!(plan_analitico.brief_creativo.gancho_3s.contains("ROI"));
    assert!(plan_analitico.brief_creativo.guion_tts.contains("TIR"));
    assert_eq!(
        plan_analitico.brief_creativo.canal_sugerido,
        CanalContacto::CorreoEjecutivo
    );

    // Validación psicológica DinamicoVisual
    assert_eq!(
        plan_visual.brief_creativo.perfil_psicologico,
        PerfilPsicologico::DinamicoVisual
    );
    assert!(plan_visual
        .brief_creativo
        .gancho_3s
        .to_lowercase()
        .contains("cortes rápidos"));
    assert_eq!(
        plan_visual.brief_creativo.canal_sugerido,
        CanalContacto::WhatsApp
    );
    assert!(plan_visual
        .brief_creativo
        .escenas_reel
        .iter()
        .all(|e| e.duracion_segundos <= 3));

    Ok(())
}

#[tokio::test]
async fn validacion_estricta_gramaticas_gbnf_y_rechazo_alucinaciones(
) -> Result<(), Box<dyn std::error::Error>> {
    for gramatica in [
        gbnf_diagnostico_comercial(),
        gbnf_brief_creativo(),
        gbnf_tool_call(),
        gbnf_ajuste_auditor(),
    ] {
        validar_sintaxis_gbnf(&gramatica)?;
    }

    // 1. JSON truncado
    let err_truncado = parsear_respuesta_estricta::<DiagnosticoComercial>(
        "{\"prospecto_id\":\"",
        RolCognitivo::Perfilador,
    );
    assert!(matches!(err_truncado, Err(CortexError::ViolacionEsquema { .. })));

    // 2. Prosa libre alrededor del JSON
    let id = Uuid::new_v4();
    let con_prosa = format!(
        "Claro, aqui tienes el JSON:\n{{\"prospecto_id\":\"{id}\",\"vertical_optima\":{{\"Inmobiliaria\":\"Preventa\"}},\"producto_gancho\":\"Preventa Plus\",\"dolor_principal\":\"Inflacion\",\"angulo_entrada\":\"TIR\",\"probabilidad_cierre\":0.8,\"justificacion_topologica\":\"Reciente\"}}"
    );
    let err_prosa =
        parsear_respuesta_estricta::<DiagnosticoComercial>(&con_prosa, RolCognitivo::Perfilador);
    assert!(matches!(err_prosa, Err(CortexError::ViolacionEsquema { .. })));

    // 3. Campo extra alucinado (deny_unknown_fields)
    let con_extra = format!(
        "{{\"prospecto_id\":\"{id}\",\"vertical_optima\":{{\"Inmobiliaria\":\"Preventa\"}},\"producto_gancho\":\"Preventa Plus\",\"dolor_principal\":\"Inflacion\",\"angulo_entrada\":\"TIR\",\"probabilidad_cierre\":0.8,\"justificacion_topologica\":\"Reciente\",\"campo_extra\":123}}"
    );
    let err_extra =
        parsear_respuesta_estricta::<DiagnosticoComercial>(&con_extra, RolCognitivo::Perfilador);
    assert!(matches!(err_extra, Err(CortexError::ViolacionEsquema { .. })));

    // 4. Probabilidad fuera de rango [0.0, 1.0]
    let prob_invalida = format!(
        "{{\"prospecto_id\":\"{id}\",\"vertical_optima\":{{\"Seguros\":\"HombreClave\"}},\"producto_gancho\":\"Hombre Clave\",\"dolor_principal\":\"Riesgo fiscal\",\"angulo_entrada\":\"Art 27 LISR\",\"probabilidad_cierre\":1.45,\"justificacion_topologica\":\"Socio reciente\"}}"
    );
    let err_prob = parsear_respuesta_estricta::<DiagnosticoComercial>(
        &prob_invalida,
        RolCognitivo::Perfilador,
    );
    assert!(matches!(err_prob, Err(CortexError::ViolacionEsquema { .. })));

    // 5. Llamado de herramienta con saltos fuera de [1, 2]
    let tool_saltos_invalidos = format!(
        "{{\"tool\":\"ConsultarSubgrafo4D\",\"params\":{{\"prospecto_id\":\"{id}\",\"saltos_max\":5}}}}"
    );
    let err_saltos = parsear_respuesta_estricta::<AuroraToolCall>(
        &tool_saltos_invalidos,
        RolCognitivo::Perfilador,
    );
    assert!(matches!(err_saltos, Err(CortexError::ViolacionEsquema { .. })));

    // 6. Auditoria determinista acotada en [0.5, 1.5]
    let campana_id = Uuid::new_v4();
    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let ajuste = orquestador
        .auditar_campana(&MetricasCampana {
            campana_id,
            tasa_apertura: 0.08,
            tasa_retencion_3s: 0.21,
            tasa_respuesta: 0.02,
            ganchos_bajo_rendimiento: vec!["Gancho genérico".to_string()],
        })
        .await?;
    assert_eq!(ajuste.campana_id, campana_id);
    assert!((0.5..=1.5).contains(&ajuste.factor_ajuste_urgencia));
    assert!(ajuste.requiere_pausa_vertical);

    Ok(())
}

#[tokio::test]
async fn barrera_gatekeeper_y_bloqueo_opt_out_lfpdppp(
) -> Result<(), Box<dyn std::error::Error>> {
    let prospecto = construir_prospecto_prueba(
        "Eduardo Trevino Garza",
        PerfilPsicologico::ConservadorPatrimonial,
        "San Pedro Garza García",
        "Nuevo León",
        "Adquisición de inmueble corporativo familiar",
    );

    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        vec![prospecto.clone()],
        Vec::new(),
        IslaVertical::SegurosPatrimonial,
    )?;

    let campana_id = Uuid::new_v4();
    let dispatcher = ToolDispatcher;

    // 1. Calentamiento periférico válido siempre nace en PendienteGatekeeper
    let res_cal = dispatcher.ejecutar(
        &AuroraToolCall::ProgramarCalentamientoPeriferico {
            prospecto_id: prospecto.id,
            plataforma: PlataformaCalentamiento::LinkedIn,
            tipo_interaccion: TipoInteraccionPeriferica::ComentarioValor,
            contexto_publico: "Publicacion en Canaco Nuevo Leon sobre sucesion patrimonial"
                .to_string(),
        },
        campana_id,
        &memoria,
    )?;
    match res_cal {
        ResultadoEjecucionTool::CalentamientoProgramado(cal) => {
            assert_eq!(cal.estado_gatekeeper(), EstadoAprobacion::PendienteGatekeeper);
        }
        _ => return Err("se esperaba CalentamientoProgramado".into()),
    }

    // Extraemos subgrafo válido antes de purga para probar barrera del orquestador y dispatcher
    let subgrafo = match dispatcher.ejecutar(
        &AuroraToolCall::ConsultarSubgrafo4D {
            prospecto_id: prospecto.id,
            saltos_max: 1,
        },
        campana_id,
        &memoria,
    )? {
        ResultadoEjecucionTool::SubgrafoObtenido(sg) => *sg,
        _ => return Err("se esperaba SubgrafoObtenido".into()),
    };

    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let plan_valido = orquestador
        .generar_plan_ataque(&subgrafo, campana_id, &memoria)
        .await?;
    assert_eq!(
        plan_valido.orden_produccion.estado_gatekeeper(),
        EstadoAprobacion::PendienteGatekeeper
    );
    let tipos: Vec<TipoActivoGenerado> = plan_valido
        .orden_produccion
        .activos_iniciales()
        .iter()
        .map(|a| a.tipo)
        .collect();
    assert!(tipos.contains(&TipoActivoGenerado::DossierPdf));
    assert!(tipos.contains(&TipoActivoGenerado::NotaVozTts));
    assert!(tipos.contains(&TipoActivoGenerado::ReelVerticalMp4));
    assert!(tipos.contains(&TipoActivoGenerado::MensajeDirecto));

    // 2. Purga física LFPDPPP opt_out bloquea toda generación futura inmediatamente
    memoria.purgar_prospecto_opt_out(prospecto.id)?;

    let err_plan = orquestador
        .generar_plan_ataque(&subgrafo, campana_id, &memoria)
        .await;
    assert!(matches!(
        err_plan,
        Err(CortexError::BloqueoGatekeeper { .. })
    ));

    let err_tool = dispatcher.ejecutar(
        &AuroraToolCall::ProgramarCalentamientoPeriferico {
            prospecto_id: prospecto.id,
            plataforma: PlataformaCalentamiento::CamaraEmpresarial,
            tipo_interaccion: TipoInteraccionPeriferica::InvitacionEvento,
            contexto_publico: "Foro empresarial".to_string(),
        },
        campana_id,
        &memoria,
    );
    assert!(matches!(
        err_tool,
        Err(CortexError::BloqueoGatekeeper { .. })
    ));

    Ok(())
}

#[tokio::test]
async fn cliente_http_bob_envia_gramatica_gbnf_y_slot_unico_local(
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let prospecto_id = Uuid::new_v4();

    let server_handle = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return Err("fallo al aceptar conexion TCP local".to_string());
        };

        let mut buffer = Vec::with_capacity(4096);
        let mut chunk = [0_u8; 1024];
        let header_end = loop {
            let n = socket
                .read(&mut chunk)
                .await
                .map_err(|e| format!("lectura HTTP fallida: {e}"))?;
            if n == 0 {
                return Err("socket cerrado antes de finalizar cabeceras".to_string());
            }
            buffer.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let headers_str = String::from_utf8_lossy(&buffer[..header_end]).to_string();
        let content_length = headers_str
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);

        while buffer.len() < header_end + content_length {
            let n = socket
                .read(&mut chunk)
                .await
                .map_err(|e| format!("lectura body fallida: {e}"))?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
        }

        let body_slice = &buffer[header_end..(header_end + content_length).min(buffer.len())];
        let req_json: serde_json::Value = serde_json::from_slice(body_slice)
            .map_err(|e| format!("payload JSON invalido hacia mock llama-server: {e}"))?;

        if req_json.get("id_slot").and_then(serde_json::Value::as_i64) != Some(0) {
            return Err("id_slot esperado = 0".to_string());
        }
        let gramatica_recibida = req_json
            .get("grammar")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !gramatica_recibida.contains("root ::=") {
            return Err("el request HTTP a BOB no incluyo gramatica GBNF".to_string());
        }
        let thinking_flag = req_json
            .get("chat_template_kwargs")
            .and_then(|k| k.get("enable_thinking"))
            .and_then(serde_json::Value::as_bool);
        if thinking_flag != Some(false) {
            return Err("chat_template_kwargs.enable_thinking esperado = false".to_string());
        }
        let prompt_enviado = req_json
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !prompt_enviado.contains("/no_think") || !prompt_enviado.ends_with("<think>\n</think>\n")
        {
            return Err("el prompt GBNF no desactivo el bloque de pensamiento interno".to_string());
        }

        let diag_json = serde_json::to_string(&DiagnosticoComercial {
            prospecto_id,
            vertical_optima: VerticalComercial::Inmobiliaria(
                SubverticalInmobiliaria::ComercialIndustrial,
            ),
            producto_gancho: "Nave Industrial Sale & Leaseback".to_string(),
            dolor_principal: "Liquidez operativa durante expansion".to_string(),
            angulo_entrada: "Modelo TIR 14.5% anual".to_string(),
            probabilidad_cierre: 0.84,
            justificacion_topologica: "Expansion confirmada en servidor mock HTTP".to_string(),
        })
        .map_err(|e| format!("serializacion diag: {e}"))?;

        let response_body = serde_json::json!({ "content": diag_json }).to_string();
        let http_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        socket
            .write_all(http_resp.as_bytes())
            .await
            .map_err(|e| format!("escritura HTTP fallida: {e}"))?;
        Ok(())
    });

    let client = HttpBobCortexClient::new(ConfigHttpBob {
        url_base: format!("http://{addr}"),
        slot_id: 0,
        n_predict: 512,
        timeout_ms: 5_000,
        max_reintentos: 0,
        cache_prompt: true,
    })?;

    let gramatica = gbnf_diagnostico_comercial();
    let raw = client
        .inferir_estructurado(
            RolCognitivo::Perfilador,
            &format!("prospecto_id: {prospecto_id}"),
            &gramatica,
        )
        .await?;

    let parsed: DiagnosticoComercial =
        parsear_respuesta_estricta(&raw, RolCognitivo::Perfilador)?;
    assert_eq!(parsed.prospecto_id, prospecto_id);
    assert_eq!(parsed.probabilidad_cierre, 0.84);

    let server_res = server_handle
        .await
        .map_err(|e| format!("join mock server error: {e}"))?;
    if let Err(msg) = server_res {
        return Err(msg.into());
    }

    Ok(())
}
