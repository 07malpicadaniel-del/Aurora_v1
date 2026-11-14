use std::collections::{BTreeMap, HashMap, HashSet};

use aurora_cortex::MetricasCampana;
use aurora_memory::{IslaVertical, MotorMemoriaAurora};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::dispatcher::{ComprobanteDespacho, EstadoCampanaProspecto};
use crate::error::{PulseError, PulseResult};

/// Tipos de señal observables tras el despacho comercial multicanal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TipoEventoTelemetria {
    /// Impresión registrada de un Reel vertical o pieza audiovisual.
    ImpresionReel,
    /// Medición de retención sobre Reel vertical en segundos reproducidos.
    RetencionReel {
        /// Segundos efectivos observados por el usuario.
        segundos_vistos: f32,
        /// Duración total del Reel reproducido.
        duracion_total_s: f32,
        /// Indica si la reproducción llegó al final del activo.
        completado: bool,
    },
    /// El prospecto guardó la pieza/Reel o reenvió internamente el material.
    GuardadoArtefacto,
    /// El prospecto abrió el Dossier PDF o la landing personalizada de propuesta.
    AperturaDossierPdf,
    /// Respuesta comercial positiva o solicitud explícita de información/reunión.
    RespuestaPositiva {
        /// Extracto resumido no sensible de la respuesta.
        texto_resumen: String,
    },
    /// Rebote técnico o entrega rechazada por el canal externo.
    ReboteEntrega {
        /// Motivo diagnóstico reportado por el transporte.
        motivo: String,
    },
    /// Solicitud expresa ARCO / `opt_out` que detiene campañas y dispara purga LFPDPPP.
    SolicitudBajaOptOut {
        /// Motivo o canal por el que se recibió la solicitud de baja.
        motivo: String,
    },
}

/// Evento individual capturado por el bus de telemetría comercial de Aurora Pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventoTelemetria {
    /// Identificador único del evento.
    pub evento_id: Uuid,
    /// Campaña a la que pertenece la interacción.
    pub campana_id: Uuid,
    /// Prospecto asociado al envío.
    pub prospecto_id: Uuid,
    /// Identificador único de tracking asignado en [`ComprobanteDespacho`].
    pub tracking_id: Uuid,
    /// Isla vectorial vertical para retroalimentación circadiana 4D.
    pub isla: Option<IslaVertical>,
    /// Gancho creativo de 3 segundos utilizado en la pieza evaluada.
    pub gancho_usado: String,
    /// Resumen o guion asociado al gancho para construir pares `chosen`/`rejected` en DPO.
    pub guion_resumen: String,
    /// Tipo de señal registrada.
    pub tipo: TipoEventoTelemetria,
    /// Marca temporal UTC de captura.
    pub registrado_en: DateTime<Utc>,
}

impl EventoTelemetria {
    /// Constructor ergonómico de eventos de telemetría con sello UTC actual.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn nuevo(
        campana_id: Uuid,
        prospecto_id: Uuid,
        tracking_id: Uuid,
        isla: Option<IslaVertical>,
        gancho_usado: impl Into<String>,
        guion_resumen: impl Into<String>,
        tipo: TipoEventoTelemetria,
    ) -> Self {
        Self {
            evento_id: Uuid::new_v4(),
            campana_id,
            prospecto_id,
            tracking_id,
            isla,
            gancho_usado: gancho_usado.into(),
            guion_resumen: guion_resumen.into(),
            tipo,
            registrado_en: Utc::now(),
        }
    }
}

/// Estadística consolidada por gancho creativo para auditoría y dataset DPO.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumenGanchoTelemetria {
    /// Texto del gancho creativo utilizado.
    pub gancho: String,
    /// Resumen del guion/ángulo asociado.
    pub guion_resumen: String,
    /// Campaña origen de las métricas.
    pub campana_id: Uuid,
    /// Isla vertical objetivo (si fue informada).
    pub isla: Option<IslaVertical>,
    /// Número total de eventos capturados para este gancho.
    pub muestras: usize,
    /// Prospectos vinculados al gancho para refuerzo/atenuación 4D nocturna.
    pub prospectos_asociados: Vec<Uuid>,
    /// Promedio normalizado `[0.0, 1.0]` de retención observada en Reels.
    pub tasa_retencion_promedio: f32,
    /// Tasa agregada `[0.0, 1.0]` de aperturas, guardados y respuestas positivas.
    pub tasa_apertura_o_respuesta: f32,
    /// Indica si el gancho supera el umbral de alto desempeño para DPO `chosen`.
    pub es_alto_rendimiento: bool,
}

