//! Crate `aurora-factory` — Fábrica de entregables multimodales (Stub de Fase 0).
//!
//! En Fase 4 implementará generación local de dossiers PDF, síntesis TTS en CPU
//! y ensamblaje de video vertical MP4 con apoyo de `ffmpeg`/NVENC bajo supervisión
//! del Gatekeeper (`EstadoAprobacion`).

use aurora_domain::{AuroraError, AuroraResult};
use tracing::debug;

/// Descriptor de estado para el módulo Factory en Fase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactoryStubStatus {
    /// Fase objetivo de implementación completa.
    pub target_phase: u8,
    /// Indica si los pipelines multimodales están habilitados.
    pub enabled: bool,
}

/// Verifica disponibilidad del stub Factory en Fase 0.
pub fn phase0_readiness() -> AuroraResult<FactoryStubStatus> {
    let status = FactoryStubStatus {
        target_phase: 4,
        enabled: false,
    };
    debug!(
        target_phase = status.target_phase,
        enabled = status.enabled,
        "aurora-factory inicializado como stub de Fase 0"
    );
    Ok(status)
}

/// Punto de entrada reservado para Fase 4; retorna error controlado en Fase 0.
pub async fn not_implemented_yet() -> AuroraResult<()> {
    Err(AuroraError::Validacion(
        "aurora-factory se implementará en Fase 4".to_string(),
    ))
}
