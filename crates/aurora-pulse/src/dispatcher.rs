use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::CanalContacto;
use aurora_domain::{validar_activo, validar_prospecto, ActivoGenerado, EstadoAprobacion, Prospecto};
use aurora_factory::{sha256_bytes, MetadatosArtefacto, PaqueteManufacturado};
use aurora_memory::MotorMemoriaAurora;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{PulseError, PulseResult};
use crate::rate_limiter::{ControladorTasaPlataforma, PlataformaPulso};

/// Ciclo de vida de campaña por prospecto rescatado y tipado desde el legado `aurora-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstadoCampanaProspecto {
    /// Registrado y pendiente de evaluación/calentamiento.
    Pendiente,
    /// En ventana de calentamiento periférico de 24–72 h.
    Calentando,
    /// Paquete multimodal despachado hacia el canal seleccionado.
    Enviado,
    /// El prospecto abrió el dossier o visitó la propuesta con tracking.
    LandingVisitada,
    /// El prospecto respondió positivamente iniciando diálogo comercial.
    EnConversacion,
    /// Operación cerrada exitosamente.
    Cerrado,
    /// Prospecto purgado irrevocablemente por solicitud `opt_out` LFPDPPP.
    ExcluidoOptOut,
}

/// Orden formal de envío entregada al [`ChannelTransport`] seleccionado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrdenDespacho {
    /// Identificador único de rastreo para correlación con telemetría.
    pub tracking_id: Uuid,
    /// Identificador de la orden producida en `aurora-factory`.
    pub orden_id: Uuid,
    /// Campaña comercial origen.
    pub campana_id: Uuid,
    /// Prospecto destinatario validado bajo LFPDPPP.
    pub prospecto: Prospecto,
    /// Canal de contacto autorizado por Gatekeeper.
    pub canal: CanalContacto,
    /// Paquete multimodal completo con rutas y checksums SHA-256 en NVMe.
    pub paquete: PaqueteManufacturado,
    /// Enlace personalizado de seguimiento (migrado de `campana_activa.json` de `aurora-core`).
    pub url_propuesta_tracking: String,
    /// Cadencia sugerida en horas para el próximo punto de seguimiento (`48` h por defecto).
    pub cadencia_seguimiento_horas: u16,
    /// Correo electrónico explícito de destino cuando aplica despacho por `CorreoEjecutivo`.
    #[serde(default)]
    pub correo_destino: Option<String>,
}

/// Comprobante trazable emitido tras despachar un paquete multimodal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprobanteDespacho {
    /// Identificador único de rastreo en telemetría.
    pub tracking_id: Uuid,
    /// Identificador de orden original.
    pub orden_id: Uuid,
    /// Campaña asociada.
    pub campana_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Canal empleado para el despacho.
    pub canal: CanalContacto,
    /// Estado de campaña resultante (`Enviado`).
    pub estado_campana: EstadoCampanaProspecto,
    /// Marca temporal UTC del despacho efectivo.
    pub despachado_en: DateTime<Utc>,
    /// Próxima marca temporal UTC sugerida para seguimiento si no hay respuesta.
    pub proximo_seguimiento_en: DateTime<Utc>,
    /// Enlace único de seguimiento asociado a esta entrega.
    pub url_propuesta_tracking: String,
    /// ID de mensaje externo o recibo devuelto por SMTP/API.
    pub referencia_externa: String,
    /// Los 4 `ActivoGenerado` ya transicionados a [`EstadoAprobacion::Despachado`].
    pub activos_despachados: Vec<ActivoGenerado>,
}

/// Contrato asíncrono desacoplado para entrega multi-canal (SMTP, WhatsApp API, LinkedIn, etc.).
#[async_trait]
pub trait ChannelTransport: Send + Sync {
    /// Ejecuta el envío físico del paquete comercial pre-verificado.
    async fn enviar_paquete(&self, envio: &OrdenDespacho) -> PulseResult<ComprobanteDespacho>;
}

