use aurora_domain::AuroraError;
use aurora_memory::MemoryError;
use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para el crate `aurora-cortex`.
pub type CortexResult<T> = Result<T, CortexError>;

/// Errores del Córtex Comercial de BOB, gramáticas GBNF y registro de herramientas.
#[derive(Debug, Error)]
pub enum CortexError {
    /// Error de transporte o red comunicándose con el servidor `bob_engine` / `llama.cpp`.
    #[error("error de conexión con BOB Cortex en '{url}': {source}")]
    ConexionBob {
        /// URL del servidor de inferencia.
        url: String,
        /// Error subyacente de `reqwest`.
        #[source]
        source: reqwest::Error,
    },

    /// Respuesta HTTP no exitosa del servidor de inferencia local.
    #[error("respuesta HTTP inválida ({codigo}) de BOB Cortex en '{url}': {detalle}")]
    RespuestaHttpInvalida {
        /// URL del servidor.
        url: String,
        /// Código HTTP recibido.
        codigo: u16,
        /// Cuerpo o detalle de error.
        detalle: String,
    },

    /// Error de configuración del cliente HTTP del Córtex.
    #[error("error de configuración de cliente BOB Cortex: {0}")]
    ConfiguracionCliente(String),

    /// La salida del LLM violó la gramática GBNF, el esquema JSON estricto o los rangos semánticos.
    #[error("violación de esquema/GBNF en rol '{rol}': {detalle}")]
    ViolacionEsquema {
        /// Rol cognitivo activo durante la inferencia.
        rol: String,
        /// Motivo detallado del rechazo.
        detalle: String,
    },

    /// La gramática GBNF generada o validada contiene un error sintáctico.
    #[error("gramática GBNF inválida: {0}")]
    GramaticaInvalida(String),

    /// Se intentó invocar una herramienta no permitida o con argumentos inválidos.
    #[error("herramienta '{herramienta}' no autorizada: {motivo}")]
    HerramientaNoAutorizada {
        /// Nombre de la herramienta solicitada.
        herramienta: String,
        /// Razón del rechazo determinista.
        motivo: String,
    },

    /// El Gatekeeper bloqueó la orden por cumplimiento LFPDPPP o restricción de seguridad.
    #[error("bloqueo Gatekeeper para prospecto {prospecto_id}: {motivo}")]
    BloqueoGatekeeper {
        /// Identificador del prospecto bloqueado.
        prospecto_id: Uuid,
        /// Motivo del bloqueo.
        motivo: String,
    },

    /// Tiempo límite o reintentos agotados esperando inferencia en la RTX 5060 Ti.
    #[error("timeout de inferencia BOB Cortex tras {intentos} intentos ({timeout_ms} ms)")]
    TimeoutInferencia {
        /// Número de intentos realizados.
        intentos: u32,
        /// Timeout por intento en milisegundos.
        timeout_ms: u64,
    },

    /// Error propagado desde la capa de memoria topológica 4D (`aurora-memory`).
    #[error("error en memoria 4D: {0}")]
    Memoria(#[from] MemoryError),

    /// Error propagado desde el dominio o validación LFPDPPP (`aurora-domain`).
    #[error("error de dominio/LFPDPPP: {0}")]
    Dominio(#[from] AuroraError),
}

impl From<CortexError> for AuroraError {
    fn from(err: CortexError) -> Self {
        match err {
            CortexError::Dominio(inner) => inner,
            CortexError::Memoria(mem) => Self::from(mem),
            otro => Self::Validacion(otro.to_string()),
        }
    }
}
