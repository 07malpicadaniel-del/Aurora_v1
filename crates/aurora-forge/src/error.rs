use aurora_council::CouncilError;
use aurora_domain::AuroraError;
use thiserror::Error;

/// Alias estándar de resultado para `aurora-forge`.
pub type ForgeResult<T> = Result<T, ForgeError>;

/// Errores de aislamiento en worktrees Git, seguridad de rutas y compuertas de validación.
#[derive(Debug, Error)]
pub enum ForgeError {
    /// La orden de ingeniería declara rutas vacías, inválidas o un comando no autorizado.
    #[error("ruta o comando no permitido en aurora-forge: {0}")]
    RutaNoPermitida(String),

    /// El agente intentó leer/editar un archivo protegido o fuera de `rutas_permitidas`.
    #[error("bloqueo de seguridad en aurora-forge: '{ruta}' viola regla '{regla}'")]
    ArchivoProtegido {
        /// Ruta relativa infractora detectada en el worktree o en la solicitud.
        ruta: String,
        /// Regla de protección o alcance violada.
        regla: String,
    },

    /// Fallo creando, auditando, commiteando o limpiando el `git worktree` temporal.
    #[error("fallo en git worktree aislado: {0}")]
    WorktreeFallido(String),

    /// La compuerta determinista (`cargo clippy`/`cargo test`/`pnpm build`) falló.
    #[error("compuerta de validación fallida ({comando}): {salida}")]
    ValidacionCompuertaFallida {
        /// Comando de validación que falló.
        comando: String,
        /// Salida resumida del compilador o test runner.
        salida: String,
    },

    /// Se excedió el tiempo límite configurado en ejecución del agente o validación.
    #[error("timeout de ejecución en etapa '{etapa}' tras {timeout_segundos} s")]
    TimeoutEjecucion {
        /// Etapa que excedió el límite temporal.
        etapa: String,
        /// Timeout aplicado en segundos.
        timeout_segundos: u64,
    },

    /// Error propagado desde el consejo multi-agente `aurora-council`.
    #[error("error del consejo multi-agente en forja: {0}")]
    Council(#[from] CouncilError),

    /// Error de E/S en disco local durante la operación del sandbox.
    #[error("error de E/S en aurora-forge: {0}")]
    Io(#[from] std::io::Error),

    /// Error propagado desde el contrato base `aurora-domain`.
    #[error("error de dominio Aurora: {0}")]
    Dominio(#[from] AuroraError),
}
