use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use aurora_cortex::{BobInferenceBackend, RolCognitivo};
use aurora_domain::{Prospecto, VerticalComercial};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

use crate::context_sandbox::{
    ConfigContextoAislado, ContextPackager, ContextPrivacyAudit, ReporteAuditoriaPrivacidad,
};
use crate::drivers::{
    gbnf_respuesta_consejo_bob, DiagnosticoSesionAgente, SolicitudConsejo,
    DEFAULT_COUNCIL_CLI_TIMEOUT_MS,
};
use crate::error::{CouncilError, CouncilResult};
use crate::quota::{EstadoCuotaMiembro, MiembroConsejo};
use crate::router::{IntentoFallidoConsejo, OrquestadorConsejo, TipoTareaConsejo};
use crate::sanitizer::{MapaSanitizacionPii, PiiFirewall};

/// Especialidades paralelas coordinadas por BOB como Subdirector de Proyectos Multi-IA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RolEspecialista {
    /// Redacción persuasiva, ganchos de 3s y guiones comerciales de alto ticket.
    GuionesYCopy,
    /// Ingeniería Rust, conectores OSINT, scrapers y validaciones en `aurora-forge`.
    IngenieriaYConectores,
    /// Dirección de escenas 9:16, guiones de video NVENC local y cola de briefs Higgsfield.
    DireccionVideoYContexto,
    /// Estrategia comercial, auditoría DPO, priorización y análisis de mercado.
    EstrategiaYAuditoria,
}

impl RolEspecialista {
    /// Matriz de afinidad ordenada por especialidad, degradando siempre a `BobLocal`.
    #[must_use]
    pub const fn cascada_afinidad(&self) -> &'static [MiembroConsejo] {
        match self {
            Self::GuionesYCopy => &[
                MiembroConsejo::ClaudePro,
                MiembroConsejo::AntigravityGemini,
                MiembroConsejo::CodexPlus,
                MiembroConsejo::BobLocal,
            ],
            Self::IngenieriaYConectores => &[
                MiembroConsejo::CodexPlus,
                MiembroConsejo::ClaudePro,
                MiembroConsejo::OpenCodeLocal,
                MiembroConsejo::BobLocal,
            ],
            Self::DireccionVideoYContexto | Self::EstrategiaYAuditoria => &[
                MiembroConsejo::AntigravityGemini,
                MiembroConsejo::ClaudePro,
                MiembroConsejo::CodexPlus,
                MiembroConsejo::BobLocal,
            ],
        }
    }

    /// Etiqueta estable en español para UI y observabilidad.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::GuionesYCopy => "Guiones y Copy",
            Self::IngenieriaYConectores => "Ingeniería y Conectores",
            Self::DireccionVideoYContexto => "Dirección de Video y Contexto",
            Self::EstrategiaYAuditoria => "Estrategia y Auditoría",
        }
    }

    /// Convierte el rol especialista al [`TipoTareaConsejo`] canónico del consejo.
    #[must_use]
    pub const fn tipo_tarea(&self) -> TipoTareaConsejo {
        match self {
            Self::GuionesYCopy => TipoTareaConsejo::CopyAltoTicket,
            Self::IngenieriaYConectores => TipoTareaConsejo::ReparacionScraper,
            Self::DireccionVideoYContexto => TipoTareaConsejo::MicroHerramientaVenta,
            Self::EstrategiaYAuditoria => TipoTareaConsejo::AuditoriaLoteDpo,
        }
    }

    /// Prompt de sistema específico por rol (consciente de cero PII y modo Higgsfield manual).
    #[must_use]
    pub fn prompt_sistema(&self, higgsfield_pagado: bool) -> String {
        match self {
            Self::GuionesYCopy => {
                "Eres especialista en guiones comerciales y copy de alto ticket para México (Inmobiliaria y Seguros). Entrega ganchos de 3s, estructura persuasiva y CTA claros sin datos personales. Conserva intactos los marcadores [PROSPECTO_XX], [EMPRESA_XX], [CORREO_XX] y [TEL_XX].".to_string()
            }
            Self::IngenieriaYConectores => {
                "Eres ingeniero senior Rust para Aurora v2. Propón diseño o mejoras verificables con cero unwrap()/expect(), cumplimiento estricto LFPDPPP y sin tocar validation.rs ni gatekeeper.".to_string()
            }
            Self::DireccionVideoYContexto => {
                if higgsfield_pagado {
                    "Eres director audiovisual de Reels verticales 9:16 (1080x1920) para TikTok, Instagram y YouTube Shorts. Redacta manifiestos de escenas (>= 61s para monetización), prompts B-Roll sin texto y overlays limpios sin PII.".to_string()
                } else {
                    "Eres director audiovisual de Reels verticales 9:16 (1080x1920) para TikTok, Instagram y YouTube Shorts. IMPORTANTE: Higgsfield NO está pagado (opera en modo cola manual); entrega escenas listas para renderizado local con FFmpeg h264_nvenc + Piper TTS CPU y prompts de cola manual listos para copiar, 100% libres de PII.".to_string()
                }
            }
            Self::EstrategiaYAuditoria => {
                "Eres estratega comercial y auditor de métricas DPO para Aurora v2 en México. Prioriza palancas de conversión, cumplimiento LFPDPPP y decisiones medibles.".to_string()
            }
        }
    }
}

