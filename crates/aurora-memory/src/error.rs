use std::path::PathBuf;

use aurora_domain::AuroraError;
use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para el crate `aurora-memory`.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// Errores estructurados del motor de memoria topológica 4D, embeddings CPU y persistencia WAL.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Fallo durante la vectorización o carga de pesos en CPU.
    #[error("error en embedder CPU: {0}")]
    Embedding(String),

    /// El modelo multilingüe configurado aún no ha sido cargado desde disco.
    #[error("modelo de embedding no cargado: {0}")]
    ModeloNoCargado(String),

    /// Discrepancia de dimensionalidad entre el vector recibido y la isla objetivo.
    #[error("dimensión vectorial incompatible: esperada {esperada}, recibida {recibida}")]
    DimensionMismatch {
        /// Dimensión esperada por el índice.
        esperada: usize,
        /// Dimensión recibida.
        recibida: usize,
    },

    /// La isla comercial solicitada no existe o no está inicializada.
    #[error("isla vectorial no encontrada: {0}")]
    IslaNoEncontrada(String),

    /// Error interno en índice vectorial o inserción en isla.
    #[error("error en índice vectorial: {0}")]
    IndiceVectorial(String),

    /// Error operando el hipergrafo causal 4D.
    #[error("error en grafo temporal 4D: {0}")]
    GrafoCausal(String),

    /// Fallo de entrada/salida al operar el WAL o snapshots en NVMe.
    #[error("error de E/S en almacenamiento '{ruta}': {source}")]
    WalIo {
        /// Ruta del archivo WAL o snapshot afectado.
        ruta: PathBuf,
        /// Error de sistema subyacente.
        #[source]
        source: std::io::Error,
    },

    /// Trama o cabecera corrupta detectada durante la recuperación del WAL.
    #[error("WAL corrupto o inconsistente: {0}")]
    WalCorrupto(String),

    /// Error serializando o deserializando estructuras binarias (`bincode`).
    #[error("error de serialización binaria en memoria: {0}")]
    SerializacionBinaria(String),

    /// Fallo durante la purga física LFPDPPP (`opt_out`).
    #[error("error en purga LFPDPPP: {0}")]
    PurgaLfpdppp(String),

    /// El prospecto solicitado no se encuentra en la telaraña ni en las islas.
    #[error("prospecto {0} no encontrado en memoria")]
    ProspectoNoEncontrado(Uuid),

    /// Error de validación o cumplimiento propagado desde `aurora-domain`.
    #[error("error de dominio/LFPDPPP: {0}")]
    Dominio(#[from] AuroraError),
}

impl From<MemoryError> for AuroraError {
    fn from(err: MemoryError) -> Self {
        match err {
            MemoryError::Dominio(inner) => inner,
            otro => Self::PuenteMemoria(otro.to_string()),
        }
    }
}
