use std::cmp::Ordering;
use std::collections::BTreeSet;

use aurora_domain::{
    validar_activo, validar_prospecto, ActivoGenerado, AuroraError, EstadoAprobacion,
    TipoActivoGenerado, VerticalComercial,
};
use aurora_memory::{
    GatilloVigente, MemoryError, MotorMemoriaAurora, SubgrafoProspecto, VecinoCirculoCercano,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, info_span, warn};
use uuid::Uuid;

use crate::error::{CortexError, CortexResult};
use crate::grammar::ValidarRespuestaCortex;
use crate::roles::{CanalContacto, EscenaReel, RolCognitivo};

/// Plataforma pública permitida para interacciones periféricas de calentamiento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlataformaCalentamiento {
    /// Perfil profesional público en LinkedIn.
    LinkedIn,
    /// Cuenta pública empresarial o directiva en X.
    X,
    /// Perfil comercial público en Instagram.
    Instagram,
    /// Foro o directorio público de cámara empresarial.
    CamaraEmpresarial,
}

/// Naturaleza de la interacción periférica de valor previa al contacto directo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoInteraccionPeriferica {
    /// Comentario técnico aportando valor sin pitch agresivo.
    ComentarioValor,
    /// Reacción contextual sobre hito corporativo público.
    ReaccionContextual,
    /// Invitación a mesa redonda o briefing sectorial.
    InvitacionEvento,
}

/// Catálogo cerrado de herramientas deterministas que BOB puede invocar vía GBNF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", content = "params", deny_unknown_fields)]
pub enum AuroraToolCall {
    /// Consulta el subgrafo topológico 4D (1–2 saltos) de un prospecto en `aurora-memory`.
    ConsultarSubgrafo4D {
        /// Identificador canónico del prospecto.
        prospecto_id: Uuid,
        /// Profundidad máxima de expansión (`1` o `2` saltos).
        saltos_max: u8,
    },
    /// Registra un diagnóstico comercial validado contra el esquema de Aurora.
    EmitirDiagnosticoComercial {
        /// Identificador canónico del prospecto.
        prospecto_id: Uuid,
        /// Vertical y subvertical recomendadas.
        vertical: VerticalComercial,
        /// Producto o solución gancho inicial.
        producto_gancho: String,
        /// Dolor financiero/patrimonial principal detectado.
        dolor_principal: String,
        /// Probabilidad estimada en `[0.0, 1.0]`.
        probabilidad_cierre: f32,
        /// Justificación fundamentada en gatillos 4D y Círculo Cercano.
        justificacion_topologica: String,
    },
    /// Emite una orden de producción multimodal siempre en estado `PendienteGatekeeper`.
    SolicitarPaqueteMultimodal {
        /// Identificador canónico del prospecto.
        prospecto_id: Uuid,
        /// Gancho de apertura de primeros 3 segundos.
        gancho_3s: String,
        /// Guion locutado adaptado para síntesis TTS local.
        guion_tts: String,
        /// Secuencia de escenas para el reel vertical.
        escenas_reel: Vec<EscenaReel>,
        /// Índice de secciones del dossier ejecutivo PDF.
        secciones_dossier_pdf: Vec<String>,
        /// Canal de salida recomendado sujeto a aprobación humana.
        canal_sugerido: CanalContacto,
    },
    /// Programa una interacción periférica previa siempre en estado `PendienteGatekeeper`.
    ProgramarCalentamientoPeriferico {
        /// Identificador canónico del prospecto.
        prospecto_id: Uuid,
        /// Red o foro público donde ocurre la interacción.
        plataforma: PlataformaCalentamiento,
        /// Tipo de interacción de calentamiento.
        tipo_interaccion: TipoInteraccionPeriferica,
        /// Contexto público de respaldo para auditoría LFPDPPP.
        contexto_publico: String,
    },
}

fn validar_no_vacio(campo: &'static str, valor: &str, rol: RolCognitivo) -> CortexResult<()> {
    if valor.trim().is_empty() {
        return Err(CortexError::ViolacionEsquema {
            rol: rol.etiqueta().to_string(),
            detalle: format!("el campo '{campo}' no puede estar vacío"),
        });
    }
    Ok(())
}

