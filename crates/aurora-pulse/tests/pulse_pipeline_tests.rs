use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::{
    AuroraToolCall, CanalContacto, DeterministicMockBob, OrquestadorComercialBob,
    PlataformaCalentamiento, ResultadoEjecucionTool, TipoInteraccionPeriferica, ToolDispatcher,
};
use aurora_domain::{
    CapacidadAdquisitiva, EstadoAprobacion, GatilloVital, MetadatosLfpdppp, PerfilPsicologico,
    Prospecto, RelacionTopologica, SubverticalInmobiliaria, TipoGatilloVital, TipoVinculo,
    Ubicacion, VerticalComercial,
};
use aurora_factory::{
    ConfigFabrica, DeterministicMockDocumentEngine, DeterministicMockVideoComposer,
    DeterministicSilentWavEngine, FabricaMultimodalAurora, SolicitudManufactura,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConsultaHibrida, DeterministicSimdEmbedder, IslaVertical,
    MotorMemoriaAurora, MotorRecuperacion, TextEmbedder,
};
use aurora_osint::{
    CoMencionCruda, ConsultaObjetivo, DestiladorOsint, EnjambreOsint, EntityResolver, FormatoCrudo,
    OsintResult, OsintSource, PipelineOsint, RegistroCrudo,
};
use aurora_pulse::{
    ConfigCalentamiento, ConfigConsolidacionNocturna, ControladorTasaPlataforma,
    DespachadorMulticanal, DeterministicMockTransport, DeterministicMockWarmupDriver,
    EntradaPreferenciaDpo, EstadoCampanaProspecto, EventoTelemetria, GestorCalentamientoPeriferico,
    MotorConsolidacionNocturna, PlataformaPulso, PoliticaTasaPlataforma, PulseError,
    RecolectorTelemetria, TipoEventoTelemetria,
};
use chrono::{Duration, Utc};
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

fn dir_temporal_prueba(prefijo: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()))
}

fn construir_prospecto_seguros(
    nombre: &str,
    perfil: PerfilPsicologico,
    empresa: &str,
    peso_urgencia: f32,
) -> Prospecto {
    let ahora = Utc::now();
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: nombre.to_string(),
        aliases: vec![empresa.to_string()],
        ubicacion: Ubicacion {
            ciudad: "Ciudad de México".to_string(),
            estado: "CDMX".to_string(),
            zona: Some("Santa Fe".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 7_200_000,
            patrimonio_estimado_mxn: Some(42_000_000),
        },
        perfil_psicologico: perfil,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "Boletín Corporativo Público".to_string(),
            url_evidencia: "https://datos.ejemplo.mx/cdmx/directorio".to_string(),
            capturado_en: ahora,
            opt_out: false,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::CompraActivo,
            descripcion: format!("Estrategia fiscal y blindaje directivo para {empresa}"),
            ocurrido_en: ahora,
            peso_urgencia,
        }],
    }
}

async fn preparar_paquete_aprobado(
    prospecto: &Prospecto,
    memoria: &MotorMemoriaAurora,
    isla: IslaVertical,
    dir_fabrica: PathBuf,
) -> Result<(aurora_cortex::PlanAtaqueComercial, aurora_factory::PaqueteManufacturado), Box<dyn std::error::Error>>
{
    let recuperacion = MotorRecuperacion::default();
    let subgrafos = recuperacion.consultar(
        memoria,
        &ConsultaHibrida::nueva(
            "expansion logistica blindaje patrimonial",
            Some(isla),
        ),
    )?;
    let subgrafo = subgrafos
        .iter()
        .find(|s| s.prospecto.id == prospecto.id)
        .or_else(|| subgrafos.first())
        .ok_or("no se encontró subgrafo para manufactura")?;

    let campana_id = Uuid::new_v4();
    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let mut plan = orquestador
        .generar_plan_ataque(subgrafo, campana_id, memoria)
        .await?;
    plan.orden_produccion.aprobar_gatekeeper()?;

    let fabrica = FabricaMultimodalAurora::new(
        ConfigFabrica {
            directorio_base: dir_fabrica,
        },
        DeterministicSilentWavEngine,
        DeterministicMockDocumentEngine,
        DeterministicMockVideoComposer,
    );

    let solicitud = SolicitudManufactura {
        orden: plan.orden_produccion.clone(),
        prospecto: prospecto.clone(),
        diagnostico: plan.diagnostico.clone(),
        brief: plan.brief_creativo.clone(),
    };
    let paquete = fabrica.manufacturar(&solicitud).await?;
    Ok((plan, paquete))
}