/// Petición de misión multi-track orquestada en paralelo por BOB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolicitudMisionProyecto {
    /// Título corto del proyecto o iniciativa.
    pub titulo: String,
    /// Objetivo comercial, editorial o técnico a descomponer.
    pub objetivo: String,
    /// Vertical comercial objetivo opcional.
    #[serde(default)]
    pub vertical: Option<VerticalComercial>,
    /// Tracks paralelos a ejecutar (si viene vacío usa Guiones, Video y Estrategia).
    #[serde(default)]
    pub tracks: Vec<RolEspecialista>,
    /// Si es `true`, copia únicamente la lista blanca de documentación/código al sandbox.
    #[serde(default = "valor_true")]
    pub incluir_contexto_proyecto: bool,
    /// UUIDs de prospectos opcionales cuyo contexto agregado se sanitizará vía `PiiFirewall`.
    #[serde(default)]
    pub prospecto_ids: Vec<Uuid>,
    /// Si es `true` y existe el track `IngenieriaYConectores`, delega código a `aurora-forge`.
    #[serde(default)]
    pub ejecutar_forge_en_codigo: bool,
    /// Bandera operativa de Higgsfield (`false` por defecto: modo cola manual + NVENC local).
    #[serde(default)]
    pub higgsfield_pagado: bool,
    /// Timeout por subtarea en milisegundos.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

const fn valor_true() -> bool {
    true
}

/// Subtarea concreta asignada por BOB a un track especialista.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsignacionSubtarea {
    /// Especialidad responsable.
    pub rol: RolEspecialista,
    /// Consigna enfocada generada por BOB.
    pub consigna: String,
    /// Orden de preferencia de agentes para este track.
    pub cascada_preferida: Vec<MiembroConsejo>,
}

/// Estado final de una subtarea paralela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EstadoSubtarea {
    /// Ejecutada y verificada correctamente.
    Completada,
    /// No pudo completarse ni con respaldo local.
    Fallida,
}

/// Resultado individual de un track dentro de la misión paralela.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultadoSubtarea {
    /// Especialidad del track.
    pub rol: RolEspecialista,
    /// Consigna ejecutada (sin PII cruda).
    pub consigna: String,
    /// Miembro del consejo que resolvió finalmente el track.
    pub miembro_ejecutor: MiembroConsejo,
    /// Indica si hubo reasignación respecto al primer especialista de la matriz de afinidad.
    pub fallback_usado: bool,
    /// Traza de miembros omitidos o fallidos antes de completar el track.
    pub intentos_fallidos: Vec<IntentoFallidoConsejo>,
    /// Latencia real del track en milisegundos.
    pub latencia_ms: u64,
    /// Estado final del track.
    pub estado: EstadoSubtarea,
    /// Contenido entregado (rehidratado localmente en memoria si hubo tokens PII).
    pub salida: String,
    /// Rama `forge/<id>` creada cuando `ejecutar_forge_en_codigo == true` (en `PendienteRevisionHumana`).
    pub rama_forge: Option<String>,
    /// Estado de cuota actualizado del miembro ejecutor.
    pub consumo_cuota: Option<EstadoCuotaMiembro>,
}

