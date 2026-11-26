use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aurora_domain::Prospecto;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::drivers::{
    CouncilAgentDriver, DiagnosticoSesionAgente, SolicitudConsejo, DEFAULT_COUNCIL_CLI_TIMEOUT_MS,
};
use crate::error::{CouncilError, CouncilResult};
use crate::quota::{EstadoCuotaMiembro, MiembroConsejo, QuotaGovernor, ORDEN_CASCADA_CONSEJO};
use crate::sanitizer::{MapaSanitizacionPii, PiiFirewall};

/// Tipos de tareas especializadas gobernadas por el Consejo Multi-Agente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoTareaConsejo {
    /// Redacción persuasiva de alto ticket (inmobiliaria/seguros patrimoniales).
    CopyAltoTicket,
    /// Respuesta consultiva ante objeciones complejas de clientes B2B/HNWI.
    RespuestaObjecionCompleja,
    /// Auditoría por lotes del dataset nocturno DPO y patrones de conversión.
    AuditoriaLoteDpo,
    /// Reparación automatizada de selectores o adaptadores OSINT (Auto-healing).
    ReparacionScraper,
    /// Generación de micro-herramientas o calculadoras comerciales de venta.
    MicroHerramientaVenta,
}

impl TipoTareaConsejo {
    /// Etiqueta estable para logs y telemetría.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::CopyAltoTicket => "copy_alto_ticket",
            Self::RespuestaObjecionCompleja => "respuesta_objecion_compleja",
            Self::AuditoriaLoteDpo => "auditoria_lote_dpo",
            Self::ReparacionScraper => "reparacion_scraper",
            Self::MicroHerramientaVenta => "micro_herramienta_venta",
        }
    }

    /// Prompt de sistema por defecto adaptado a cada especialidad del consejo.
    #[must_use]
    pub const fn prompt_sistema(&self) -> &'static str {
        match self {
            Self::CopyAltoTicket => {
                "Eres un redactor ejecutivo B2B en español de México para operaciones de alto ticket. Conserva exactamente todos los marcadores [PROSPECTO_XX], [EMPRESA_XX], [CORREO_XX] y [TEL_XX] sin modificarlos."
            }
            Self::RespuestaObjecionCompleja => {
                "Eres un estratega consultivo especializado en manejo de objeciones financieras y patrimoniales en México. Conserva intactos los marcadores [PROSPECTO_XX], [EMPRESA_XX], [CORREO_XX] y [TEL_XX]."
            }
            Self::AuditoriaLoteDpo => {
                "Eres un auditor cuantitativo de campañas comerciales y datasets DPO. Emite hallazgos accionables, concisos y verificables."
            }
            Self::ReparacionScraper => {
                "Eres un ingeniero Rust especialista en extracción OSINT pública y cumplimiento LFPDPPP. Propón correcciones mínimas, deterministas y libres de unwrap/expect."
            }
            Self::MicroHerramientaVenta => {
                "Eres un ingeniero de producto comercial. Diseña calculadoras o micro-herramientas de venta claras, precisas y verificables."
            }
        }
    }
}

/// Petición de alto nivel enviada a [`OrquestadorConsejo::delegar`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeticionDelegacion {
    /// Especialidad de la tarea.
    pub tarea: TipoTareaConsejo,
    /// Contexto y consigna del trabajo a delegar.
    pub contexto: String,
    /// Prospecto asociado opcional; si existe, se aplica `PiiFirewall` estricto antes de salir.
    #[serde(default)]
    pub prospecto: Option<Prospecto>,
    /// Si es `true`, resuelve directamente en `BobLocal` sin consultar cuotas de nube.
    #[serde(default)]
    pub forzar_local: bool,
    /// Directorio de trabajo opcional para tareas de código confinadas a un worktree.
    #[serde(default)]
    pub directorio_trabajo: Option<PathBuf>,
    /// Timeout específico opcional en milisegundos.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Registro auditable de cada miembro descartado o fallido durante la cascada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentoFallidoConsejo {
    /// Miembro evaluado o invocado que no completó la tarea.
    pub miembro: MiembroConsejo,
    /// Causa técnica (cuota agotada, rate-limit 429, binario ausente, timeout, etc.).
    pub motivo: String,
}

