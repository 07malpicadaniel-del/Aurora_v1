use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::{
    CalentamientoPendiente, PlataformaCalentamiento, TipoInteraccionPeriferica,
};
use aurora_domain::{validar_prospecto, EstadoAprobacion};
use aurora_memory::{MemoryError, MotorMemoriaAurora};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{PulseError, PulseResult};
use crate::rate_limiter::{ControladorTasaPlataforma, PlataformaPulso};

/// Resultado auditado tras ejecutar una interacción periférica previa al contacto directo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroInteraccion {
    /// Identificador único de la ejecución.
    pub interaccion_id: Uuid,
    /// Identificador de la solicitud `CalentamientoPendiente` aprobada.
    pub calentamiento_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Plataforma pública donde se ejecutó la interacción.
    pub plataforma: PlataformaCalentamiento,
    /// Tipo de señal no invasiva aplicada.
    pub tipo_interaccion: TipoInteraccionPeriferica,
    /// Instante UTC de ejecución efectiva.
    pub ejecutado_en: DateTime<Utc>,
    /// Ventana mínima (24–72 h) tras la cual se habilita el impacto directo con Reel/Dossier.
    pub impacto_habilitado_desde: DateTime<Utc>,
    /// Identificador o evidencia verificable emitida por el controlador/CDP.
    pub evidencia_plataforma: String,
}

/// Contrato asíncrono desacoplado para ejecutar acciones sociales periféricas.
///
/// Preparado para conectarse con `bob_engine/router/src/browser_cdp.rs` vía Chrome DevTools
/// o APIs sociales sin acoplar dependencias de navegador en el núcleo.
#[async_trait]
pub trait SocialWarmupDriver: Send + Sync {
    /// Ejecuta la interacción periférica pre-aprobada sobre la plataforma objetivo.
    async fn ejecutar_interaccion(
        &self,
        accion: &CalentamientoPendiente,
    ) -> PulseResult<RegistroInteraccion>;
}

/// Configuración de maduración entre calentamiento periférico e impacto comercial directo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCalentamiento {
    /// Ventana de maduración recomendada en horas (acotada al rango `24..=72`).
    pub ventana_maduracion_horas: i64,
}

impl Default for ConfigCalentamiento {
    fn default() -> Self {
        Self {
            ventana_maduracion_horas: 36,
        }
    }
}

/// Gestor de calentamiento periférico con barrera Gatekeeper, LFPDPPP y rate-limiting.
#[derive(Debug)]
pub struct GestorCalentamientoPeriferico<W: SocialWarmupDriver> {
    driver: W,
    rate_limiter: Arc<ControladorTasaPlataforma>,
    config: ConfigCalentamiento,
    maduracion_por_prospecto: Mutex<HashMap<Uuid, DateTime<Utc>>>,
}

impl<W: SocialWarmupDriver> GestorCalentamientoPeriferico<W> {
    /// Construye un gestor de calentamiento sobre `driver` y el controlador de tasas compartido.
    #[must_use]
    pub fn new(
        driver: W,
        rate_limiter: Arc<ControladorTasaPlataforma>,
        config: ConfigCalentamiento,
    ) -> Self {
        Self {
            driver,
            rate_limiter,
            config,
            maduracion_por_prospecto: Mutex::new(HashMap::new()),
        }
    }

    /// Devuelve referencia al controlador de ejecución social.
    #[must_use]
    pub const fn driver(&self) -> &W {
        &self.driver
    }

    /// Devuelve la fecha UTC desde la cual un prospecto maduró para impacto directo.
    #[must_use]
    pub fn ventana_maduracion(&self, prospecto_id: Uuid) -> Option<DateTime<Utc>> {
        self.maduracion_por_prospecto
            .lock()
            .get(&prospecto_id)
            .copied()
    }

    /// Indica si la ventana de maduración (24–72 h) ya venció en `ahora` (o no tiene bloqueo activo).
    #[must_use]
    pub fn impacto_directo_habilitado(&self, prospecto_id: Uuid, ahora: DateTime<Utc>) -> bool {
        self.maduracion_por_prospecto
            .lock()
            .get(&prospecto_id)
            .is_none_or(|&habilitado_desde| ahora >= habilitado_desde)
    }

