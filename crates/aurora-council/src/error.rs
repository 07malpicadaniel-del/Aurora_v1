use aurora_cortex::CortexError;
use aurora_domain::AuroraError;
use thiserror::Error;

/// Alias estándar de resultado para `aurora-council`.
pub type CouncilResult<T> = Result<T, CouncilError>;

/// Errores de orquestación multi-agente, cuotas, sanitización PII y drivers CLI.
#[derive(Debug, Error)]
pub enum CouncilError {
    /// La cuota móvil de 5 horas o el tope diario del proveedor se encuentran agotados.
    #[error("cuota agotada para {miembro}: {detalle}")]
    CuotaAgotada {
        /// Miembro del consejo que agotó su presupuesto.
        miembro: String,
        /// Detalle operativo del bloqueo de cuota.
        detalle: String,
    },

    /// El proveedor externo devolvió un límite de tasa (HTTP 429 / usage limit CLI).
    #[error("rate-limit externo detectado en {miembro}: {detalle}")]
    RateLimitExterno {
        /// Miembro que reportó rate-limit.
        miembro: String,
        /// Extracto diagnóstico de la respuesta del CLI.
        detalle: String,
    },

    /// El subproceso CLI excedió el tiempo máximo permitido y fue terminado.
    #[error("timeout ejecutando CLI de {miembro} tras {timeout_ms} ms")]
    TimeoutCli {
        /// Miembro cuyo subproceso expiró.
        miembro: String,
        /// Timeout aplicado en milisegundos.
        timeout_ms: u64,
    },

    /// El binario CLI o driver solicitado no está disponible en el sistema local.
    #[error("CLI no disponible para {miembro}: {detalle}")]
    CliNoDisponible {
        /// Miembro indisponible.
        miembro: String,
        /// Motivo o ruta no encontrada.
        detalle: String,
    },

    /// Falla crítica del `PiiFirewall` (fuga residual detectada o regla inválida).
    #[error("bloqueo LFPDPPP de PiiFirewall: {0}")]
    FalloSanitizacionPii(String),

    /// El driver devolvió salida vacía, malformada o código de salida distinto de cero.
    #[error("salida inválida desde {miembro}: {detalle}")]
    SalidaInvalida {
        /// Miembro origen de la salida inválida.
        miembro: String,
        /// Detalle del fallo de ejecución o formato.
        detalle: String,
    },

    /// Error de lectura/escritura atómica en NVMe al persistir cuotas.
    #[error("error de E/S en persistencia NVMe del consejo: {0}")]
    IoPersistencia(#[from] std::io::Error),

    /// Error propagado desde el motor local `aurora-cortex` (BOB `slot_id = 0`).
    #[error("error cognitivo BOB local: {0}")]
    Cortex(#[from] CortexError),

    /// Error propagado desde el contrato base `aurora-domain`.
    #[error("error de dominio Aurora: {0}")]
    Dominio(#[from] AuroraError),
}