/// Reporte consolidado de una misión multi-IA ejecutada por [`SubdirectorProyectosBob`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReporteMisionProyecto {
    /// Identificador único de la misión.
    pub mision_id: Uuid,
    /// Título de la misión.
    pub titulo: String,
    /// Objetivo de la misión (versión sanitizada).
    pub objetivo: String,
    /// Vertical objetivo opcional.
    pub vertical: Option<VerticalComercial>,
    /// Resumen del plan generado por BOB como Subdirector.
    pub plan_bob: String,
    /// Resultados individuales por cada track ejecutado en paralelo.
    pub subtareas: Vec<ResultadoSubtarea>,
    /// Tiempo total de pared de la misión (ms) — refleja paralelismo real.
    pub latencia_total_ms: u64,
    /// Resultado de la auditoría pre-vuelo de privacidad y cero secretos.
    pub auditoria_privacidad: ReporteAuditoriaPrivacidad,
    /// Número de entidades PII protegidas por `PiiFirewall`.
    pub entidades_pii_sanitizadas: usize,
    /// Confirma que el directorio temporal del sandbox fue limpiado al finalizar.
    pub sandbox_limpiado: bool,
    /// Indica el modo activo de Higgsfield durante la misión.
    pub higgsfield_modo: String,
    /// Marca temporal UTC de finalización.
    pub ejecutada_en: DateTime<Utc>,
}

/// Resultado devuelto por el delegado opcional de `aurora-forge` para el track de ingeniería.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultadoDelegacionForja {
    /// Rama aislada creada (`forge/<id>`), siempre en `PendienteRevisionHumana`.
    pub rama_forge: Option<String>,
    /// Resumen técnico verificable de la ejecución en sandbox.
    pub resumen: String,
}

/// Puente desacoplado hacia `aurora-forge` inyectado desde `aurora-daemon` para evitar ciclos de crates.
#[async_trait]
pub trait ForgeCodeDelegate: Send + Sync {
    /// Ejecuta una orden acotada en `WorktreeSandbox` conservando todas las rutas protegidas.
    async fn ejecutar_track_codigo(
        &self,
        mision_id: Uuid,
        consigna_sanitizada: &str,
    ) -> CouncilResult<ResultadoDelegacionForja>;
}

/// Subdirector de Proyectos Multi-IA encabezado por BOB Local (`slot_id = 0`).
///
/// Orquesta en paralelo a Claude Code, Codex, Antigravity/Gemini, OpenCode y BOB Local
/// sobre un directorio efímero aislado auditado por [`ContextPrivacyAudit`], con semáforo = 1
/// por proveedor y protección total de suscripciones vía [`QuotaGovernor`].
pub struct SubdirectorProyectosBob {
    consejo: Arc<OrquestadorConsejo>,
    config_contexto: ConfigContextoAislado,
    bob_planificador: Option<Arc<dyn BobInferenceBackend>>,
    bob_slot_lock: Option<Arc<Mutex<()>>>,
    forge_delegate: Option<Arc<dyn ForgeCodeDelegate>>,
}

impl SubdirectorProyectosBob {
    /// Construye el Subdirector sobre el [`OrquestadorConsejo`] y la configuración de sandbox aislado.
    #[must_use]
    pub fn new(
        consejo: Arc<OrquestadorConsejo>,
        config_contexto: ConfigContextoAislado,
    ) -> Self {
        Self {
            consejo,
            config_contexto,
            bob_planificador: None,
            bob_slot_lock: None,
            forge_delegate: None,
        }
    }

    /// Conecta el backend local BOB (`slot_id = 0`) y su mutex de exclusividad VRAM para planificación.
    #[must_use]
    pub fn con_bob_planificador(
        mut self,
        backend: Arc<dyn BobInferenceBackend>,
        slot_lock: Arc<Mutex<()>>,
    ) -> Self {
        self.bob_planificador = Some(backend);
        self.bob_slot_lock = Some(slot_lock);
        self
    }

    /// Conecta el puente hacia `aurora-forge` para el track `IngenieriaYConectores`.
    #[must_use]
    pub fn con_forge_delegate(mut self, delegate: Arc<dyn ForgeCodeDelegate>) -> Self {
        self.forge_delegate = Some(delegate);
        self
    }

    /// Devuelve el diagnóstico no intrusivo de disponibilidad y sesión de los agentes registrados.
    #[must_use]
    pub fn diagnosticos_agentes(&self) -> Vec<DiagnosticoSesionAgente> {
        self.consejo.diagnosticos_agentes()
    }

