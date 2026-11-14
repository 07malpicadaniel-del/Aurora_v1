use aurora_domain::AuroraError;
use thiserror::Error;

/// Alias estándar de resultado para el crate `aurora-osint`.
pub type OsintResult<T> = Result<T, OsintError>;

/// Errores estructurados de extracción, destilación y resolución de entidades OSINT.
#[derive(Debug, Error)]
pub enum OsintError {
    /// Fallo de transporte o cliente HTTP al consultar una fuente pública.
    #[error("error de red/HTTP consultando '{url}': {source}")]
    HttpRequest {
        /// URL objetivo solicitada.
        url: String,
        /// Error subyacente de `reqwest`.
        #[source]
        source: reqwest::Error,
    },

    /// Respuesta HTTP con código de error inesperado tras agotar reintentos o sin reintento.
    #[error("respuesta HTTP inesperada ({codigo}) en '{url}'")]
    HttpStatusInesperado {
        /// URL objetivo solicitada.
        url: String,
        /// Código de estado HTTP recibido.
        codigo: u16,
    },

    /// Se excedió el presupuesto de reintentos ante límites de tasa (`429` / `503`).
    #[error("rate-limit excedido en fuente '{fuente}' tras {intentos} intentos (HTTP {ultimo_codigo})")]
    RateLimitExcedido {
        /// Nombre de la fuente o dominio limitado.
        fuente: String,
        /// Número de intentos ejecutados.
        intentos: u32,
        /// Último código de estado observado.
        ultimo_codigo: u16,
    },

    /// Error al construir el cliente HTTP o procesar una URL objetivo.
    #[error("error de configuración HTTP: {0}")]
    ConfiguracionHttp(String),

    /// Fallo al parsear HTML o selectores CSS de una fuente pública.
    #[error("error parseando HTML en fuente '{fuente}': {detalle}")]
    ParseoHtml {
        /// Fuente origen del documento.
        fuente: String,
        /// Motivo del fallo.
        detalle: String,
    },

    /// Fallo al deserializar una respuesta JSON pública.
    #[error("error parseando JSON en fuente '{fuente}': {source}")]
    ParseoJson {
        /// Fuente origen del JSON.
        fuente: String,
        /// Error subyacente de `serde_json`.
        #[source]
        source: serde_json::Error,
    },

    /// Error del puente opcional de navegador CDP cuando se requiere renderizado JS.
    #[error("error en DynamicBrowserFetcher: {0}")]
    BrowserCdp(String),

    /// Fallo durante la deduplicación o inferencia relacional de entidades.
    #[error("error en resolución de entidades: {0}")]
    ResolucionEntidades(String),

    /// Fallo en primitivas de concurrencia asíncrona (`tokio::sync::Semaphore` / `JoinSet`).
    #[error("error de concurrencia OSINT: {0}")]
    Concurrencia(String),

    /// Error propagado desde las reglas de dominio o cumplimiento LFPDPPP de `aurora-domain`.
    #[error("error de dominio/LFPDPPP: {0}")]
    Dominio(#[from] AuroraError),
}

impl From<OsintError> for AuroraError {
    fn from(err: OsintError) -> Self {
        match err {
            OsintError::Dominio(inner) => inner,
            otro => Self::Validacion(otro.to_string()),
        }
    }
}