/// Despachador multi-canal con verificación SHA-256, Gatekeeper Zero-Bypass, LFPDPPP y rate-limit.
#[derive(Debug)]
pub struct DespachadorMulticanal<T: ChannelTransport> {
    transport: T,
    rate_limiter: Arc<ControladorTasaPlataforma>,
    cadencia_seguimiento_horas: u16,
    url_base_tracking: String,
}

impl<T: ChannelTransport> DespachadorMulticanal<T> {
    /// Crea un despachador multi-canal con cadencia estándar de seguimiento (48 h).
    #[must_use]
    pub fn new(transport: T, rate_limiter: Arc<ControladorTasaPlataforma>) -> Self {
        Self {
            transport,
            rate_limiter,
            cadencia_seguimiento_horas: 48,
            url_base_tracking: "https://aurora.local/propuesta".to_string(),
        }
    }

    /// Permite personalizar la cadencia de seguimiento y URL base de propuestas.
    #[must_use]
    pub fn con_configuracion(
        mut self,
        cadencia_seguimiento_horas: u16,
        url_base_tracking: impl Into<String>,
    ) -> Self {
        self.cadencia_seguimiento_horas = cadencia_seguimiento_horas.max(12);
        self.url_base_tracking = url_base_tracking.into();
        self
    }

