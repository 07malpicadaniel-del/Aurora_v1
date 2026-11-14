//! Crate `aurora-pulse` — Orquestación de despacho, pulso y puente hacia Axis (Stub de Fase 0).
//!
//! En Fase 5 implementará colas de despacho supervisado por Gatekeeper y telemetría
//! headless hacia el frontend de escritorio en `../axis_v1` (Tauri v2).

use aurora_domain::{AuroraError, AuroraResult};
use tracing::debug;

/// Descriptor de estado para el módulo Pulse en Fase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseStubStatus {
    /// Fase objetivo de implementación completa.
    pub target_phase: u8,
    /// Indica si los conectores de despacho están habilitados.
    pub enabled: bool,
}

/// Verifica disponibilidad del stub Pulse en Fase 0.
pub fn phase0_readiness() -> AuroraResult<PulseStubStatus> {
    let status = PulseStubStatus {
        target_phase: 5,
        enabled: false,
    };
    debug!(
        target_phase = status.target_phase,
        enabled = status.enabled,
        "aurora-pulse inicializado como stub de Fase 0"
    );
    Ok(status)
}

/// Punto de entrada reservado para Fase 5; retorna error controlado en Fase 0.
pub async fn not_implemented_yet() -> AuroraResult<()> {
    Err(AuroraError::Validacion(
        "aurora-pulse se implementará en Fase 5".to_string(),
    ))
}