#[tokio::test]
async fn test_barrera_gatekeeper_sha256_cuota_y_circuit_breaker(
) -> Result<(), Box<dyn std::error::Error>> {
    let prospecto = construir_prospecto_seguros(
        "Fernando Ibarra Lozano",
        PerfilPsicologico::AnaliticoFinanciero,
        "Constructora Santa Fe",
        0.90,
    );
    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        vec![prospecto.clone()],
        Vec::new(),
        IslaVertical::SegurosHombreClave,
    )?;

    let campana_id = Uuid::new_v4();
    let llamado_calentamiento = AuroraToolCall::ProgramarCalentamientoPeriferico {
        prospecto_id: prospecto.id,
        plataforma: PlataformaCalentamiento::LinkedIn,
        tipo_interaccion: TipoInteraccionPeriferica::ComentarioValor,
        contexto_publico: "Publicación directiva sobre eficiencia energética en obra".to_string(),
    };
    let ResultadoEjecucionTool::CalentamientoProgramado(calentamiento_pendiente) =
        ToolDispatcher.ejecutar(&llamado_calentamiento, campana_id, &memoria)?
    else {
        return Err("ToolDispatcher no devolvió CalentamientoProgramado".into());
    };

    let rate_limiter = Arc::new(ControladorTasaPlataforma::default());
    let gestor_warmup = GestorCalentamientoPeriferico::new(
        DeterministicMockWarmupDriver::new(),
        Arc::clone(&rate_limiter),
        ConfigCalentamiento::default(),
    );

    let t0 = Utc::now();
    // 1. Calentamiento sin aprobación Gatekeeper debe fallar sin invocar driver.
    let err_warmup = gestor_warmup
        .ejecutar_calentamiento(&calentamiento_pendiente, &memoria, t0)
        .await;
    assert!(matches!(
        err_warmup,
        Err(PulseError::NoAprobadoPorGatekeeper {
            estado_actual: EstadoAprobacion::PendienteGatekeeper,
            ..
        })
    ));
    assert!(gestor_warmup.driver().historial().is_empty());

    // 2. Despacho con paquete alterado o sin EstadoAprobacion::Aprobado debe fallar antes del transporte.
    let dir_fabrica = dir_temporal_prueba("aurora_pulse_gate");
    let (plan, paquete) = preparar_paquete_aprobado(
        &prospecto,
        &memoria,
        IslaVertical::SegurosHombreClave,
        dir_fabrica.clone(),
    )
    .await?;

    let despachador = DespachadorMulticanal::new(
        DeterministicMockTransport::new(),
        Arc::clone(&rate_limiter),
    );

    let mut paquete_pendiente = paquete.clone();
    if let Some(primero) = paquete_pendiente.activos_actualizados.first_mut() {
        primero.estado = EstadoAprobacion::PendienteGatekeeper;
    }
    let err_gate_despacho = despachador
        .despachar(&paquete_pendiente, &prospecto, plan.brief_creativo.canal_sugerido, &memoria, t0)
        .await;
    assert!(matches!(
        err_gate_despacho,
        Err(PulseError::NoAprobadoPorGatekeeper {
            estado_actual: EstadoAprobacion::PendienteGatekeeper,
            ..
        })
    ));
    assert!(despachador.transport().envios_realizados().is_empty());

    // 3. Alteración de bytes en NVMe debe ser detectada por SHA-256 antes de enviar.
    let paquete_corrupto = paquete.clone();
    fs::write(&paquete_corrupto.dossier_pdf.ruta_nvme, b"PDF alterado manualmente")?;
    let err_sha = despachador
        .despachar(
            &paquete_corrupto,
            &prospecto,
            plan.brief_creativo.canal_sugerido,
            &memoria,
            t0,
        )
        .await;
    assert!(matches!(err_sha, Err(PulseError::DespachoFallido { .. })));

    // Restauramos el paquete limpio en un nuevo directorio para pruebas de cuota + circuit breaker.
    let dir_fabrica_2 = dir_temporal_prueba("aurora_pulse_quota");
    let (_plan2, paquete_valido) = preparar_paquete_aprobado(
        &prospecto,
        &memoria,
        IslaVertical::SegurosHombreClave,
        dir_fabrica_2.clone(),
    )
    .await?;

    // 4. Cuota horaria estricta (máximo 2 envíos por hora en CorreoEjecutivo).
    let rl_acotado = Arc::new(ControladorTasaPlataforma::default());
    rl_acotado.configurar_politica(
        PlataformaPulso::CorreoEjecutivo,
        PoliticaTasaPlataforma {
            cuota_max_hora: 2,
            cuota_max_dia: 10,
            intervalo_minimo_ms: 1_000,
            jitter_max_ms: 500,
            umbral_fallos_circuit_breaker: 2,
            enfriamiento_circuit_breaker_s: 600,
        },
    );
    let desp_cuota = DespachadorMulticanal::new(
        DeterministicMockTransport::new(),
        Arc::clone(&rl_acotado),
    );
    desp_cuota
        .despachar(
            &paquete_valido,
            &prospecto,
            CanalContacto::CorreoEjecutivo,
            &memoria,
            t0,
        )
        .await?;
    desp_cuota
        .despachar(
            &paquete_valido,
            &prospecto,
            CanalContacto::CorreoEjecutivo,
            &memoria,
            t0 + Duration::seconds(5),
        )
        .await?;
    let err_cuota = desp_cuota
        .despachar(
            &paquete_valido,
            &prospecto,
            CanalContacto::CorreoEjecutivo,
            &memoria,
            t0 + Duration::seconds(10),
        )
        .await;
    assert!(matches!(
        err_cuota,
        Err(PulseError::RateLimitPlataforma { .. })
    ));

    // 5. Apertura automática de circuit breaker tras 2 fallos consecutivos 429 en WhatsApp.
    let rl_cb = Arc::new(ControladorTasaPlataforma::default());
    rl_cb.configurar_politica(
        PlataformaPulso::WhatsApp,
        PoliticaTasaPlataforma {
            cuota_max_hora: 20,
            cuota_max_dia: 50,
            intervalo_minimo_ms: 1_000,
            jitter_max_ms: 200,
            umbral_fallos_circuit_breaker: 2,
            enfriamiento_circuit_breaker_s: 1_800,
        },
    );
    let transporte_cb = DeterministicMockTransport::new();
    transporte_cb.inyectar_fallos_rate_limit_consecutivos(2);
    let desp_cb = DespachadorMulticanal::new(transporte_cb, Arc::clone(&rl_cb));

    let err_429_1 = desp_cb
        .despachar(&paquete_valido, &prospecto, CanalContacto::WhatsApp, &memoria, t0)
        .await;
    assert!(matches!(err_429_1, Err(PulseError::RateLimitPlataforma { .. })));

    let err_429_2 = desp_cb
        .despachar(
            &paquete_valido,
            &prospecto,
            CanalContacto::WhatsApp,
            &memoria,
            t0 + Duration::seconds(5),
        )
        .await;
    assert!(matches!(err_429_2, Err(PulseError::RateLimitPlataforma { .. })));

    // El 3er intento debe ser rechazado instantáneamente por CircuitBreakerAbierto.
    let err_cb_abierto = desp_cb
        .despachar(
            &paquete_valido,
            &prospecto,
            CanalContacto::WhatsApp,
            &memoria,
            t0 + Duration::seconds(10),
        )
        .await;
    assert!(matches!(
        err_cb_abierto,
        Err(PulseError::CircuitBreakerAbierto {
            fallos_consecutivos: 2,
            ..
        })
    ));

    let _ = fs::remove_dir_all(&dir_fabrica);
    let _ = fs::remove_dir_all(&dir_fabrica_2);
    Ok(())
}