#[derive(Debug, Clone)]
struct EstadoProspectoTelemetria {
    estado: EstadoCampanaProspecto,
    abrio_dossier: bool,
    retencion_3s_alcanzada: bool,
    guardado: bool,
    respuesta_positiva: bool,
    rebotado: bool,
    opt_out: bool,
}

impl Default for EstadoProspectoTelemetria {
    fn default() -> Self {
        Self {
            estado: EstadoCampanaProspecto::Pendiente,
            abrio_dossier: false,
            retencion_3s_alcanzada: false,
            guardado: false,
            respuesta_positiva: false,
            rebotado: false,
            opt_out: false,
        }
    }
}

#[derive(Debug, Clone)]
struct AcumuladorGancho {
    gancho: String,
    guion_resumen: String,
    campana_id: Uuid,
    isla: Option<IslaVertical>,
    eventos_totales: usize,
    impresiones_o_despachos: usize,
    muestras_retencion: usize,
    suma_ratio_retencion: f32,
    aperturas_y_guardados: usize,
    respuestas_positivas: usize,
    prospectos: HashSet<Uuid>,
}

impl AcumuladorGancho {
    fn new(
        gancho: String,
        guion_resumen: String,
        campana_id: Uuid,
        isla: Option<IslaVertical>,
    ) -> Self {
        Self {
            gancho,
            guion_resumen,
            campana_id,
            isla,
            eventos_totales: 0,
            impresiones_o_despachos: 0,
            muestras_retencion: 0,
            suma_ratio_retencion: 0.0,
            aperturas_y_guardados: 0,
            respuestas_positivas: 0,
            prospectos: HashSet::new(),
        }
    }

    fn resumen(&self) -> ResumenGanchoTelemetria {
        let tasa_retencion_promedio = if self.muestras_retencion > 0 {
            (self.suma_ratio_retencion / self.muestras_retencion as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let denominador_interaccion = self
            .impresiones_o_despachos
            .max(self.prospectos.len())
            .max(self.muestras_retencion)
            .max(1) as f32;
        let interacciones_positivas =
            (self.aperturas_y_guardados + self.respuestas_positivas) as f32;
        let tasa_apertura_o_respuesta =
            (interacciones_positivas / denominador_interaccion).clamp(0.0, 1.0);

        let mut prospectos_asociados: Vec<Uuid> = self.prospectos.iter().copied().collect();
        prospectos_asociados.sort();

        let es_alto_rendimiento =
            tasa_retencion_promedio >= 0.50 || self.respuestas_positivas > 0;

        ResumenGanchoTelemetria {
            gancho: self.gancho.clone(),
            guion_resumen: self.guion_resumen.clone(),
            campana_id: self.campana_id,
            isla: self.isla,
            muestras: self.eventos_totales.max(self.prospectos.len()),
            prospectos_asociados,
            tasa_retencion_promedio,
            tasa_apertura_o_respuesta,
            es_alto_rendimiento,
        }
    }
}

#[derive(Debug, Default)]
struct RegistroCampana {
    isla: Option<IslaVertical>,
    prospectos: HashMap<Uuid, EstadoProspectoTelemetria>,
    trackings: HashMap<Uuid, Uuid>,
    ganchos: BTreeMap<String, AcumuladorGancho>,
}

#[derive(Debug, Default)]
struct EstadoInternoTelemetria {
    campanas: HashMap<Uuid, RegistroCampana>,
    prospectos_detenidos: HashMap<Uuid, String>,
    historial_eventos: Vec<EventoTelemetria>,
}

/// Recolector de telemetría comercial con soporte inmediato de bajas ARCO/LFPDPPP.
#[derive(Debug, Default)]
pub struct RecolectorTelemetria {
    estado: Mutex<EstadoInternoTelemetria>,
}

impl RecolectorTelemetria {
    /// Crea un recolector de telemetría vacío en memoria.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un comprobante de despacho para inicializar tracking y métricas de campaña.
    pub fn registrar_despacho(
        &self,
        comprobante: &ComprobanteDespacho,
        isla: Option<IslaVertical>,
        gancho_usado: impl Into<String>,
        guion_resumen: impl Into<String>,
    ) -> PulseResult<()> {
        let gancho = gancho_usado.into();
        let guion = guion_resumen.into();
        let gancho_normalizado = normalizar_etiqueta_gancho(&gancho);

        let mut guard = self.estado.lock();
        if guard
            .prospectos_detenidos
            .contains_key(&comprobante.prospecto_id)
        {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: comprobante.prospecto_id,
            });
        }

        let campana = guard.campanas.entry(comprobante.campana_id).or_default();
        if campana.isla.is_none() {
            campana.isla = isla;
        }
        campana
            .trackings
            .insert(comprobante.tracking_id, comprobante.prospecto_id);

        let reg_prospecto = campana
            .prospectos
            .entry(comprobante.prospecto_id)
            .or_default();
        if reg_prospecto.estado == EstadoCampanaProspecto::Pendiente
            || reg_prospecto.estado == EstadoCampanaProspecto::Calentando
        {
            reg_prospecto.estado = comprobante.estado_campana;
        }

        if let Some(clave) = gancho_normalizado {
            let acum = campana
                .ganchos
                .entry(clave)
                .or_insert_with(|| AcumuladorGancho::new(gancho, guion, comprobante.campana_id, isla));
            acum.impresiones_o_despachos += 1;
            acum.prospectos.insert(comprobante.prospecto_id);
        }

        Ok(())
    }