/// Resultado completo y trazable de una delegación del consejo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultadoDelegacion {
    /// Identificador único de la delegación.
    pub solicitud_id: Uuid,
    /// Categoría de tarea resuelta.
    pub tarea: TipoTareaConsejo,
    /// Miembro que generó efectivamente la respuesta final.
    pub miembro_respondedor: MiembroConsejo,
    /// Contenido final ya rehidratado localmente cuando hubo sanitización PII.
    pub contenido: String,
    /// Latencia total medida en milisegundos.
    pub latencia_ms: u64,
    /// Indica si fue necesario caer a un miembro secundario o a `BobLocal`.
    pub hubo_fallback: bool,
    /// Traza ordenada de miembros omitidos o fallidos antes del éxito.
    pub intentos_fallidos: Vec<IntentoFallidoConsejo>,
    /// Confirma si se aplicó `PiiFirewall` isomórfico en el envío externo.
    pub pii_sanitizado: bool,
    /// Número de entidades PII reemplazadas por marcadores neutros.
    pub marcadores_pii_aplicados: usize,
    /// Estado actualizado de cuota del miembro que atendió la solicitud.
    pub consumo_cuota: EstadoCuotaMiembro,
}

/// Orquestador multi-agente gobernado por cuotas NVMe y protegido por `PiiFirewall`.
pub struct OrquestadorConsejo {
    governor: Arc<QuotaGovernor>,
    drivers: BTreeMap<MiembroConsejo, Arc<dyn CouncilAgentDriver>>,
    timeout_por_defecto_ms: u64,
}

impl OrquestadorConsejo {
    /// Construye un orquestador vacío respaldado por el [`QuotaGovernor`] dado.
    #[must_use]
    pub fn new(governor: Arc<QuotaGovernor>) -> Self {
        Self {
            governor,
            drivers: BTreeMap::new(),
            timeout_por_defecto_ms: DEFAULT_COUNCIL_CLI_TIMEOUT_MS,
        }
    }