#[tokio::test]
async fn test_pipeline_e2e_osint_a_pulse_calentamiento_y_despacho_verificado(
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
            url_evidencia: "https://rpc.ejemplo.mx/nl/folio-910".to_string(),
            capturado_en: ahora,
            formato: FormatoCrudo::TextoPlano,
            contenido_crudo: "Ricardo Salinas Garza dirige expansion logistica y construccion de nueva bodega AAA en Apodaca Monterrey".to_string(),
            nombre_detectado: Some("Ricardo Salinas Garza".to_string()),
            empresa_detectada: Some("Logistica Regia Norte SA".to_string()),
            ubicacion_detectada: Some(ubicacion.clone()),
            co_mencionados: vec![CoMencionCruda {
                nombre: "Valeria Ochoa".to_string(),
                contexto: "Socia directora de parque industrial Monterrey".to_string(),
                empresa: Some("Logistica Regia Norte SA".to_string()),
            }],
            datos_extra: BTreeMap::new(),
            opt_out_detectado: false,
        },
        RegistroCrudo {
            fuente_origen: "Registro Publico Comercio NL".to_string(),
            url_evidencia: "https://rpc.ejemplo.mx/nl/folio-911".to_string(),
            capturado_en: ahora,
            formato: FormatoCrudo::TextoPlano,
            contenido_crudo: "Valeria Ochoa coordina compra de nave industrial y fideicomiso patrimonial en Nuevo Leon".to_string(),
            nombre_detectado: Some("Valeria Ochoa".to_string()),
            empresa_detectada: Some("Logistica Regia Norte SA".to_string()),
            ubicacion_detectada: Some(ubicacion.clone()),
            co_mencionados: Vec::new(),
            datos_extra: BTreeMap::new(),
            opt_out_detectado: false,
        },
    ];

    // Fase 1: OSINT
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
    let ingesta = pipeline_osint
        .ejecutar(&ConsultaObjetivo {
            vertical: VerticalComercial::Inmobiliaria(
                SubverticalInmobiliaria::ComercialIndustrial,
            ),
            ubicacion,
            palabras_clave: vec!["bodega".to_string(), "logistica".to_string()],
            limite_resultados: 10,
            requiere_js: false,
        })
        .await?;
    assert!(!ingesta.prospectos.is_empty());
    let prospecto_objetivo = ingesta.prospectos[0].clone();

    // Fase 2: Memoria Topológica 4D
    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        ingesta.prospectos,
        ingesta.relaciones,
        IslaVertical::InmobiliariaComercialIndustrial,
    )?;

    // Fase 3: Córtex BOB + Gatekeeper
    let dir_fabrica = dir_temporal_prueba("aurora_e2e_pulse");
    let (plan, paquete) = preparar_paquete_aprobado(
        &prospecto_objetivo,
        &memoria,
        IslaVertical::InmobiliariaComercialIndustrial,
        dir_fabrica.clone(),
    )
    .await?;

    let llamado_warm = AuroraToolCall::ProgramarCalentamientoPeriferico {
        prospecto_id: prospecto_objetivo.id,
        plataforma: PlataformaCalentamiento::LinkedIn,
        tipo_interaccion: TipoInteraccionPeriferica::ReaccionContextual,
        contexto_publico: "Noticia sobre expansión de naves industriales en Monterrey".to_string(),
    };
    let ResultadoEjecucionTool::CalentamientoProgramado(mut calentamiento) =
        ToolDispatcher.ejecutar(&llamado_warm, plan.campana_id, &memoria)?
    else {
        return Err("no se generó CalentamientoProgramado".into());
    };
    calentamiento.aprobar_gatekeeper()?;

    // Fase 5A: Calentamiento Periférico con ventana 24–72 h
    let rate_limiter = Arc::new(ControladorTasaPlataforma::default());
    let gestor_warmup = GestorCalentamientoPeriferico::new(
        DeterministicMockWarmupDriver::new(),
        Arc::clone(&rate_limiter),
        ConfigCalentamiento {
            ventana_maduracion_horas: 36,
        },
    );
    let registro_warm = gestor_warmup
        .ejecutar_calentamiento(&calentamiento, &memoria, ahora)
        .await?;
    assert_eq!(registro_warm.prospecto_id, prospecto_objetivo.id);
    assert!(!gestor_warmup.impacto_directo_habilitado(
        prospecto_objetivo.id,
        ahora + Duration::hours(12)
    ));
    assert!(gestor_warmup.impacto_directo_habilitado(
        prospecto_objetivo.id,
        ahora + Duration::hours(36)
    ));

    // Fase 5B: Despacho verificado por SHA-256 + transición a EstadoAprobacion::Despachado
    let despachador = DespachadorMulticanal::new(
        DeterministicMockTransport::new(),
        Arc::clone(&rate_limiter),
    );
    let comprobante = despachador
        .despachar(
            &paquete,
            &prospecto_objetivo,
            plan.brief_creativo.canal_sugerido,
            &memoria,
            ahora + Duration::hours(36),
        )
        .await?;

    assert_eq!(comprobante.campana_id, plan.campana_id);
    assert_eq!(comprobante.prospecto_id, prospecto_objetivo.id);
    assert_eq!(comprobante.estado_campana, EstadoCampanaProspecto::Enviado);
    assert_eq!(comprobante.activos_despachados.len(), 4);
    assert!(comprobante
        .activos_despachados
        .iter()
        .all(|a| a.estado == EstadoAprobacion::Despachado));
    assert!(comprobante
        .url_propuesta_tracking
        .contains(&comprobante.tracking_id.to_string()));

    // Fase 5C: Registro en telemetría
    let telemetria = RecolectorTelemetria::new();
    telemetria.registrar_despacho(
        &comprobante,
        Some(IslaVertical::InmobiliariaComercialIndustrial),
        plan.brief_creativo.gancho_3s.clone(),
        plan.brief_creativo.guion_tts.clone(),
    )?;
    let estado = telemetria.registrar_evento(EventoTelemetria::nuevo(
        plan.campana_id,
        prospecto_objetivo.id,
        comprobante.tracking_id,
        Some(IslaVertical::InmobiliariaComercialIndustrial),
        plan.brief_creativo.gancho_3s,
        plan.brief_creativo.guion_tts,
        TipoEventoTelemetria::AperturaDossierPdf,
    ))?;
    assert_eq!(estado, EstadoCampanaProspecto::LandingVisitada);

    let _ = fs::remove_dir_all(&dir_fabrica);
    Ok(())
}

