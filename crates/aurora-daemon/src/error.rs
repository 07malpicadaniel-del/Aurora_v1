use aurora_cortex::CortexError;
use aurora_domain::AuroraError;
use aurora_factory::FactoryError;
use aurora_memory::MemoryError;
use aurora_osint::OsintError;
use aurora_pulse::PulseError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para `aurora-daemon`.
pub type DaemonResult<T> = Result<T, DaemonError>;

/// Errores HTTP y de orquestación del demonio persistente `aurora-daemon`.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// Prospecto no encontrado o purgado previamente por LFPDPPP.
    #[error("prospecto {0} no encontrado en memoria 4D o purgado por LFPDPPP")]
    ProspectoNoEncontrado(Uuid),

    /// Orden pendiente no localizada en la cola Gatekeeper.
    #[error("orden Gatekeeper {0} no encontrada")]
    OrdenNoEncontrada(Uuid),

    /// Solicitud HTTP malformada o parámetros inválidos.
    #[error("solicitud inválida: {0}")]
    SolicitudInvalida(String),

    /// Barrera Gatekeeper o estado inválido de transición.
    #[error("bloqueo Gatekeeper: {0}")]
    BloqueoGatekeeper(String),

    /// Prospecto bloqueado por ejercicio de derechos ARCO / `opt_out`.
    #[error("cumplimiento LFPDPPP: prospecto {0} en opt_out")]
    OptOutActivo(Uuid),

    /// Limitación de tasa o circuit breaker abierto en `aurora-pulse`.
    #[error("control de flujo activo: {0}")]
    RateLimit(String),

    /// Error propagado desde `aurora-domain`.
    #[error("error de dominio: {0}")]
    Dominio(#[from] AuroraError),

    /// Error propagado desde `aurora-osint`.
    #[error("error OSINT: {0}")]
    Osint(#[from] OsintError),

    /// Error propagado desde `aurora-memory`.
    #[error("error de memoria 4D: {0}")]
    Memoria(#[from] MemoryError),

    /// Error propagado desde `aurora-cortex`.
    #[error("error cognitivo BOB: {0}")]
    Cortex(#[from] CortexError),

    /// Error propagado desde `aurora-factory`.
    #[error("error de fábrica multimodal: {0}")]
    Fabrica(#[from] FactoryError),

    /// Error propagado desde `aurora-pulse`.
    #[error("error de pulso y distribución: {0}")]
    Pulso(#[from] PulseError),

    /// Fallo interno de sincronización o persistencia en NVMe.
    #[error("error interno del demonio: {0}")]
    Interno(String),
}

#[derive(Debug, Serialize)]
struct ErrorHttpBody {
    error: String,
    codigo: String,
    detalle: String,
}

impl IntoResponse for DaemonError {
    fn into_response(self) -> Response {
        let (status, codigo) = match &self {
            Self::ProspectoNoEncontrado(_) | Self::OrdenNoEncontrada(_) => {
                (StatusCode::NOT_FOUND, "NOT_FOUND")
            }
            Self::SolicitudInvalida(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            Self::BloqueoGatekeeper(_) => (StatusCode::CONFLICT, "GATEKEEPER_LOCK"),
            Self::OptOutActivo(_) => (StatusCode::FORBIDDEN, "LFPDPPP_OPT_OUT"),
            Self::RateLimit(_) => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMIT"),
            Self::Dominio(AuroraError::OptOutActive { .. }) => {
                (StatusCode::FORBIDDEN, "LFPDPPP_OPT_OUT")
            }
            Self::Dominio(_) => (StatusCode::BAD_REQUEST, "DOMAIN_VALIDATION"),
            Self::Osint(_) => (StatusCode::BAD_GATEWAY, "OSINT_ERROR"),
            Self::Memoria(MemoryError::ProspectoNoEncontrado(_)) => {
                (StatusCode::NOT_FOUND, "NOT_FOUND")
            }
            Self::Memoria(MemoryError::Dominio(AuroraError::OptOutActive { .. })) => {
                (StatusCode::FORBIDDEN, "LFPDPPP_OPT_OUT")
            }
            Self::Memoria(_) => (StatusCode::INTERNAL_SERVER_ERROR, "MEMORY_ERROR"),
            Self::Cortex(CortexError::ConexionBob { .. })
            | Self::Cortex(CortexError::TimeoutInferencia { .. }) => {
                (StatusCode::BAD_GATEWAY, "BOB_OFFLINE")
            }
            Self::Cortex(CortexError::BloqueoGatekeeper { .. }) => {
                (StatusCode::CONFLICT, "GATEKEEPER_LOCK")
            }
            Self::Cortex(_) => (StatusCode::BAD_GATEWAY, "CORTEX_ERROR"),
            Self::Fabrica(FactoryError::NoAprobadoPorGatekeeper { .. }) => {
                (StatusCode::CONFLICT, "GATEKEEPER_LOCK")
            }
            Self::Fabrica(FactoryError::ProspectoOptOut { .. }) => {
                (StatusCode::FORBIDDEN, "LFPDPPP_OPT_OUT")
            }
            Self::Fabrica(_) => (StatusCode::INTERNAL_SERVER_ERROR, "FACTORY_ERROR"),
            Self::Pulso(PulseError::NoAprobadoPorGatekeeper { .. }) => {
                (StatusCode::CONFLICT, "GATEKEEPER_LOCK")
            }
            Self::Pulso(PulseError::ProspectoOptOut { .. }) => {
                (StatusCode::FORBIDDEN, "LFPDPPP_OPT_OUT")
            }
            Self::Pulso(PulseError::RateLimitPlataforma { .. })
            | Self::Pulso(PulseError::CircuitBreakerAbierto { .. }) => {
                (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMIT")
            }
            Self::Pulso(_) => (StatusCode::INTERNAL_SERVER_ERROR, "PULSE_ERROR"),
            Self::Interno(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let detalle = self.to_string();
        let body = Json(ErrorHttpBody {
            error: codigo.to_string(),
            codigo: codigo.to_string(),
            detalle,
        });
        (status, body).into_response()
    }
}