    /// Devuelve referencia al adaptador de transporte subyacente.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Valida integridad SHA-256 en NVMe, Gatekeeper, LFPDPPP y cuotas, y despacha el paquete.
    #[tracing::instrument(
        skip(self, paquete, prospecto, memoria),
        fields(
            orden_id = %paquete.orden_id,
            campana_id = %paquete.campana_id,
            prospecto_id = %prospecto.id,
            canal = ?canal
        )
    )]
    pub async fn despachar(
        &self,
        paquete: &PaqueteManufacturado,
        prospecto: &Prospecto,
        canal: CanalContacto,
        memoria: &MotorMemoriaAurora,
        ahora: DateTime<Utc>,
    ) -> PulseResult<ComprobanteDespacho> {
        self.despachar_con_correo(paquete, prospecto, canal, None, memoria, ahora)
            .await
    }

    /// Variante que adjunta explícitamente un correo destino verificado para `CorreoEjecutivo`.
    #[tracing::instrument(
        skip(self, paquete, prospecto, memoria),
        fields(
            orden_id = %paquete.orden_id,
            campana_id = %paquete.campana_id,
            prospecto_id = %prospecto.id,
            canal = ?canal
        )
    )]
    pub async fn despachar_con_correo(
        &self,
        paquete: &PaqueteManufacturado,
        prospecto: &Prospecto,
        canal: CanalContacto,
        correo_destino: Option<String>,
        memoria: &MotorMemoriaAurora,
        ahora: DateTime<Utc>,
    ) -> PulseResult<ComprobanteDespacho> {
        // 1. Cumplimiento estricto LFPDPPP
        if prospecto.lfpdppp.opt_out || memoria.esta_en_blacklist(prospecto.id) {
            warn!(
                prospecto_id = %prospecto.id,
                "Despacho bloqueado: opt_out activo o presente en blacklist física"
            );
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: prospecto.id,
            });
        }
        validar_prospecto(prospecto)?;

        if let Some(reg) = memoria.grafo().obtener_por_uuid(prospecto.id) {
            if reg.prospecto.lfpdppp.opt_out {
                return Err(PulseError::ProspectoOptOut {
                    prospecto_id: prospecto.id,
                });
            }
        }

        // 2. Coherencia de IDs y barrera Zero-Bypass Gatekeeper sobre los 4 activos
        if paquete.prospecto_id != prospecto.id {
            return Err(PulseError::DespachoFallido {
                canal: format!("{canal:?}"),
                tracking_id: None,
                detalle: format!(
                    "prospecto_id inconsistente entre paquete ({}) y prospecto ({})",
                    paquete.prospecto_id, prospecto.id
                ),
            });
        }

        if paquete.activos_actualizados.is_empty() {
            return Err(PulseError::DespachoFallido {
                canal: format!("{canal:?}"),
                tracking_id: None,
                detalle: "el paquete manufacturado no contiene activos registrados".to_string(),
            });
        }

        for activo in &paquete.activos_actualizados {
            if activo.estado != EstadoAprobacion::Aprobado {
                warn!(
                    activo_id = %activo.id,
                    estado = ?activo.estado,
                    "Despacho bloqueado: activo sin EstadoAprobacion::Aprobado"
                );
                return Err(PulseError::NoAprobadoPorGatekeeper {
                    entidad_id: activo.id,
                    prospecto_id: prospecto.id,
                    estado_actual: activo.estado,
                });
            }
            validar_activo(activo)?;
        }

        // 3. Verificación criptográfica SHA-256 de todos los artefactos en NVMe antes de salir
        verificar_integridad_artefactos(paquete).await?;

        // 4. Rate-limiting y circuit breaker por plataforma
        let plat = PlataformaPulso::from(canal);
        let jitter_ms = self.rate_limiter.verificar_y_reservar(plat, ahora)?;
        debug!(jitter_ms, canal = ?canal, "Cuota de despacho aprobada");

        let tracking_id = Uuid::new_v4();
        let url_propuesta_tracking = format!(
            "{}/{}/{}",
            self.url_base_tracking.trim_end_matches('/'),
            prospecto.id,
            tracking_id
        );

        let envio = OrdenDespacho {
            tracking_id,
            orden_id: paquete.orden_id,
            campana_id: paquete.campana_id,
            prospecto: prospecto.clone(),
            canal,
            paquete: paquete.clone(),
            url_propuesta_tracking: url_propuesta_tracking.clone(),
            cadencia_seguimiento_horas: self.cadencia_seguimiento_horas,
            correo_destino,
        };

        let mut comprobante = match self.transport.enviar_paquete(&envio).await {
            Ok(comp) => {
                self.rate_limiter.registrar_exito(plat);
                comp
            }
            Err(err) => {
                let es_rate_limit = matches!(err, PulseError::RateLimitPlataforma { .. });
                let _ = self.rate_limiter.registrar_fallo(plat, ahora, es_rate_limit);
                return Err(err);
            }
        };

        // 5. Garantizar transición formal de activos a EstadoAprobacion::Despachado
        let mut activos_despachados = paquete.activos_actualizados.clone();
        for activo in &mut activos_despachados {
            activo.estado = EstadoAprobacion::Despachado;
            activo.actualizado_en = ahora;
            activo.motivo_rechazo = None;
            validar_activo(activo)?;
        }
        comprobante.despachado_en = ahora;
        comprobante.proximo_seguimiento_en =
            ahora + Duration::hours(i64::from(self.cadencia_seguimiento_horas));
        comprobante.estado_campana = EstadoCampanaProspecto::Enviado;
        comprobante.activos_despachados = activos_despachados;

        info!(
            tracking_id = %comprobante.tracking_id,
            campana_id = %comprobante.campana_id,
            prospecto_id = %comprobante.prospecto_id,
            canal = ?canal,
            "Paquete manufacturado despachado y marcado como EstadoAprobacion::Despachado"
        );

        Ok(comprobante)
    }
}

async fn verificar_integridad_artefactos(paquete: &PaqueteManufacturado) -> PulseResult<()> {
    for (etiqueta, meta) in [
        ("dossier_pdf", &paquete.dossier_pdf),
        ("nota_voz_wav", &paquete.nota_voz_wav),
        ("reel_vertical_mp4", &paquete.reel_vertical_mp4),
        ("subtitulos_srt", &paquete.subtitulos_srt),
    ] {
        verificar_un_artefacto(etiqueta, meta, paquete.orden_id).await?;
    }
    Ok(())
}