impl ValidarRespuestaCortex for AuroraToolCall {
    fn validar_semantica(&self, rol: RolCognitivo) -> CortexResult<()> {
        match self {
            Self::ConsultarSubgrafo4D {
                prospecto_id,
                saltos_max,
            } => {
                if prospecto_id.is_nil() {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: "prospecto_id en ConsultarSubgrafo4D no puede ser UUID nil"
                            .to_string(),
                    });
                }
                if !(1..=2).contains(saltos_max) {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: format!(
                            "saltos_max={saltos_max} fuera de rango permitido [1, 2]"
                        ),
                    });
                }
            }
            Self::EmitirDiagnosticoComercial {
                prospecto_id,
                vertical: _,
                producto_gancho,
                dolor_principal,
                probabilidad_cierre,
                justificacion_topologica,
            } => {
                if prospecto_id.is_nil() {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: "prospecto_id en EmitirDiagnosticoComercial no puede ser UUID nil"
                            .to_string(),
                    });
                }
                validar_no_vacio("producto_gancho", producto_gancho, rol)?;
                validar_no_vacio("dolor_principal", dolor_principal, rol)?;
                validar_no_vacio("justificacion_topologica", justificacion_topologica, rol)?;
                if !probabilidad_cierre.is_finite() || !(0.0..=1.0).contains(probabilidad_cierre) {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: format!(
                            "probabilidad_cierre={probabilidad_cierre} fuera de [0.0, 1.0]"
                        ),
                    });
                }
            }
            Self::SolicitarPaqueteMultimodal {
                prospecto_id,
                gancho_3s,
                guion_tts,
                escenas_reel,
                secciones_dossier_pdf,
                canal_sugerido: _,
            } => {
                if prospecto_id.is_nil() {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: "prospecto_id en SolicitarPaqueteMultimodal no puede ser UUID nil"
                            .to_string(),
                    });
                }
                validar_no_vacio("gancho_3s", gancho_3s, rol)?;
                validar_no_vacio("guion_tts", guion_tts, rol)?;
                if escenas_reel.is_empty() {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: "escenas_reel debe contener al menos una escena".to_string(),
                    });
                }
                for escena in escenas_reel {
                    if escena.indice == 0 || escena.duracion_segundos == 0 {
                        return Err(CortexError::ViolacionEsquema {
                            rol: rol.etiqueta().to_string(),
                            detalle: "cada escena requiere indice >= 1 y duracion_segundos >= 1"
                                .to_string(),
                        });
                    }
                    validar_no_vacio("escena.descripcion_visual", &escena.descripcion_visual, rol)?;
                    validar_no_vacio("escena.texto_overlay", &escena.texto_overlay, rol)?;
                }
                if secciones_dossier_pdf.is_empty()
                    || secciones_dossier_pdf.iter().any(|s| s.trim().is_empty())
                {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle: "secciones_dossier_pdf debe incluir secciones no vacías"
                            .to_string(),
                    });
                }
            }
            Self::ProgramarCalentamientoPeriferico {
                prospecto_id,
                plataforma: _,
                tipo_interaccion: _,
                contexto_publico,
            } => {
                if prospecto_id.is_nil() {
                    return Err(CortexError::ViolacionEsquema {
                        rol: rol.etiqueta().to_string(),
                        detalle:
                            "prospecto_id en ProgramarCalentamientoPeriferico no puede ser UUID nil"
                                .to_string(),
                    });
                }
                validar_no_vacio("contexto_publico", contexto_publico, rol)?;
            }
        }
        Ok(())
    }
}

/// Diagnóstico comercial validado resultante de `EmitirDiagnosticoComercial`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticoToolValidado {
    /// Identificador de la campaña asociada.
    pub campana_id: Uuid,
    /// Identificador del prospecto evaluado.
    pub prospecto_id: Uuid,
    /// Vertical comercial asignada.
    pub vertical: VerticalComercial,
    /// Producto gancho validado.
    pub producto_gancho: String,
    /// Dolor principal detectado.
    pub dolor_principal: String,
    /// Probabilidad estimada en `[0.0, 1.0]`.
    pub probabilidad_cierre: f32,
    /// Justificación topológica.
    pub justificacion_topologica: String,
    /// Marca temporal UTC de emisión.
    pub emitido_en: DateTime<Utc>,
}