#[tokio::test]
async fn test_purga_inmediata_opt_out_desde_telemetria_y_wal_lfpdppp(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir_wal = dir_temporal_prueba("aurora_pulse_wal_optout");
    let embedder: Arc<dyn TextEmbedder> = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::abrir(
        &dir_wal,
        Arc::clone(&embedder),
        ConfigDecaimiento4D::default(),
    )?;

    let prospecto_1 = construir_prospecto_seguros(
        "Patricia Domínguez Ávila",
        PerfilPsicologico::ConservadorPatrimonial,
        "Grupo Patrimonial Norte",
        0.88,
    );
    let prospecto_2 = construir_prospecto_seguros(
        "Héctor Ramírez Téllez",
        PerfilPsicologico::DinamicoVisual,
        "Capital Industrial Sur",
        0.82,
    );
    memoria.ingestar_lote(
        vec![prospecto_1.clone(), prospecto_2.clone()],
        vec![RelacionTopologica {
            origen_id: prospecto_1.id,
            destino_id: prospecto_2.id,
            tipo_vinculo: TipoVinculo::SocioComercial,
            fuerza: 0.78,
            contexto_compartido: "Socios en fideicomiso empresarial".to_string(),
        }],
        IslaVertical::SegurosPatrimonial,
    )?;

    let dir_fabrica = dir_temporal_prueba("aurora_optout_pkg");
    let (_plan1, paquete1) = preparar_paquete_aprobado(
        &prospecto_1,
        &memoria,
        IslaVertical::SegurosPatrimonial,
        dir_fabrica.clone(),
    )
    .await?;

    let rate_limiter = Arc::new(ControladorTasaPlataforma::default());
    let despachador = DespachadorMulticanal::new(
        DeterministicMockTransport::new(),
        Arc::clone(&rate_limiter),
    );
    let comp1 = despachador
        .despachar(
            &paquete1,
            &prospecto_1,
            CanalContacto::CorreoEjecutivo,
            &memoria,
            Utc::now(),
        )
        .await?;

    let telemetria = RecolectorTelemetria::new();
    telemetria.registrar_despacho(
        &comp1,
        Some(IslaVertical::SegurosPatrimonial),
        "Blindaje sucesorio sin fideicomisos complejos",
        "Resumen ejecutivo patrimonial",
    )?;

    // 1. Llega señal ARCO/opt_out vía evento de telemetría -> detiene campaña y purga WAL + HNSW.
    let estado_baja = telemetria.registrar_evento_con_memoria(
        EventoTelemetria::nuevo(
            comp1.campana_id,
            prospecto_1.id,
            comp1.tracking_id,
            Some(IslaVertical::SegurosPatrimonial),
            "Blindaje sucesorio sin fideicomisos complejos",
            "Resumen ejecutivo patrimonial",
            TipoEventoTelemetria::SolicitudBajaOptOut {
                motivo: "El titular ejerció derecho de oposición LFPDPPP por correo".to_string(),
            },
        ),
        &mut memoria,
    )?;
    assert_eq!(estado_baja, EstadoCampanaProspecto::ExcluidoOptOut);
    assert!(telemetria.esta_prospecto_detenido(prospecto_1.id));
    assert!(memoria.esta_en_blacklist(prospecto_1.id));
    assert!(memoria.grafo().obtener_por_uuid(prospecto_1.id).is_none());

    // Intentar despachar o registrar eventos posteriores para prospecto_1 debe quedar bloqueado.
    let err_desp = despachador
        .despachar(
            &paquete1,
            &prospecto_1,
            CanalContacto::CorreoEjecutivo,
            &memoria,
            Utc::now() + Duration::minutes(5),
        )
        .await;
    assert!(matches!(
        err_desp,
        Err(PulseError::ProspectoOptOut { prospecto_id }) if prospecto_id == prospecto_1.id
    ));

    let err_ev = telemetria.registrar_evento(EventoTelemetria::nuevo(
        comp1.campana_id,
        prospecto_1.id,
        comp1.tracking_id,
        Some(IslaVertical::SegurosPatrimonial),
        "Blindaje sucesorio sin fideicomisos complejos",
        "Resumen ejecutivo patrimonial",
        TipoEventoTelemetria::AperturaDossierPdf,
    ));
    assert!(matches!(
        err_ev,
        Err(PulseError::ProspectoOptOut { prospecto_id }) if prospecto_id == prospecto_1.id
    ));

    // 2. También probamos procesar_solicitud_opt_out directo sobre prospecto_2.
    telemetria.procesar_solicitud_opt_out(
        prospecto_2.id,
        Some(comp1.campana_id),
        "Solicitud directa por WhatsApp STOP",
        &mut memoria,
    )?;
    assert!(telemetria.esta_prospecto_detenido(prospecto_2.id));
    assert!(memoria.esta_en_blacklist(prospecto_2.id));
    assert!(memoria.grafo().obtener_por_uuid(prospecto_2.id).is_none());
    assert_eq!(memoria.grafo().len_nodos(), 0);

    // Reabrimos desde NVMe para comprobar que la purga física LFPDPPP sobrevivió al WAL/snapshot.
    drop(memoria);
    let memoria_reabierta = MotorMemoriaAurora::abrir(
        &dir_wal,
        Arc::clone(&embedder),
        ConfigDecaimiento4D::default(),
    )?;
    assert!(memoria_reabierta.esta_en_blacklist(prospecto_1.id));
    assert!(memoria_reabierta.esta_en_blacklist(prospecto_2.id));
    assert_eq!(memoria_reabierta.grafo().len_nodos(), 0);

    let _ = fs::remove_dir_all(&dir_fabrica);
    let _ = fs::remove_dir_all(&dir_wal);
    Ok(())
}