async fn verificar_un_artefacto(
    etiqueta: &str,
    meta: &MetadatosArtefacto,
    orden_id: Uuid,
) -> PulseResult<()> {
    let bytes = tokio::fs::read(&meta.ruta_nvme).await.map_err(|e| {
        PulseError::DespachoFallido {
            canal: "verificador_nvme".to_string(),
            tracking_id: Some(orden_id),
            detalle: format!(
                "no se pudo leer '{etiqueta}' en '{}': {e}",
                meta.ruta_nvme.display()
            ),
        }
    })?;

    if (bytes.len() as u64) != meta.tamano_bytes {
        return Err(PulseError::DespachoFallido {
            canal: "verificador_nvme".to_string(),
            tracking_id: Some(orden_id),
            detalle: format!(
                "tamaño alterado en '{etiqueta}': esperado {} bytes, real {} bytes",
                meta.tamano_bytes,
                bytes.len()
            ),
        });
    }

    let hash_actual = sha256_bytes(&bytes);
    if hash_actual != meta.sha256_hex {
        return Err(PulseError::DespachoFallido {
            canal: "verificador_nvme".to_string(),
            tracking_id: Some(orden_id),
            detalle: format!(
                "checksum SHA-256 inválido en '{etiqueta}': esperado {}, real {hash_actual}",
                meta.sha256_hex
            ),
        });
    }

    Ok(())
}

/// Transporte simulado en memoria con soporte de inyección de fallos 429 y señales `opt_out`.
#[derive(Debug, Default)]
pub struct DeterministicMockTransport {
    envios: Mutex<Vec<OrdenDespacho>>,
    fallos_429_restantes: Mutex<u32>,
    prospectos_opt_out: Mutex<HashSet<Uuid>>,
}

impl DeterministicMockTransport {
    /// Crea un transporte mock limpio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Programa `cantidad` errores consecutivos simulados HTTP 429 para probar circuit breakers.
    pub fn inyectar_fallos_rate_limit_consecutivos(&self, cantidad: u32) {
        *self.fallos_429_restantes.lock() = cantidad;
    }

    /// Registra que un prospecto rechazará el envío reportando señal remota de `opt_out`.
    pub fn inyectar_bloqueo_opt_out(&self, prospecto_id: Uuid) {
        self.prospectos_opt_out.lock().insert(prospecto_id);
    }

    /// Devuelve todos los despachos completados exitosamente en memoria.
    #[must_use]
    pub fn envios_realizados(&self) -> Vec<OrdenDespacho> {
        self.envios.lock().clone()
    }
}

#[async_trait]
impl ChannelTransport for DeterministicMockTransport {
    async fn enviar_paquete(&self, envio: &OrdenDespacho) -> PulseResult<ComprobanteDespacho> {
        if self.prospectos_opt_out.lock().contains(&envio.prospecto.id) {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: envio.prospecto.id,
            });
        }

        {
            let mut rest = self.fallos_429_restantes.lock();
            if *rest > 0 {
                *rest -= 1;
                return Err(PulseError::RateLimitPlataforma {
                    plataforma: PlataformaPulso::from(envio.canal).etiqueta().to_string(),
                    motivo: "HTTP 429 Too Many Requests simulado por plataforma".to_string(),
                    esperar_ms: 15_000,
                });
            }
        }

        self.envios.lock().push(envio.clone());
        let ahora = Utc::now();

        Ok(ComprobanteDespacho {
            tracking_id: envio.tracking_id,
            orden_id: envio.orden_id,
            campana_id: envio.campana_id,
            prospecto_id: envio.prospecto.id,
            canal: envio.canal,
            estado_campana: EstadoCampanaProspecto::Enviado,
            despachado_en: ahora,
            proximo_seguimiento_en: ahora
                + Duration::hours(i64::from(envio.cadencia_seguimiento_horas)),
            url_propuesta_tracking: envio.url_propuesta_tracking.clone(),
            referencia_externa: format!("mock://{:?}/{}", envio.canal, envio.tracking_id),
            activos_despachados: envio.paquete.activos_actualizados.clone(),
        })
    }
}