    /// Ingiere un evento de telemetría actualizando el estado del prospecto y métricas del gancho.
    ///
    /// Si el prospecto ya fue dado de baja por `opt_out`, cualquier evento posterior distinto de
    /// `SolicitudBajaOptOut` se rechaza con [`PulseError::ProspectoOptOut`].
    pub fn registrar_evento(
        &self,
        evento: EventoTelemetria,
    ) -> PulseResult<EstadoCampanaProspecto> {
        validar_evento_telemetria(&evento)?;

        let mut guard = self.estado.lock();
        let es_solicitud_baja = matches!(evento.tipo, TipoEventoTelemetria::SolicitudBajaOptOut { .. });

        if !es_solicitud_baja && guard.prospectos_detenidos.contains_key(&evento.prospecto_id) {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: evento.prospecto_id,
            });
        }

        if let TipoEventoTelemetria::SolicitudBajaOptOut { ref motivo } = evento.tipo {
            guard
                .prospectos_detenidos
                .insert(evento.prospecto_id, motivo.clone());
            // Excluimos al prospecto en todas sus campañas registradas.
            for campana in guard.campanas.values_mut() {
                if let Some(reg) = campana.prospectos.get_mut(&evento.prospecto_id) {
                    reg.opt_out = true;
                    reg.estado = EstadoCampanaProspecto::ExcluidoOptOut;
                }
            }
        }

        let nuevo_estado = {
            let campana = guard.campanas.entry(evento.campana_id).or_default();
            if campana.isla.is_none() {
                campana.isla = evento.isla;
            }
            campana
                .trackings
                .insert(evento.tracking_id, evento.prospecto_id);

            let isla_campana = campana.isla;
            let estado_prospecto = campana.prospectos.entry(evento.prospecto_id).or_default();
            aplicar_evento_a_prospecto(estado_prospecto, &evento.tipo);
            let estado_resultante = estado_prospecto.estado;

            if !es_solicitud_baja {
                if let Some(clave) = normalizar_etiqueta_gancho(&evento.gancho_usado) {
                    let acum = campana.ganchos.entry(clave).or_insert_with(|| {
                        AcumuladorGancho::new(
                            evento.gancho_usado.clone(),
                            evento.guion_resumen.clone(),
                            evento.campana_id,
                            evento.isla.or(isla_campana),
                        )
                    });
                    aplicar_evento_a_gancho(acum, &evento);
                }
            }

            estado_resultante
        };

        guard.historial_eventos.push(evento);
        Ok(nuevo_estado)
    }

    /// Registra un evento y, en caso de [`TipoEventoTelemetria::SolicitudBajaOptOut`], ejecuta de
    /// inmediato la purga física LFPDPPP en [`MotorMemoriaAurora`].
    pub fn registrar_evento_con_memoria(
        &self,
        evento: EventoTelemetria,
        memoria: &mut MotorMemoriaAurora,
    ) -> PulseResult<EstadoCampanaProspecto> {
        if let TipoEventoTelemetria::SolicitudBajaOptOut { ref motivo } = evento.tipo {
            let prospecto_id = evento.prospecto_id;
            let campana_id = evento.campana_id;
            let motivo_clon = motivo.clone();
            let estado = self.registrar_evento(evento)?;
            self.aplicar_purga_en_memoria(prospecto_id, Some(campana_id), &motivo_clon, memoria)?;
            return Ok(estado);
        }

        self.registrar_evento(evento)
    }

    /// Detiene inmediatamente campañas activas para `prospecto_id` y purga físicamente sus datos
    /// e hipervectores en el WAL y grafo 4D de [`MotorMemoriaAurora`].
    #[tracing::instrument(skip(self, memoria), fields(prospecto_id = %prospecto_id))]
    pub fn procesar_solicitud_opt_out(
        &self,
        prospecto_id: Uuid,
        campana_id: Option<Uuid>,
        motivo: &str,
        memoria: &mut MotorMemoriaAurora,
    ) -> PulseResult<()> {
        let motivo_limpio = if motivo.trim().is_empty() {
            "Solicitud ARCO / opt_out recibida vía telemetría".to_string()
        } else {
            motivo.trim().to_string()
        };

        {
            let mut guard = self.estado.lock();
            guard
                .prospectos_detenidos
                .insert(prospecto_id, motivo_limpio.clone());

            for campana in guard.campanas.values_mut() {
                if let Some(reg) = campana.prospectos.get_mut(&prospecto_id) {
                    reg.opt_out = true;
                    reg.estado = EstadoCampanaProspecto::ExcluidoOptOut;
                }
            }

            if let Some(cid) = campana_id {
                let campana = guard.campanas.entry(cid).or_default();
                let reg = campana.prospectos.entry(prospecto_id).or_default();
                reg.opt_out = true;
                reg.estado = EstadoCampanaProspecto::ExcluidoOptOut;
            }
        }

        self.aplicar_purga_en_memoria(prospecto_id, campana_id, &motivo_limpio, memoria)
    }

    fn aplicar_purga_en_memoria(
        &self,
        prospecto_id: Uuid,
        campana_id: Option<Uuid>,
        motivo: &str,
        memoria: &mut MotorMemoriaAurora,
    ) -> PulseResult<()> {
        if memoria.grafo().obtener_por_uuid(prospecto_id).is_some()
            || !memoria.esta_en_blacklist(prospecto_id)
        {
            memoria.purgar_prospecto_opt_out(prospecto_id)?;
        }

        warn!(
            %prospecto_id,
            ?campana_id,
            motivo,
            "LFPDPPP opt_out ejecutado: prospecto detenido y purgado físicamente de WAL/HNSW"
        );
        Ok(())
    }

    /// Indica si el prospecto quedó detenido irrevocablemente por solicitud `opt_out`.
    #[must_use]
    pub fn esta_prospecto_detenido(&self, prospecto_id: Uuid) -> bool {
        self.estado
            .lock()
            .prospectos_detenidos
            .contains_key(&prospecto_id)
    }

    /// Devuelve el estado de campaña actual para un prospecto dentro de una campaña.
    #[must_use]
    pub fn estado_prospecto(
        &self,
        campana_id: Uuid,
        prospecto_id: Uuid,
    ) -> Option<EstadoCampanaProspecto> {
        let guard = self.estado.lock();
        if guard.prospectos_detenidos.contains_key(&prospecto_id) {
            return Some(EstadoCampanaProspecto::ExcluidoOptOut);
        }
        guard
            .campanas
            .get(&campana_id)
            .and_then(|c| c.prospectos.get(&prospecto_id))
            .map(|p| p.estado)
    }

    /// Lista ordenada determinísticamente de IDs de campaña con telemetría registrada.
    #[must_use]
    pub fn campanas_registradas(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.estado.lock().campanas.keys().copied().collect();
        ids.sort();
        ids
    }

    /// Devuelve la [`IslaVertical`] asociada a una campaña, si fue informada.
    #[must_use]
    pub fn isla_campana(&self, campana_id: Uuid) -> Option<IslaVertical> {
        self.estado
            .lock()
            .campanas
            .get(&campana_id)
            .and_then(|c| c.isla)
    }

    /// Número total de eventos históricos almacenados.
    #[must_use]
    pub fn total_eventos(&self) -> usize {
        self.estado.lock().historial_eventos.len()
    }

    /// Consolida las métricas agregadas de una campaña en el contrato [`MetricasCampana`]
    /// consumido por el rol [`aurora_cortex::RolCognitivo::Auditor`].
    pub fn generar_metricas_campana(&self, campana_id: Uuid) -> PulseResult<MetricasCampana> {
        let guard = self.estado.lock();
        let campana = guard.campanas.get(&campana_id).ok_or_else(|| {
            PulseError::TelemetriaInvalida(format!(
                "no existe telemetría registrada para la campaña {campana_id}"
            ))
        })?;

        let prospectos_activos: Vec<&EstadoProspectoTelemetria> = campana
            .prospectos
            .values()
            .filter(|p| !p.opt_out)
            .collect();

        let total_prospectos = prospectos_activos.len().max(1) as f32;
        let aperturas = prospectos_activos
            .iter()
            .filter(|p| p.abrio_dossier)
            .count() as f32;
        let respuestas = prospectos_activos
            .iter()
            .filter(|p| p.respuesta_positiva)
            .count() as f32;
        let prospectos_con_retencion_3s = prospectos_activos
            .iter()
            .filter(|p| p.retencion_3s_alcanzada)
            .count() as f32;

        let tasa_apertura = (aperturas / total_prospectos).clamp(0.0, 1.0);
        let tasa_respuesta = (respuestas / total_prospectos).clamp(0.0, 1.0);

        let mut suma_ratio_retencion = 0.0_f32;
        let mut total_muestras_retencion = 0_usize;
        let mut ganchos_bajo_rendimiento = Vec::new();

        for acum in campana.ganchos.values() {
            suma_ratio_retencion += acum.suma_ratio_retencion;
            total_muestras_retencion += acum.muestras_retencion;
            let resumen = acum.resumen();
            if resumen.muestras > 0
                && resumen.tasa_retencion_promedio < 0.30
                && resumen.tasa_apertura_o_respuesta < 0.20
            {
                ganchos_bajo_rendimiento.push(resumen.gancho);
            }
        }

        let tasa_retencion_3s = if total_muestras_retencion > 0 {
            (suma_ratio_retencion / total_muestras_retencion as f32).clamp(0.0, 1.0)
        } else {
            (prospectos_con_retencion_3s / total_prospectos).clamp(0.0, 1.0)
        };

        info!(
            %campana_id,
            tasa_apertura,
            tasa_retencion_3s,
            tasa_respuesta,
            ganchos_bajo_rendimiento = ganchos_bajo_rendimiento.len(),
            "Métricas de campaña consolidadas para Auditoría BOB"
        );

        Ok(MetricasCampana {
            campana_id,
            tasa_apertura,
            tasa_retencion_3s,
            tasa_respuesta,
            ganchos_bajo_rendimiento,
        })
    }

    /// Exporta resúmenes ordenados de desempeño por gancho para construir datasets DPO.
    #[must_use]
    pub fn exportar_estadisticas_ganchos(&self) -> Vec<ResumenGanchoTelemetria> {
        let guard = self.estado.lock();
        let mut out = Vec::new();
        for campana in guard.campanas.values() {
            for acum in campana.ganchos.values() {
                out.push(acum.resumen());
            }
        }
        out.sort_by(|a, b| {
            b.tasa_retencion_promedio
                .total_cmp(&a.tasa_retencion_promedio)
                .then_with(|| {
                    b.tasa_apertura_o_respuesta
                        .total_cmp(&a.tasa_apertura_o_respuesta)
                })
                .then_with(|| a.gancho.cmp(&b.gancho))
        });
        out
    }
}