    /// Ejecuta una misión multi-track en paralelo con contexto aislado y auditoría cero-fugas.
    #[tracing::instrument(
        skip(self, solicitud, prospectos_referenciados, prospectos_activos_auditoria),
        fields(titulo = %solicitud.titulo, tracks = solicitud.tracks.len())
    )]
    pub async fn ejecutar_mision(
        &self,
        solicitud: SolicitudMisionProyecto,
        prospectos_referenciados: &[Prospecto],
        prospectos_activos_auditoria: &[Prospecto],
    ) -> CouncilResult<ReporteMisionProyecto> {
        if solicitud.titulo.trim().is_empty() || solicitud.objetivo.trim().is_empty() {
            return Err(CouncilError::SalidaInvalida {
                miembro: "subdirector_bob".to_string(),
                detalle: "titulo y objetivo de la misión son obligatorios".to_string(),
            });
        }

        let inicio_mision = Instant::now();
        let mision_id = Uuid::new_v4();

        let tracks = if solicitud.tracks.is_empty() {
            vec![
                RolEspecialista::GuionesYCopy,
                RolEspecialista::DireccionVideoYContexto,
                RolEspecialista::EstrategiaYAuditoria,
            ]
        } else {
            let mut unicos = Vec::new();
            for t in &solicitud.tracks {
                if !unicos.contains(t) {
                    unicos.push(*t);
                }
            }
            unicos
        };

        // 1. Sanitización PII en RAM para referencias a prospectos + verificación de cero secretos.
        let mut mapa_pii_global = MapaSanitizacionPii::default();
        let (titulo_limpio, objetivo_limpio, resumen_prospectos_sanitizado) =
            sanitizar_contexto_mision(
                &solicitud.titulo,
                &solicitud.objetivo,
                prospectos_referenciados,
                &mut mapa_pii_global,
            );

        // 2. Planificación de subtareas con BOB Local (o descomposición determinista si está ocupado/offline).
        let (plan_bob, asignaciones) = self
            .planificar_con_bob(
                &titulo_limpio,
                &objetivo_limpio,
                solicitud.vertical,
                &tracks,
                solicitud.higgsfield_pagado,
            )
            .await;

        // 3. Construcción de MISSION_BRIEF.md sanitizado y empaquetado del sandbox aislado.
        let vertical_str = solicitud
            .vertical
            .map_or_else(|| "general".to_string(), |v| v.etiqueta().to_string());
        let higgsfield_modo = if solicitud.higgsfield_pagado {
            "api_pagada".to_string()
        } else {
            "cola_manual_sin_pago".to_string()
        };
        let brief_md = format!(
            "# Misión Aurora v2 :: {titulo_limpio}\n\n\
             - ID Misión: {mision_id}\n\
             - Vertical: {vertical_str}\n\
             - Modo Higgsfield: {higgsfield_modo}\n\
             - Plan Subdirector BOB: {plan_bob}\n\n\
             ## Objetivo\n{objetivo_limpio}\n\n\
             {resumen_prospectos_sanitizado}\
             ## Reglas de Seguridad\n\
             - Cero datos personales reales (LFPDPPP).\n\
             - Cero secretos, tokens ni archivos de configuración.\n\
             - Cero unwrap()/expect() en código Rust.\n"
        );

        let entorno = ContextPackager::preparar_entorno_mision(
            &self.config_contexto,
            mision_id,
            &brief_md,
            solicitud.incluir_contexto_proyecto,
        )?;

        // 4. Auditoría pre-vuelo obligatoria: escanea staging completo + prompt contra PII y secretos.
        let prompt_auditoria = format!(
            "{brief_md}\n{}",
            asignaciones
                .iter()
                .map(|a| a.consigna.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let auditoria_privacidad = ContextPrivacyAudit::auditar_directorio_y_prompt(
            &entorno.directorio,
            &prompt_auditoria,
            prospectos_activos_auditoria,
        )?;

        // 5. Ejecución PARALELA real entre proveedores distintos vía JoinSet (semáforo = 1 por proveedor).
        let timeout_ms = solicitud
            .timeout_ms
            .unwrap_or(DEFAULT_COUNCIL_CLI_TIMEOUT_MS)
            .max(1_000);
        let sandbox_dir = entorno.directorio.clone();
        let ejecutar_forge = solicitud.ejecutar_forge_en_codigo;
        let higgsfield_pagado = solicitud.higgsfield_pagado;

        let mut join_set: JoinSet<(usize, ResultadoSubtarea)> = JoinSet::new();
        for (indice, asignacion) in asignaciones.into_iter().enumerate() {
            let consejo = Arc::clone(&self.consejo);
            let forge_delegate = self.forge_delegate.clone();
            let dir_trabajo = sandbox_dir.clone();
            let brief_ref = brief_md.clone();

            join_set.spawn(async move {
                let res = ejecutar_subtarea_especialista(
                    mision_id,
                    asignacion,
                    consejo,
                    forge_delegate,
                    dir_trabajo,
                    brief_ref,
                    ejecutar_forge,
                    higgsfield_pagado,
                    timeout_ms,
                )
                .await;
                (indice, res)
            });
        }

        let mut resultados_indexados: BTreeMap<usize, ResultadoSubtarea> = BTreeMap::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((idx, mut res)) => {
                    if !mapa_pii_global.is_empty() {
                        res.salida = PiiFirewall::rehidratar_respuesta(&res.salida, &mapa_pii_global);
                    }
                    resultados_indexados.insert(idx, res);
                }
                Err(join_err) => {
                    warn!(error = %join_err, "Error en tarea paralela JoinSet del Subdirector");
                }
            }
        }

        let subtareas: Vec<ResultadoSubtarea> = resultados_indexados.into_values().collect();

        // 6. Limpieza física explícita del entorno aislado (respaldada además por Drop).
        let sandbox_limpiado = entorno.limpiar().is_ok();
        let latencia_total_ms = inicio_mision.elapsed().as_millis().min(u64::MAX as u128) as u64;

        info!(
            mision_id = %mision_id,
            tracks = subtareas.len(),
            latencia_total_ms,
            sandbox_limpiado,
            "Misión Multi-IA paralela completada por Subdirector BOB"
        );

        Ok(ReporteMisionProyecto {
            mision_id,
            titulo: titulo_limpio,
            objetivo: objetivo_limpio,
            vertical: solicitud.vertical,
            plan_bob,
            subtareas,
            latencia_total_ms,
            auditoria_privacidad,
            entidades_pii_sanitizadas: mapa_pii_global.len(),
            sandbox_limpiado,
            higgsfield_modo,
            ejecutada_en: Utc::now(),
        })
    }

    async fn planificar_con_bob(
        &self,
        titulo: &str,
        objetivo: &str,
        vertical: Option<VerticalComercial>,
        tracks: &[RolEspecialista],
        higgsfield_pagado: bool,
    ) -> (String, Vec<AsignacionSubtarea>) {
        let vertical_label = vertical.map_or("general", |v| v.etiqueta());
        let modo_video = if higgsfield_pagado {
            "API Higgsfield + NVENC local"
        } else {
            "cola manual Higgsfield (sin pago) + renderizado NVENC local + Piper CPU"
        };

        let mut resumen_plan = format!(
            "BOB descompuso '{titulo}' ({vertical_label}) en {} tracks paralelos sobre entorno aislado ({modo_video}).",
            tracks.len()
        );

        // Si BOB Local está disponible y libre (try_lock para nunca bloquear una cacería activa),
        // solicitamos un resumen ejecutivo breve de coordinación bajo GBNF estricto.
        if let Some(ref backend) = self.bob_planificador {
            let intento_lock = match &self.bob_slot_lock {
                Some(lock) => lock.try_lock().ok(),
                None => None,
            };
            let puede_consultar_bob = self.bob_slot_lock.is_none() || intento_lock.is_some();
            if puede_consultar_bob {
                let ctx = format!(
                    "Eres BOB Subdirector de Proyectos. Resume en 1 frase clara el enfoque coordinador para la misión '{titulo}' (vertical={vertical_label}, modo_video={modo_video}) con objetivo: {objetivo}"
                );
                if let Ok(raw) = backend
                    .inferir_estructurado(
                        RolCognitivo::Auditor,
                        &ctx,
                        gbnf_respuesta_consejo_bob(),
                    )
                    .await
                {
                    if let Some(texto) = serde_json::from_str::<serde_json::Value>(&raw)
                        .ok()
                        .and_then(|v| {
                            v.get("respuesta")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(ToString::to_string)
                        })
                    {
                        if ContextPrivacyAudit::verificar_sin_secretos(&texto).is_ok()
                            && PiiFirewall::verificar_texto_sin_pii_generica(&texto).is_ok()
                        {
                            resumen_plan = texto;
                        }
                    }
                }
            }
        }

        let asignaciones = tracks
            .iter()
            .map(|rol| {
                let enfoque = match rol {
                    RolEspecialista::GuionesYCopy => format!(
                        "Redacta guion comercial, 3 ganchos de apertura de 3 segundos y CTA en español de México para '{titulo}' ({vertical_label}). Objetivo: {objetivo}"
                    ),
                    RolEspecialista::IngenieriaYConectores => format!(
                        "Diseña la arquitectura técnica Rust (cero unwrap/expect, cumplimiento LFPDPPP) de conectores o mejoras para '{titulo}' ({vertical_label}). Objetivo: {objetivo}"
                    ),
                    RolEspecialista::DireccionVideoYContexto => format!(
                        "Crea manifiesto de escenas verticales 9:16 (>= 61s para monetización TikTok/Reels/Shorts) operando en modo {modo_video} para '{titulo}' ({vertical_label}). Objetivo: {objetivo}"
                    ),
                    RolEspecialista::EstrategiaYAuditoria => format!(
                        "Emite plan de priorización, KPIs medibles de retención/conversión y riesgos LFPDPPP para '{titulo}' ({vertical_label}). Objetivo: {objetivo}"
                    ),
                };
                AsignacionSubtarea {
                    rol: *rol,
                    consigna: enfoque,
                    cascada_preferida: rol.cascada_afinidad().to_vec(),
                }
            })
            .collect();

        (resumen_plan, asignaciones)
    }
}