/// Datos base necesarios para emitir una `OrdenProduccionActivo` bloqueada en Gatekeeper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParametrosOrdenProduccion {
    /// Campaña comercial asociada.
    pub campana_id: Uuid,
    /// Prospecto destinatario validado.
    pub prospecto_id: Uuid,
    /// Gancho de apertura de primeros 3 segundos.
    pub gancho_3s: String,
    /// Guion locutado para TTS.
    pub guion_tts: String,
    /// Secuencia de escenas para el reel vertical.
    pub escenas_reel: Vec<EscenaReel>,
    /// Secciones del dossier PDF.
    pub secciones_dossier_pdf: Vec<String>,
    /// Canal de contacto sugerido.
    pub canal_sugerido: CanalContacto,
}

/// Orden formal de producción multimodal lista para ser consumida por `aurora-factory` (Fase 4).
///
/// Por invariante arquitectónica, su estado y todos sus activos nacen estrictamente en
/// [`EstadoAprobacion::PendienteGatekeeper`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdenProduccionActivo {
    /// Identificador único de la orden.
    pub orden_id: Uuid,
    /// Campaña comercial asociada.
    pub campana_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Gancho de los primeros 3 segundos.
    pub gancho_3s: String,
    /// Guion locutado para síntesis de voz.
    pub guion_tts: String,
    /// Escenas ordenadas del reel vertical.
    pub escenas_reel: Vec<EscenaReel>,
    /// Secciones planificadas para el dossier PDF.
    pub secciones_dossier_pdf: Vec<String>,
    /// Canal de contacto recomendado al operador humano.
    pub canal_sugerido: CanalContacto,
    /// Fecha de emisión UTC.
    pub creada_en: DateTime<Utc>,
    estado_gatekeeper: EstadoAprobacion,
    activos_iniciales: Vec<ActivoGenerado>,
}

impl OrdenProduccionActivo {
    /// Construye una orden multimodal bloqueada por diseño en `EstadoAprobacion::PendienteGatekeeper`.
    pub fn nueva_pendiente(params: ParametrosOrdenProduccion) -> CortexResult<Self> {
        let ParametrosOrdenProduccion {
            campana_id,
            prospecto_id,
            gancho_3s,
            guion_tts,
            escenas_reel,
            secciones_dossier_pdf,
            canal_sugerido,
        } = params;

        let ahora = Utc::now();
        let orden_id = Uuid::new_v4();

        let specs = [
            (
                TipoActivoGenerado::DossierPdf,
                format!("pendiente://factory/dossier_{orden_id}.pdf"),
            ),
            (
                TipoActivoGenerado::NotaVozTts,
                format!("pendiente://factory/voz_{orden_id}.wav"),
            ),
            (
                TipoActivoGenerado::ReelVerticalMp4,
                format!("pendiente://factory/reel_{orden_id}.mp4"),
            ),
            (TipoActivoGenerado::MensajeDirecto, gancho_3s.clone()),
        ];

        let mut activos_iniciales = Vec::with_capacity(specs.len());
        for (tipo, contenido_o_ruta) in specs {
            let activo = ActivoGenerado {
                id: Uuid::new_v4(),
                campana_id,
                prospecto_id,
                tipo,
                estado: EstadoAprobacion::PendienteGatekeeper,
                contenido_o_ruta,
                creado_en: ahora,
                actualizado_en: ahora,
                motivo_rechazo: None,
            };
            validar_activo(&activo)?;
            activos_iniciales.push(activo);
        }

        Ok(Self {
            orden_id,
            campana_id,
            prospecto_id,
            gancho_3s,
            guion_tts,
            escenas_reel,
            secciones_dossier_pdf,
            canal_sugerido,
            creada_en: ahora,
            estado_gatekeeper: EstadoAprobacion::PendienteGatekeeper,
            activos_iniciales,
        })
    }

    /// Devuelve el estado obligatorio de Gatekeeper para la orden.
    #[must_use]
    pub const fn estado_gatekeeper(&self) -> EstadoAprobacion {
        self.estado_gatekeeper
    }