fn normalizar_etiqueta_gancho(gancho: &str) -> Option<String> {
    let limpio = gancho.trim();
    if limpio.is_empty() {
        None
    } else {
        Some(limpio.to_lowercase())
    }
}

fn validar_evento_telemetria(evento: &EventoTelemetria) -> PulseResult<()> {
    if let TipoEventoTelemetria::RetencionReel {
        segundos_vistos,
        duracion_total_s,
        ..
    } = evento.tipo
    {
        if !segundos_vistos.is_finite()
            || !duracion_total_s.is_finite()
            || segundos_vistos < 0.0
            || duracion_total_s <= 0.0
        {
            return Err(PulseError::TelemetriaInvalida(format!(
                "métricas de RetencionReel inválidas (segundos_vistos={segundos_vistos}, duracion_total_s={duracion_total_s})"
            )));
        }
    }
    Ok(())
}

fn aplicar_evento_a_prospecto(
    reg: &mut EstadoProspectoTelemetria,
    tipo: &TipoEventoTelemetria,
) {
    match tipo {
        TipoEventoTelemetria::ImpresionReel => {
            if reg.estado == EstadoCampanaProspecto::Pendiente {
                reg.estado = EstadoCampanaProspecto::Enviado;
            }
        }
        TipoEventoTelemetria::RetencionReel {
            segundos_vistos,
            duracion_total_s,
            completado,
        } => {
            let ratio = (*segundos_vistos / *duracion_total_s).clamp(0.0, 1.0);
            if *segundos_vistos >= 3.0 || *completado || ratio >= 0.50 {
                reg.retencion_3s_alcanzada = true;
            }
            if reg.estado == EstadoCampanaProspecto::Pendiente {
                reg.estado = EstadoCampanaProspecto::Enviado;
            }
        }
        TipoEventoTelemetria::GuardadoArtefacto => {
            reg.guardado = true;
            if matches!(
                reg.estado,
                EstadoCampanaProspecto::Pendiente
                    | EstadoCampanaProspecto::Calentando
                    | EstadoCampanaProspecto::Enviado
            ) {
                reg.estado = EstadoCampanaProspecto::LandingVisitada;
            }
        }
        TipoEventoTelemetria::AperturaDossierPdf => {
            reg.abrio_dossier = true;
            if matches!(
                reg.estado,
                EstadoCampanaProspecto::Pendiente
                    | EstadoCampanaProspecto::Calentando
                    | EstadoCampanaProspecto::Enviado
            ) {
                reg.estado = EstadoCampanaProspecto::LandingVisitada;
            }
        }
        TipoEventoTelemetria::RespuestaPositiva { .. } => {
            reg.respuesta_positiva = true;
            reg.abrio_dossier = true;
            reg.estado = EstadoCampanaProspecto::EnConversacion;
        }
        TipoEventoTelemetria::ReboteEntrega { .. } => {
            reg.rebotado = true;
        }
        TipoEventoTelemetria::SolicitudBajaOptOut { .. } => {
            reg.opt_out = true;
            reg.estado = EstadoCampanaProspecto::ExcluidoOptOut;
        }
    }
}