#[allow(clippy::too_many_arguments)]
async fn ejecutar_subtarea_especialista(
    mision_id: Uuid,
    asignacion: AsignacionSubtarea,
    consejo: Arc<OrquestadorConsejo>,
    forge_delegate: Option<Arc<dyn ForgeCodeDelegate>>,
    dir_trabajo: std::path::PathBuf,
    brief_md: String,
    ejecutar_forge: bool,
    higgsfield_pagado: bool,
    timeout_ms: u64,
) -> ResultadoSubtarea {
    let inicio = Instant::now();
    let governor = consejo.governor();
    let mut intentos_fallidos = Vec::new();
    let preferido_inicial = asignacion
        .cascada_preferida
        .first()
        .copied()
        .unwrap_or(MiembroConsejo::BobLocal);

    // Si el track es de ingeniería y el usuario activó sandbox de código `aurora-forge`.
    if asignacion.rol == RolEspecialista::IngenieriaYConectores && ejecutar_forge {
        if let Some(delegate) = forge_delegate {
            match delegate
                .ejecutar_track_codigo(mision_id, &asignacion.consigna)
                .await
            {
                Ok(res_forge) => {
                    return ResultadoSubtarea {
                        rol: asignacion.rol,
                        consigna: asignacion.consigna,
                        miembro_ejecutor: MiembroConsejo::CodexPlus,
                        fallback_usado: false,
                        intentos_fallidos,
                        latencia_ms: inicio.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        estado: EstadoSubtarea::Completada,
                        salida: res_forge.resumen,
                        rama_forge: res_forge.rama_forge,
                        consumo_cuota: None,
                    };
                }
                Err(err) => {
                    intentos_fallidos.push(IntentoFallidoConsejo {
                        miembro: MiembroConsejo::CodexPlus,
                        motivo: format!("aurora-forge delegó fallback: {err}"),
                    });
                }
            }
        }
    }

    let prompt_usuario = format!(
        "{}\n\n[REFERENCIA SANITIZADA DEL PROYECTO]\n{}",
        asignacion.consigna, brief_md
    );
    let prompt_sistema = asignacion.rol.prompt_sistema(higgsfield_pagado);

    for &miembro in &asignacion.cascada_preferida {
        let Some(driver) = consejo.driver(miembro) else {
            intentos_fallidos.push(IntentoFallidoConsejo {
                miembro,
                motivo: "driver no registrado".to_string(),
            });
            continue;
        };

        if !driver.disponible() {
            intentos_fallidos.push(IntentoFallidoConsejo {
                miembro,
                motivo: "binario CLI no disponible en sistema".to_string(),
            });
            continue;
        }

        if !matches!(miembro, MiembroConsejo::BobLocal) && !governor.puede_ejecutar(miembro).await {
            let estado_cuota = governor.estado_miembro(miembro).await;
            intentos_fallidos.push(IntentoFallidoConsejo {
                miembro,
                motivo: estado_cuota
                    .motivo_bloqueo
                    .unwrap_or_else(|| "cuota temporalmente agotada".to_string()),
            });
            continue;
        }

        // Semáforo = 1 por proveedor: paralelismo simultáneo entre proveedores distintos sin ráfagas
        // sobre la misma suscripción.
        let semaforo = governor.semaforo_miembro(miembro);
        let Ok(_permiso) = semaforo.acquire_owned().await else {
            intentos_fallidos.push(IntentoFallidoConsejo {
                miembro,
                motivo: "semáforo de proveedor cerrado".to_string(),
            });
            continue;
        };

        // Re-verificación de cuota tras adquirir el semáforo para evitar condición de carrera.
        if !matches!(miembro, MiembroConsejo::BobLocal) && !governor.puede_ejecutar(miembro).await {
            intentos_fallidos.push(IntentoFallidoConsejo {
                miembro,
                motivo: "cuota agotada al adquirir turno de ejecución".to_string(),
            });
            continue;
        }

        let solicitud = SolicitudConsejo {
            id: Uuid::new_v4(),
            tarea: asignacion.rol.tipo_tarea(),
            prompt_sistema: prompt_sistema.clone(),
            prompt_usuario: prompt_usuario.clone(),
            directorio_trabajo: Some(dir_trabajo.clone()),
            timeout_ms,
        };

        match driver.ejecutar(&solicitud).await {
            Ok(resp) => {
                let consumo = governor.registrar_uso(miembro).await.ok();
                return ResultadoSubtarea {
                    rol: asignacion.rol,
                    consigna: asignacion.consigna,
                    miembro_ejecutor: miembro,
                    fallback_usado: miembro != preferido_inicial || !intentos_fallidos.is_empty(),
                    intentos_fallidos,
                    latencia_ms: inicio.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    estado: EstadoSubtarea::Completada,
                    salida: resp.contenido,
                    rama_forge: None,
                    consumo_cuota: consumo,
                };
            }
            Err(CouncilError::RateLimitExterno { detalle, .. }) => {
                let _ = governor.registrar_rate_limit(miembro, detalle.clone()).await;
                intentos_fallidos.push(IntentoFallidoConsejo {
                    miembro,
                    motivo: format!("rate-limit externo: {detalle}"),
                });
            }
            Err(err) => {
                intentos_fallidos.push(IntentoFallidoConsejo {
                    miembro,
                    motivo: err.to_string(),
                });
            }
        }
    }

    ResultadoSubtarea {
        rol: asignacion.rol,
        consigna: asignacion.consigna,
        miembro_ejecutor: MiembroConsejo::BobLocal,
        fallback_usado: true,
        intentos_fallidos,
        latencia_ms: inicio.elapsed().as_millis().min(u64::MAX as u128) as u64,
        estado: EstadoSubtarea::Fallida,
        salida: "Ningún miembro del consejo pudo completar el track solicitado.".to_string(),
        rama_forge: None,
        consumo_cuota: None,
    }
}