    /// Devuelve los 4 activos iniciales registrados en espera de aprobación humana.
    #[must_use]
    pub fn activos_iniciales(&self) -> &[ActivoGenerado] {
        &self.activos_iniciales
    }

    /// Verifica explícitamente que ni la orden ni ningún activo hayan saltado el Gatekeeper.
    pub fn verificar_invariante_gatekeeper(&self) -> CortexResult<()> {
        if self.estado_gatekeeper != EstadoAprobacion::PendienteGatekeeper {
            return Err(CortexError::BloqueoGatekeeper {
                prospecto_id: self.prospecto_id,
                motivo: "toda orden emitida por Cortex debe iniciar en PendienteGatekeeper"
                    .to_string(),
            });
        }
        for activo in &self.activos_iniciales {
            if activo.estado != EstadoAprobacion::PendienteGatekeeper {
                return Err(CortexError::BloqueoGatekeeper {
                    prospecto_id: self.prospecto_id,
                    motivo: format!(
                        "el activo {} no respeta EstadoAprobacion::PendienteGatekeeper",
                        activo.id
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Solicitud de calentamiento periférico supervisado por el Gatekeeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalentamientoPendiente {
    /// Identificador único de la solicitud.
    pub calentamiento_id: Uuid,
    /// Campaña comercial asociada.
    pub campana_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Plataforma pública seleccionada.
    pub plataforma: PlataformaCalentamiento,
    /// Tipo de interacción de valor.
    pub tipo_interaccion: TipoInteraccionPeriferica,
    /// Evidencia/contexto público asociado.
    pub contexto_publico: String,
    /// Fecha de creación UTC.
    pub creado_en: DateTime<Utc>,
    estado_gatekeeper: EstadoAprobacion,
}

impl CalentamientoPendiente {
    /// Crea una interacción periférica bloqueada en `EstadoAprobacion::PendienteGatekeeper`.
    #[must_use]
    pub fn nueva_pendiente(
        campana_id: Uuid,
        prospecto_id: Uuid,
        plataforma: PlataformaCalentamiento,
        tipo_interaccion: TipoInteraccionPeriferica,
        contexto_publico: String,
    ) -> Self {
        Self {
            calentamiento_id: Uuid::new_v4(),
            campana_id,
            prospecto_id,
            plataforma,
            tipo_interaccion,
            contexto_publico,
            creado_en: Utc::now(),
            estado_gatekeeper: EstadoAprobacion::PendienteGatekeeper,
        }
    }

    /// Devuelve el estado obligatorio de Gatekeeper.
    #[must_use]
    pub const fn estado_gatekeeper(&self) -> EstadoAprobacion {
        self.estado_gatekeeper
    }
}

/// Resultado tipado emitido tras ejecutar deterministamente una `AuroraToolCall`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResultadoEjecucionTool {
    /// Subgrafo 4D recuperado directamente desde `aurora-memory`.
    SubgrafoObtenido(Box<SubgrafoProspecto>),
    /// Diagnóstico comercial validado contra reglas de dominio.
    DiagnosticoValidado(Box<DiagnosticoToolValidado>),
    /// Orden multimodal generada en estado `PendienteGatekeeper`.
    OrdenProduccion(Box<OrdenProduccionActivo>),
    /// Calentamiento periférico registrado en estado `PendienteGatekeeper`.
    CalentamientoProgramado(Box<CalentamientoPendiente>),
}

/// Despachador determinista que ejecuta herramientas de BOB validando LFPDPPP y Gatekeeper.
#[derive(Debug, Default, Clone, Copy)]
pub struct ToolDispatcher;

impl ToolDispatcher {
    /// Verifica que el prospecto exista, no esté en blacklist y no tenga `opt_out` LFPDPPP activo.
    pub fn verificar_habilitacion_lfpdppp(
        prospecto_id: Uuid,
        memoria: &MotorMemoriaAurora,
    ) -> CortexResult<()> {
        if memoria.esta_en_blacklist(prospecto_id) {
            warn!(prospecto_id = %prospecto_id, "Gatekeeper bloqueó prospecto presente en blacklist LFPDPPP");
            return Err(CortexError::BloqueoGatekeeper {
                prospecto_id,
                motivo: "prospecto incluido en blacklist de purga física LFPDPPP".to_string(),
            });
        }

        let Some(registro) = memoria.grafo().obtener_por_uuid(prospecto_id) else {
            return Err(CortexError::Memoria(MemoryError::ProspectoNoEncontrado(
                prospecto_id,
            )));
        };

        if registro.prospecto.lfpdppp.opt_out {
            warn!(prospecto_id = %prospecto_id, "Gatekeeper bloqueó prospecto con opt_out=true");
            return Err(CortexError::BloqueoGatekeeper {
                prospecto_id,
                motivo: "bandera LFPDPPP opt_out activa en el nodo del prospecto".to_string(),
            });
        }

        validar_prospecto(&registro.prospecto).map_err(|err| match err {
            AuroraError::OptOutActive { prospecto_id: pid } => CortexError::BloqueoGatekeeper {
                prospecto_id: pid,
                motivo: "exclusión ARCO/LFPDPPP activa".to_string(),
            },
            otro => CortexError::Dominio(otro),
        })?;

        Ok(())
    }

    /// Ejecuta una llamada de herramienta previamente restringida por GBNF.
    #[tracing::instrument(skip(self, memoria), fields(campana_id = %campana_id))]
    pub fn ejecutar(
        &self,
        llamado: &AuroraToolCall,
        campana_id: Uuid,
        memoria: &MotorMemoriaAurora,
    ) -> CortexResult<ResultadoEjecucionTool> {
        let _span = info_span!("tool_dispatcher_ejecutar", campana_id = %campana_id).entered();
        llamado.validar_semantica(RolCognitivo::DirectorCreativo)?;

        let prospecto_id = match llamado {
            AuroraToolCall::ConsultarSubgrafo4D { prospecto_id, .. }
            | AuroraToolCall::EmitirDiagnosticoComercial { prospecto_id, .. }
            | AuroraToolCall::SolicitarPaqueteMultimodal { prospecto_id, .. }
            | AuroraToolCall::ProgramarCalentamientoPeriferico { prospecto_id, .. } => {
                *prospecto_id
            }
        };

        Self::verificar_habilitacion_lfpdppp(prospecto_id, memoria)?;

        match llamado {
            AuroraToolCall::ConsultarSubgrafo4D {
                prospecto_id,
                saltos_max,
            } => {
                let subgrafo = reconstruir_subgrafo_4d(*prospecto_id, *saltos_max, memoria)?;
                info!(
                    prospecto_id = %prospecto_id,
                    vecinos = subgrafo.vecinos_circulo_cercano.len(),
                    "Subgrafo 4D recuperado por ToolDispatcher"
                );
                Ok(ResultadoEjecucionTool::SubgrafoObtenido(Box::new(subgrafo)))
            }
            AuroraToolCall::EmitirDiagnosticoComercial {
                prospecto_id,
                vertical,
                producto_gancho,
                dolor_principal,
                probabilidad_cierre,
                justificacion_topologica,
            } => {
                let diagnostico = DiagnosticoToolValidado {
                    campana_id,
                    prospecto_id: *prospecto_id,
                    vertical: *vertical,
                    producto_gancho: producto_gancho.clone(),
                    dolor_principal: dolor_principal.clone(),
                    probabilidad_cierre: *probabilidad_cierre,
                    justificacion_topologica: justificacion_topologica.clone(),
                    emitido_en: Utc::now(),
                };
                info!(
                    prospecto_id = %prospecto_id,
                    probabilidad_cierre,
                    "Diagnóstico comercial validado por ToolDispatcher"
                );
                Ok(ResultadoEjecucionTool::DiagnosticoValidado(Box::new(
                    diagnostico,
                )))
            }
            AuroraToolCall::SolicitarPaqueteMultimodal {
                prospecto_id,
                gancho_3s,
                guion_tts,
                escenas_reel,
                secciones_dossier_pdf,
                canal_sugerido,
            } => {
                let orden = OrdenProduccionActivo::nueva_pendiente(ParametrosOrdenProduccion {
                    campana_id,
                    prospecto_id: *prospecto_id,
                    gancho_3s: gancho_3s.clone(),
                    guion_tts: guion_tts.clone(),
                    escenas_reel: escenas_reel.clone(),
                    secciones_dossier_pdf: secciones_dossier_pdf.clone(),
                    canal_sugerido: *canal_sugerido,
                })?;
                orden.verificar_invariante_gatekeeper()?;
                info!(
                    prospecto_id = %prospecto_id,
                    orden_id = %orden.orden_id,
                    activos = orden.activos_iniciales().len(),
                    "OrdenProduccionActivo emitida en PendienteGatekeeper"
                );
                Ok(ResultadoEjecucionTool::OrdenProduccion(Box::new(orden)))
            }
            AuroraToolCall::ProgramarCalentamientoPeriferico {
                prospecto_id,
                plataforma,
                tipo_interaccion,
                contexto_publico,
            } => {
                let calentamiento = CalentamientoPendiente::nueva_pendiente(
                    campana_id,
                    *prospecto_id,
                    *plataforma,
                    *tipo_interaccion,
                    contexto_publico.clone(),
                );
                info!(
                    prospecto_id = %prospecto_id,
                    calentamiento_id = %calentamiento.calentamiento_id,
                    "CalentamientoPeriferico emitido en PendienteGatekeeper"
                );
                Ok(ResultadoEjecucionTool::CalentamientoProgramado(Box::new(
                    calentamiento,
                )))
            }
        }
    }
}

fn reconstruir_subgrafo_4d(
    prospecto_id: Uuid,
    saltos_max: u8,
    memoria: &MotorMemoriaAurora,
) -> CortexResult<SubgrafoProspecto> {
    let grafo = memoria.grafo();
    let registro = grafo
        .obtener_por_uuid(prospecto_id)
        .ok_or(CortexError::Memoria(MemoryError::ProspectoNoEncontrado(
            prospecto_id,
        )))?;

    let ahora = Utc::now();
    let score_temporal_4d = grafo.relevancia_temporal_nodo(registro.node_id, ahora);
    let (boost_circulo_cercano, vecinos_prop) =
        grafo.propagar_senal_circulo_cercano(registro.node_id, ahora, usize::from(saltos_max));

    let mut gatillos_vigentes: Vec<GatilloVigente> = registro
        .prospecto
        .gatillos
        .iter()
        .map(|g| {
            let peso_vigente_4d = grafo.peso_gatillo_vigente(g, ahora);
            let antiguedad_horas = ((ahora - g.ocurrido_en).num_seconds().max(0) as f64) / 3600.0;
            GatilloVigente {
                gatillo: g.clone(),
                peso_vigente_4d,
                antiguedad_horas,
            }
        })
        .collect();
    gatillos_vigentes.sort_by(|a, b| {
        b.peso_vigente_4d
            .partial_cmp(&a.peso_vigente_4d)
            .unwrap_or(Ordering::Equal)
    });

    let mut clusteres_set = BTreeSet::new();
    for alias in &registro.prospecto.aliases {
        clusteres_set.insert(alias.clone());
    }

    let vecinos_circulo_cercano: Vec<VecinoCirculoCercano> = vecinos_prop
        .into_iter()
        .map(|v| {
            clusteres_set.insert(v.contexto_compartido.clone());
            VecinoCirculoCercano {
                prospecto_id: v.prospecto_id,
                nombre_normalizado: v.nombre_normalizado,
                tipo_vinculo: v.tipo_vinculo,
                fuerza_vinculo: v.fuerza_ruta,
                contexto_compartido: v.contexto_compartido,
                salto: v.salto,
                relevancia_propia_4d: v.relevancia_propia_4d,
            }
        })
        .collect();

    let score_compuesto = (0.5 * score_temporal_4d + 0.5 * boost_circulo_cercano).clamp(0.0, 1.5);

    Ok(SubgrafoProspecto {
        prospecto: registro.prospecto.clone(),
        isla: registro.isla,
        score_compuesto,
        similitud_vectorial: 1.0,
        score_temporal_4d,
        boost_circulo_cercano,
        gatillos_vigentes,
        vecinos_circulo_cercano,
        clusteres_afines: clusteres_set.into_iter().collect(),
    })
}
