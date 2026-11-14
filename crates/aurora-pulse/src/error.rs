use aurora_cortex::CortexError;
use aurora_domain::{AuroraError, EstadoAprobacion};
use aurora_factory::FactoryError;
use aurora_memory::MemoryError;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para `aurora-pulse`.
pub type PulseResult<T> = Result<T, PulseError>;

/// Errores estructurados del motor de pulso, despacho, telemetría y consolidación DPO.
#[derive(Debug, Error)]
pub enum PulseError {
    /// Bloqueo estricto Zero-Bypass cuando una orden, calentamiento o activo no tiene `EstadoAprobacion::Aprobado`.
    #[error(
        "barrera Gatekeeper: entidad {entidad_id} para prospecto {prospecto_id} en estado {estado_actual:?}; se requiere Aprobado"
    )]
    NoAprobadoPorGatekeeper {
        /// Identificador de la orden, activo o calentamiento bloqueado.
        entidad_id: Uuid,
        /// Identificador del prospecto objetivo.
        prospecto_id: Uuid,
        /// Estado detectado que impide la ejecución externa.
        estado_actual: EstadoAprobacion,
    },

    /// Bloqueo LFPDPPP inmediato por bandera `opt_out = true` o presencia en blacklist física.
    #[error("cumplimiento LFPDPPP: prospecto {prospecto_id} excluido por opt_out o baja ARCO")]
    ProspectoOptOut {
        /// Identificador del prospecto excluido.
        prospecto_id: Uuid,
    },

    /// Se alcanzó temporalmente la cuota horaria/diaria o el intervalo mínimo en una plataforma.
    #[error("rate limit activo en plataforma '{plataforma}': {motivo} (esperar {esperar_ms} ms)")]
    RateLimitPlataforma {
        /// Plataforma donde ocurrió el control de flujo.
        plataforma: String,
        /// Motivo técnico (cuota horaria, diaria, intervalo mínimo o HTTP 429).
        motivo: String,
        /// Milisegundos mínimos sugeridos antes del siguiente intento.
        esperar_ms: u64,
    },

    /// El circuit breaker de la cuenta/plataforma fue abierto por fallos consecutivos.
    #[error(
        "circuit breaker abierto para '{plataforma}' tras {fallos_consecutivos} fallos consecutivos; reapertura en {reapertura_en}"
    )]
    CircuitBreakerAbierto {
        /// Plataforma protegida contra suspensión.
        plataforma: String,
        /// Número de fallos consecutivos acumulados.
        fallos_consecutivos: u32,
        /// Instante UTC de enfriamiento antes de reintentar.
        reapertura_en: DateTime<Utc>,
    },

    /// Error de transporte o validación de integridad al despachar un paquete comercial.
    #[error("fallo en despacho por canal '{canal}': {detalle}")]
    DespachoFallido {
        /// Canal objetivo (`CorreoEjecutivo`, `WhatsApp`, `LinkedIn`, etc.).
        canal: String,
        /// Identificador de rastreo asignado cuando ya se había generado orden de despacho.
        tracking_id: Option<Uuid>,
        /// Descripción diagnóstica del fallo.
        detalle: String,
    },

    /// Evento o consulta de telemetría inconsistente o inexistente.
    #[error("telemetría inválida: {0}")]
    TelemetriaInvalida(String),

    /// Error durante la auditoría circadiana, ajuste 4D o escritura JSONL DPO.
    #[error("fallo en consolidación nocturna DPO: {0}")]
    ConsolidacionDpoFallida(String),

    /// Error propagado desde `aurora-domain`.
    #[error("error de dominio Aurora: {0}")]
    Dominio(#[from] AuroraError),

    /// Error propagado desde `aurora-memory`.
    #[error("error de memoria topológica 4D: {0}")]
    Memoria(#[from] MemoryError),

    /// Error propagado desde `aurora-cortex`.
    #[error("error de córteX BOB: {0}")]
    Cortex(#[from] CortexError),

    /// Error propagado desde `aurora-factory`.
    #[error("error de fábrica multimodal: {0}")]
    Fabrica(#[from] FactoryError),
}

impl From<PulseError> for AuroraError {
    fn from(err: PulseError) -> Self {
        match err {
            PulseError::ProspectoOptOut { prospecto_id } => Self::OptOutActive { prospecto_id },
            PulseError::Dominio(e) => e,
            PulseError::Memoria(e) => e.into(),
            PulseError::Cortex(e) => e.into(),
            PulseError::Fabrica(e) => e.into(),
            otro => Self::Validacion(otro.to_string()),
        }
    }
}
