use std::path::PathBuf;

use aurora_domain::{AuroraError, EstadoAprobacion};
use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para `aurora-factory`.
pub type FactoryResult<T> = Result<T, FactoryError>;

/// Errores estructurados del pipeline de manufactura multimodal (`aurora-factory`).
#[derive(Debug, Error)]
pub enum FactoryError {
    /// Bloqueo estricto Zero-Bypass cuando la orden no cuenta con `EstadoAprobacion::Aprobado`.
    #[error(
        "barrera Gatekeeper: orden {orden_id} para prospecto {prospecto_id} en estado {estado_actual:?}; se requiere Aprobado"
    )]
    NoAprobadoPorGatekeeper {
        /// Identificador de la orden rechazada o pendiente.
        orden_id: Uuid,
        /// Identificador del prospecto destinatario.
        prospecto_id: Uuid,
        /// Estado detectado en la orden o en alguno de sus activos.
        estado_actual: EstadoAprobacion,
    },

    /// Bloqueo LFPDPPP cuando el prospecto ejerció exclusión (`opt_out = true`).
    #[error("cumplimiento LFPDPPP: el prospecto {prospecto_id} tiene opt_out activo en Factory")]
    ProspectoOptOut {
        /// Identificador del prospecto excluido.
        prospecto_id: Uuid,
    },

    /// Fallo en la síntesis de voz local CPU/RAM (Piper o generador WAV).
    #[error("error de síntesis TTS en CPU/RAM: {0}")]
    TtsFallido(String),

    /// Fallo al renderizar HTML/CSS hacia Dossier PDF o fotogramas PNG.
    #[error("error al renderizar documento/escena HTML: {0}")]
    RenderDocumentoFallido(String),

    /// Fallo al ensamblar o codificar video vertical mediante FFmpeg (`h264_nvenc` / `libx264`).
    #[error("error de ensamblaje/codificación FFmpeg: {0}")]
    VideoFfmpegFallido(String),

    /// Exceso de tiempo límite en un subproceso del sistema (`piper`, `chromium`, `ffmpeg`).
    #[error("timeout ({timeout_ms} ms) ejecutando subproceso '{comando}'")]
    SubprocesoTimeout {
        /// Nombre o binario del subproceso agotado.
        comando: String,
        /// Tiempo máximo configurado en milisegundos.
        timeout_ms: u64,
    },

    /// Error de entrada/salida manipulando artefactos en NVMe.
    #[error("error de E/S sobre artefacto '{}': {source}", ruta.display())]
    IoArtefacto {
        /// Ruta en disco implicada en la operación fallida.
        ruta: PathBuf,
        /// Error subyacente del sistema operativo.
        #[source]
        source: std::io::Error,
    },

    /// Inconsistencia en datos de entrada, guiones vacíos o identificadores no coincidentes.
    #[error("datos inválidos en solicitud de manufactura: {0}")]
    DatosInvalidos(String),

    /// Error propagado desde validaciones canónicas de `aurora-domain`.
    #[error("error de dominio Aurora: {0}")]
    Dominio(#[from] AuroraError),
}

impl From<FactoryError> for AuroraError {
    fn from(err: FactoryError) -> Self {
        match err {
            FactoryError::ProspectoOptOut { prospecto_id } => Self::OptOutActive { prospecto_id },
            FactoryError::Dominio(e) => e,
            otro => Self::Validacion(otro.to_string()),
        }
    }
}