fn sanitizar_contexto_mision(
    titulo: &str,
    objetivo: &str,
    prospectos: &[Prospecto],
    mapa_global: &mut MapaSanitizacionPii,
) -> (String, String, String) {
    let mut titulo_out = titulo.trim().to_string();
    let mut objetivo_out = objetivo.trim().to_string();
    let mut bloques = Vec::new();

    for (idx, p) in prospectos.iter().enumerate() {
        titulo_out = PiiFirewall::sanitizar_para_nube_en_mapa(p, &titulo_out, mapa_global);
        objetivo_out = PiiFirewall::sanitizar_para_nube_en_mapa(p, &objetivo_out, mapa_global);

        bloques.push(format!(
            "- Segmento #{}: plaza={}, perfil={}, ingreso_anual_mxn={}",
            idx + 1,
            p.ubicacion.estado,
            p.perfil_psicologico.como_str(),
            p.capacidad_adquisitiva.ingreso_anual_estimado_mxn
        ));
    }

    let resumen_prospectos = if bloques.is_empty() {
        String::new()
    } else {
        format!(
            "## Contexto Comercial Agregado (Anónimo / Sin PII)\n{}\n\n",
            bloques.join("\n")
        )
    };

    (titulo_out, objetivo_out, resumen_prospectos)
}