fn aplicar_evento_a_gancho(acum: &mut AcumuladorGancho, evento: &EventoTelemetria) {
    acum.eventos_totales += 1;
    acum.prospectos.insert(evento.prospecto_id);
    if acum.guion_resumen.trim().is_empty() && !evento.guion_resumen.trim().is_empty() {
        acum.guion_resumen.clone_from(&evento.guion_resumen);
    }

    match &evento.tipo {
        TipoEventoTelemetria::ImpresionReel => {
            acum.impresiones_o_despachos += 1;
        }
        TipoEventoTelemetria::RetencionReel {
            segundos_vistos,
            duracion_total_s,
            ..
        } => {
            let ratio = (*segundos_vistos / *duracion_total_s).clamp(0.0, 1.0);
            acum.muestras_retencion += 1;
            acum.suma_ratio_retencion += ratio;
        }
        TipoEventoTelemetria::GuardadoArtefacto | TipoEventoTelemetria::AperturaDossierPdf => {
            acum.aperturas_y_guardados += 1;
        }
        TipoEventoTelemetria::RespuestaPositiva { .. } => {
            acum.respuestas_positivas += 1;
        }
        TipoEventoTelemetria::ReboteEntrega { .. }
        | TipoEventoTelemetria::SolicitudBajaOptOut { .. } => {}
    }
}