    /// Configura el timeout por defecto en milisegundos para los drivers CLI.
    #[must_use]
    pub fn con_timeout_por_defecto_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_por_defecto_ms = timeout_ms.max(1_000);
        self
    }

    /// Registra o reemplaza el driver para un miembro del consejo.
    pub fn registrar_driver(&mut self, driver: Arc<dyn CouncilAgentDriver>) {
        self.drivers.insert(driver.miembro(), driver);
    }

    /// Variante fluida para encadenar registros de drivers.
    #[must_use]
    pub fn con_driver(mut self, driver: Arc<dyn CouncilAgentDriver>) -> Self {
        self.registrar_driver(driver);
        self
    }

    /// Devuelve referencia compartida al gobernador de cuotas NVMe.
    #[must_use]
    pub fn governor(&self) -> Arc<QuotaGovernor> {
        Arc::clone(&self.governor)
    }

    /// Devuelve el driver registrado para `miembro` si existe.
    #[must_use]
    pub fn driver(&self, miembro: MiembroConsejo) -> Option<Arc<dyn CouncilAgentDriver>> {
        self.drivers.get(&miembro).map(Arc::clone)
    }

    /// Devuelve el diagnóstico no intrusivo de disponibilidad y sesión de cada miembro del consejo.
    #[must_use]
    pub fn diagnosticos_agentes(&self) -> Vec<DiagnosticoSesionAgente> {
        ORDEN_CASCADA_CONSEJO
            .iter()
            .map(|miembro| match self.drivers.get(miembro) {
                Some(driver) => driver.verificar_disponibilidad_y_sesion(),
                None => DiagnosticoSesionAgente {
                    miembro: *miembro,
                    binario_encontrado: false,
                    ruta_binario: None,
                    sesion_local_detectada: false,
                    detalle: "Driver no registrado en el orquestador".to_string(),
                },
            })
            .collect()
    }

    /// Evalúa si una petición amerita escalamiento al consejo externo o ejecución directa en BOB.
    #[must_use]
    pub fn debe_escalar_a_consejo(&self, peticion: &PeticionDelegacion) -> bool {
        if peticion.forzar_local {
            return false;
        }
        // Si no hay ningún driver externo registrado y disponible, resuelve directo en BOB local.
        ORDEN_CASCADA_CONSEJO
            .iter()
            .filter(|m| !matches!(m, MiembroConsejo::BobLocal))
            .any(|m| self.drivers.get(m).is_some_and(|d| d.disponible()))
    }

    /// Ejecuta la cascada gobernada con `PiiFirewall` obligatorio y degradación garantizada a `BobLocal`.
    #[tracing::instrument(
        skip(self, peticion),
        fields(
            tarea = peticion.tarea.etiqueta(),
            forzar_local = peticion.forzar_local,
            con_prospecto = peticion.prospecto.is_some()
        )
    )]
    pub async fn delegar(&self, peticion: PeticionDelegacion) -> CouncilResult<ResultadoDelegacion> {
        if peticion.contexto.trim().is_empty() {
            return Err(CouncilError::SalidaInvalida {
                miembro: "orquestador_consejo".to_string(),
                detalle: "el contexto de delegación no puede estar vacío".to_string(),
            });
        }

        let inicio_total = Instant::now();
        let solicitud_id = Uuid::new_v4();
        let timeout_ms = peticion
            .timeout_ms
            .unwrap_or(self.timeout_por_defecto_ms)
            .max(1_000);

        let escalar = self.debe_escalar_a_consejo(&peticion);
        let mut intentos_fallidos = Vec::new();

        // Prepara payload sanitizado en RAM una sola vez si se evaluarán miembros externos.
        let mut payload_sanitizado: Option<(String, MapaSanitizacionPii)> = None;

        if escalar {
            let estados_previos = self.governor.estado_todas_las_cuotas().await;
            let cascada_vigente = self.governor.orden_cascada_vigente().await;

            // Registra los miembros externos ya excluidos por cuota o cooldown para trazabilidad.
            for st in &estados_previos {
                if !matches!(st.miembro, MiembroConsejo::BobLocal)
                    && !cascada_vigente.contains(&st.miembro)
                {
                    intentos_fallidos.push(IntentoFallidoConsejo {
                        miembro: st.miembro,
                        motivo: st
                            .motivo_bloqueo
                            .clone()
                            .unwrap_or_else(|| "cuota agotada o en cooldown".to_string()),
                    });
                }
            }

            for miembro in cascada_vigente {
                if matches!(miembro, MiembroConsejo::BobLocal) {
                    continue;
                }

                let Some(driver) = self.drivers.get(&miembro) else {
                    intentos_fallidos.push(IntentoFallidoConsejo {
                        miembro,
                        motivo: "driver no registrado".to_string(),
                    });
                    continue;
                };

                if !driver.disponible() {
                    intentos_fallidos.push(IntentoFallidoConsejo {
                        miembro,
                        motivo: "binario CLI no disponible en el sistema".to_string(),
                    });
                    continue;
                }

                if !self.governor.puede_ejecutar(miembro).await {
                    intentos_fallidos.push(IntentoFallidoConsejo {
                        miembro,
                        motivo: "cuota agotada justo antes de ejecutar".to_string(),
                    });
                    continue;
                }

                // Barrera LFPDPPP obligatoria: sanitiza y verifica cero PII justo antes de salir.
                let (ref texto_limpio, ref mapa_pii) = *payload_sanitizado.get_or_insert_with(|| {
                    match &peticion.prospecto {
                        Some(prospecto) => {
                            PiiFirewall::sanitizar_para_nube(prospecto, &peticion.contexto)
                        }
                        None => PiiFirewall::sanitizar_texto_generico(&peticion.contexto),
                    }
                });

                match &peticion.prospecto {
                    Some(prospecto) => PiiFirewall::verificar_libre_de_pii(texto_limpio, prospecto)?,
                    None => PiiFirewall::verificar_texto_sin_pii_generica(texto_limpio)?,
                }

                let solicitud = SolicitudConsejo {
                    id: solicitud_id,
                    tarea: peticion.tarea,
                    prompt_sistema: peticion.tarea.prompt_sistema().to_string(),
                    prompt_usuario: texto_limpio.clone(),
                    directorio_trabajo: peticion.directorio_trabajo.clone(),
                    timeout_ms,
                };

                let semaforo = self.governor.semaforo_miembro(miembro);
                let _permiso_proveedor = semaforo.acquire_owned().await.map_err(|_| {
                    CouncilError::SalidaInvalida {
                        miembro: miembro.etiqueta().to_string(),
                        detalle: "semáforo de protección de proveedor cerrado".to_string(),
                    }
                })?;

                match driver.ejecutar(&solicitud).await {
                    Ok(resp) => {
                        let consumo_cuota = self.governor.registrar_uso(miembro).await?;
                        let contenido_final =
                            PiiFirewall::rehidratar_respuesta(&resp.contenido, mapa_pii);
                        let marcadores = mapa_pii.len();
                        let hubo_fallback =
                            miembro != MiembroConsejo::ClaudePro || !intentos_fallidos.is_empty();

                        info!(
                            solicitud_id = %solicitud_id,
                            miembro = miembro.etiqueta(),
                            hubo_fallback,
                            marcadores_pii = marcadores,
                            latencia_ms = resp.latencia_ms,
                            "Delegación completada por miembro externo y rehidratada localmente"
                        );

                        return Ok(ResultadoDelegacion {
                            solicitud_id,
                            tarea: peticion.tarea,
                            miembro_respondedor: miembro,
                            contenido: contenido_final,
                            latencia_ms: inicio_total
                                .elapsed()
                                .as_millis()
                                .min(u64::MAX as u128)
                                as u64,
                            hubo_fallback,
                            intentos_fallidos,
                            pii_sanitizado: true,
                            marcadores_pii_aplicados: marcadores,
                            consumo_cuota,
                        });
                    }
                    Err(CouncilError::RateLimitExterno { detalle, .. }) => {
                        warn!(
                            miembro = miembro.etiqueta(),
                            detalle = %detalle,
                            "Rate-limit detectado; aplicando cooldown y continuando cascada"
                        );
                        let _ = self.governor.registrar_rate_limit(miembro, &detalle).await;
                        intentos_fallidos.push(IntentoFallidoConsejo {
                            miembro,
                            motivo: format!("rate-limit externo: {detalle}"),
                        });
                    }
                    Err(CouncilError::FalloSanitizacionPii(msg)) => {
                        return Err(CouncilError::FalloSanitizacionPii(msg));
                    }
                    Err(err) => {
                        warn!(
                            miembro = miembro.etiqueta(),
                            error = %err,
                            "Fallo de driver en cascada; escalando al siguiente miembro"
                        );
                        intentos_fallidos.push(IntentoFallidoConsejo {
                            miembro,
                            motivo: err.to_string(),
                        });
                    }
                }
            }
        }

        // Degradación soberana garantizada a BOB Local (`slot_id = 0` en VRAM).
        let bob_driver = self
            .drivers
            .get(&MiembroConsejo::BobLocal)
            .ok_or_else(|| CouncilError::CliNoDisponible {
                miembro: MiembroConsejo::BobLocal.etiqueta().to_string(),
                detalle: "driver de respaldo BobLocal no registrado en OrquestadorConsejo"
                    .to_string(),
            })?;

        let solicitud_bob = SolicitudConsejo {
            id: solicitud_id,
            tarea: peticion.tarea,
            prompt_sistema: peticion.tarea.prompt_sistema().to_string(),
            prompt_usuario: peticion.contexto,
            directorio_trabajo: peticion.directorio_trabajo,
            timeout_ms,
        };

        let resp_bob = bob_driver.ejecutar(&solicitud_bob).await?;
        let consumo_cuota = self
            .governor
            .registrar_uso(MiembroConsejo::BobLocal)
            .await?;
        let hubo_fallback = !peticion.forzar_local;

        info!(
            solicitud_id = %solicitud_id,
            miembro = MiembroConsejo::BobLocal.etiqueta(),
            hubo_fallback,
            latencia_ms = resp_bob.latencia_ms,
            "Delegación resuelta en BOB Local soberano (0 bytes PII enviados a la nube)"
        );

        Ok(ResultadoDelegacion {
            solicitud_id,
            tarea: peticion.tarea,
            miembro_respondedor: MiembroConsejo::BobLocal,
            contenido: resp_bob.contenido,
            latencia_ms: inicio_total.elapsed().as_millis().min(u64::MAX as u128) as u64,
            hubo_fallback,
            intentos_fallidos,
            pii_sanitizado: false,
            marcadores_pii_aplicados: 0,
            consumo_cuota,
        })
    }
}
