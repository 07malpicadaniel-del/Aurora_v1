//! Crate `aurora-pulse` — Distribución algorítmica, calentamiento periférico,
//! telemetría comercial con cumplimiento LFPDPPP inmediato y consolidación DPO nocturna.
//!
//! Actúa como capa de ejecución controlada tras `aurora-cortex` (Fase 3) y
//! `aurora-factory` (Fase 4), aplicando siempre:
//! - Barrera estricta de **Gatekeeper Zero-Bypass** (`EstadoAprobacion::Aprobado`).
//! - Bloqueo y purga física **LFPDPPP** inmediata ante señales `opt_out`.
//! - Control conservador de tasa, jitter determinista y circuit breakers por plataforma.
//! - Retroalimentación nocturna al hipergrafo 4D y generación de datasets JSONL DPO para BOB.

pub mod dispatcher;
pub mod dpo_loop;
pub mod error;
pub mod rate_limiter;
pub mod smtp_transport;
pub mod social_publisher;
pub mod telemetry;
pub mod warmup;

pub use dispatcher::{
    ChannelTransport, ComprobanteDespacho, DespachadorMulticanal, DeterministicMockTransport,
    EstadoCampanaProspecto, OrdenDespacho,
};
pub use dpo_loop::{
    ConfigConsolidacionNocturna, DatasetPreferenciasDpo, EntradaPreferenciaDpo,
    MotorConsolidacionNocturna, ReporteConsolidacionDpo,
};
pub use error::{PulseError, PulseResult};
pub use rate_limiter::{
    ControladorTasaPlataforma, EstadoTasaPlataforma, PlataformaPulso, PoliticaTasaPlataforma,
};
pub use smtp_transport::{ConfigSmtp, SmtpEmailTransport};
pub use social_publisher::{
    validar_solicitud_organica, ComprobantePublicacionOrganica, ConfigMetaReels,
    ConfigYouTubeShorts, DeterministicMockSocialPublisher, EstadoPublicadorOrganico,
    GestorPublicacionOrganica, MetaReelsGraphPublisher, OrganicSocialPublisher,
    SolicitudPublicacionOrganica, YouTubeShortsApiPublisher, DEFAULT_META_GRAPH_BASE_URL,
    DEFAULT_YOUTUBE_UPLOAD_URL,
};
pub use telemetry::{
    EventoTelemetria, RecolectorTelemetria, ResumenGanchoTelemetria, TipoEventoTelemetria,
};
pub use warmup::{
    ConfigCalentamiento, DeterministicMockWarmupDriver, GestorCalentamientoPeriferico,
    RegistroInteraccion, SocialWarmupDriver,
};