    /// Ejecuta una acción de calentamiento tras validar Gatekeeper, LFPDPPP y cuotas por plataforma.
    #[tracing::instrument(
        skip(self, accion, memoria),
        fields(
            calentamiento_id = %accion.calentamiento_id,
            prospecto_id = %accion.prospecto_id,
            plataforma = ?accion.plataforma
        )
    )]
    pub async fn ejecutar_calentamiento(
        &self,
        accion: &CalentamientoPendiente,
        memoria: &MotorMemoriaAurora,
        ahora: DateTime<Utc>,
    ) -> PulseResult<RegistroInteraccion> {
        // 1. Barrera Zero-Bypass de Gatekeeper
        let estado = accion.estado_gatekeeper();
        if estado != EstadoAprobacion::Aprobado {
            warn!(
                calentamiento_id = %accion.calentamiento_id,
                ?estado,
                "Calentamiento bloqueado: no cuenta con EstadoAprobacion::Aprobado"
            );
            return Err(PulseError::NoAprobadoPorGatekeeper {
                entidad_id: accion.calentamiento_id,
                prospecto_id: accion.prospecto_id,
                estado_actual: estado,
            });
        }

        // 2. Cumplimiento estricto LFPDPPP (blacklist física + nodo vivo sin opt_out)
        if memoria.esta_en_blacklist(accion.prospecto_id) {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: accion.prospecto_id,
            });
        }
        let registro_nodo = memoria
            .grafo()
            .obtener_por_uuid(accion.prospecto_id)
            .ok_or(PulseError::Memoria(MemoryError::ProspectoNoEncontrado(
                accion.prospecto_id,
            )))?;
        if registro_nodo.prospecto.lfpdppp.opt_out {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: accion.prospecto_id,
            });
        }
        validar_prospecto(&registro_nodo.prospecto)?;

        // 3. Control de tasa y circuit breaker por plataforma
        let plat = PlataformaPulso::from(accion.plataforma);
        let jitter_ms = self.rate_limiter.verificar_y_reservar(plat, ahora)?;
        debug!(jitter_ms, "Ventana social verificada para calentamiento");

        // 4. Ejecución desacoplada vía driver
        let mut registro = match self.driver.ejecutar_interaccion(accion).await {
            Ok(reg) => {
                self.rate_limiter.registrar_exito(plat);
                reg
            }
            Err(err) => {
                let es_rate_limit = matches!(err, PulseError::RateLimitPlataforma { .. });
                let _ = self.rate_limiter.registrar_fallo(plat, ahora, es_rate_limit);
                return Err(err);
            }
        };

        // 5. Programar ventana obligatoria de maduración 24–72 h previa al impacto directo
        let horas_maduracion = self.config.ventana_maduracion_horas.clamp(24, 72);
        let habilitado_desde = ahora + Duration::hours(horas_maduracion);
        registro.ejecutado_en = ahora;
        registro.impacto_habilitado_desde = habilitado_desde;

        self.maduracion_por_prospecto
            .lock()
            .insert(accion.prospecto_id, habilitado_desde);

        info!(
            prospecto_id = %accion.prospecto_id,
            horas_maduracion,
            %habilitado_desde,
            "Calentamiento periférico completado y ventana de maduración programada"
        );

        Ok(registro)
    }
}

/// Driver determinista offline para pruebas CI sin navegador Chrome/CDP real.
#[derive(Debug, Default)]
pub struct DeterministicMockWarmupDriver {
    ejecutadas: Mutex<Vec<CalentamientoPendiente>>,
}

impl DeterministicMockWarmupDriver {
    /// Crea un driver simulado vacío.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Devuelve una copia de todas las interacciones ejecutadas.
    #[must_use]
    pub fn historial(&self) -> Vec<CalentamientoPendiente> {
        self.ejecutadas.lock().clone()
    }
}

#[async_trait]
impl SocialWarmupDriver for DeterministicMockWarmupDriver {
    async fn ejecutar_interaccion(
        &self,
        accion: &CalentamientoPendiente,
    ) -> PulseResult<RegistroInteraccion> {
        self.ejecutadas.lock().push(accion.clone());
        let ahora = Utc::now();
        Ok(RegistroInteraccion {
            interaccion_id: Uuid::new_v4(),
            calentamiento_id: accion.calentamiento_id,
            prospecto_id: accion.prospecto_id,
            plataforma: accion.plataforma,
            tipo_interaccion: accion.tipo_interaccion,
            ejecutado_en: ahora,
            impacto_habilitado_desde: ahora + Duration::hours(36),
            evidencia_plataforma: format!(
                "mock://cdp/{:?}/{}",
                accion.plataforma, accion.calentamiento_id
            ),
        })
    }
}