#[tokio::test]
async fn test_consolidacion_nocturna_dpo_recalibra_grafo_4d_y_genera_jsonl(
) -> Result<(), Box<dyn std::error::Error>> {
    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());

    let prospecto_ganador = construir_prospecto_seguros(
        "Eduardo Montemayor Ruiz",
        PerfilPsicologico::AnaliticoFinanciero,
        "Manufactura Regia SA",
        0.70,
    );
    let prospecto_perdedor = construir_prospecto_seguros(
        "Lucía Castellanos Vera",
        PerfilPsicologico::DinamicoVisual,
        "Distribuidora Centro SA",
        0.70,
    );

    memoria.ingestar_lote(
        vec![prospecto_ganador.clone(), prospecto_perdedor.clone()],
        vec![RelacionTopologica {
            origen_id: prospecto_ganador.id,
            destino_id: prospecto_perdedor.id,
            tipo_vinculo: TipoVinculo::SocioComercial,
            fuerza: 0.60,
            contexto_compartido: "Mesa directiva clúster manufacturero".to_string(),
        }],
        IslaVertical::SegurosHombreClave,
    )?;

    let lambda_antes = memoria.grafo().config().lambda_por_dia;
    let peso_ganador_antes = memoria
        .grafo()
        .obtener_por_uuid(prospecto_ganador.id)
        .and_then(|r| r.prospecto.gatillos.first().map(|g| g.peso_urgencia))
        .ok_or("no se encontró gatillo inicial del prospecto ganador")?;
    let peso_perdedor_antes = memoria
        .grafo()
        .obtener_por_uuid(prospecto_perdedor.id)
        .and_then(|r| r.prospecto.gatillos.first().map(|g| g.peso_urgencia))
        .ok_or("no se encontró gatillo inicial del prospecto perdedor")?;

    let telemetria = RecolectorTelemetria::new();
    let campana_ganadora = Uuid::new_v4();
    let campana_perdedora = Uuid::new_v4();
    let tracking_ganador = Uuid::new_v4();
    let tracking_perdedor = Uuid::new_v4();

    let gancho_bueno = "Deducción fiscal 100% Art. 27 LISR para Seguro Hombre Clave".to_string();
    let guion_bueno =
        "Análisis financiero con ahorro fiscal inmediato y continuidad operativa ante contingencia."
            .to_string();
    let gancho_malo = "¿Buscas contratar un seguro para tu empresa este año?".to_string();
    let guion_malo =
        "Cotiza coberturas generales sin cuantificar impacto fiscal en tu flujo."
            .to_string();

    // Campaña ganadora: 85% retención en Reel + apertura dossier + respuesta positiva
    telemetria.registrar_evento(EventoTelemetria::nuevo(
        campana_ganadora,
        prospecto_ganador.id,
        tracking_ganador,
        Some(IslaVertical::SegurosHombreClave),
        gancho_bueno.clone(),
        guion_bueno.clone(),
        TipoEventoTelemetria::RetencionReel {
            segundos_vistos: 25.5,
            duracion_total_s: 30.0,
            completado: true,
        },
    ))?;
    telemetria.registrar_evento(EventoTelemetria::nuevo(
        campana_ganadora,
        prospecto_ganador.id,
        tracking_ganador,
        Some(IslaVertical::SegurosHombreClave),
        gancho_bueno.clone(),
        guion_bueno.clone(),
        TipoEventoTelemetria::AperturaDossierPdf,
    ))?;
    telemetria.registrar_evento(EventoTelemetria::nuevo(
        campana_ganadora,
        prospecto_ganador.id,
        tracking_ganador,
        Some(IslaVertical::SegurosHombreClave),
        gancho_bueno.clone(),
        guion_bueno.clone(),
        TipoEventoTelemetria::RespuestaPositiva {
            texto_resumen: "Interesado en corrida financiera deducible el jueves".to_string(),
        },
    ))?;

    // Campaña perdedora: 8% retención en Reel sin apertura ni respuesta
    telemetria.registrar_evento(EventoTelemetria::nuevo(
        campana_perdedora,
        prospecto_perdedor.id,
        tracking_perdedor,
        Some(IslaVertical::SegurosHombreClave),
        gancho_malo.clone(),
        guion_malo.clone(),
        TipoEventoTelemetria::RetencionReel {
            segundos_vistos: 2.4,
            duracion_total_s: 30.0,
            completado: false,
        },
    ))?;

    let dir_dpo = dir_temporal_prueba("aurora_dpo_output");
    let motor_dpo = MotorConsolidacionNocturna::new(
        OrquestadorComercialBob::new(DeterministicMockBob),
        ConfigConsolidacionNocturna {
            directorio_salida_dpo: dir_dpo.clone(),
            ..ConfigConsolidacionNocturna::default()
        },
    );

    let dataset = motor_dpo
        .ejecutar_consolidacion_nocturna(&telemetria, &mut memoria)
        .await?;

    // 1. Verificar auditoría por BOB y generación de archivo JSONL DPO válido.
    assert_eq!(dataset.campanas_auditadas.len(), 2);
    assert!(dataset.nodos_4d_recalibrados > 0);
    assert!(dataset.aristas_4d_recalibradas > 0);
    assert!(dataset.ruta_jsonl.exists());

    let contenido_jsonl = fs::read_to_string(&dataset.ruta_jsonl)?;
    let lineas: Vec<&str> = contenido_jsonl.lines().collect();
    assert!(!lineas.is_empty());

    let mut parsed_entries = Vec::new();
    for linea in lineas {
        let entrada: EntradaPreferenciaDpo = serde_json::from_str(linea)?;
        assert!(entrada.chosen.contains("Art. 27 LISR"));
        assert!(entrada.rejected.contains("¿Buscas contratar un seguro"));
        assert!(entrada.tasa_retencion_chosen > 0.80);
        assert!(entrada.tasa_retencion_rejected < 0.10);
        parsed_entries.push(entrada);
    }
    assert_eq!(parsed_entries.len(), dataset.entradas.len());

    // 2. Verificar recalibración real de lambda_por_dia y pesos en el GrafoTemporal4D.
    let lambda_despues = memoria.grafo().config().lambda_por_dia;
    assert!((lambda_despues - lambda_antes).abs() > f32::EPSILON);

    let peso_ganador_despues = memoria
        .grafo()
        .obtener_por_uuid(prospecto_ganador.id)
        .and_then(|r| r.prospecto.gatillos.first().map(|g| g.peso_urgencia))
        .ok_or("no se encontró gatillo recalibrado del ganador")?;
    let peso_perdedor_despues = memoria
        .grafo()
        .obtener_por_uuid(prospecto_perdedor.id)
        .and_then(|r| r.prospecto.gatillos.first().map(|g| g.peso_urgencia))
        .ok_or("no se encontró gatillo recalibrado del perdedor")?;

    assert!(peso_ganador_despues > peso_ganador_antes);
    assert!(peso_perdedor_despues < peso_perdedor_antes);

    let _ = fs::remove_dir_all(&dir_dpo);
    Ok(())
}
