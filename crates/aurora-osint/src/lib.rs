//! Crate `aurora-osint` — Enjambre de extracción OSINT asíncrona (Stub de Fase 0).
//!
//! En Fase 1 implementará recolectores concurrentes en Tokio sobre fuentes públicas
//! mexicanas (DENUE, registros mercantiles y señales abiertas) aplicando filtros LFPDPPP
//! mediante `aurora-domain::validation` antes de entregar payloads a `bob_betta/vector_db`.

use aurora_domain::{AuroraError, AuroraResult};
use tracing::debug;

/// Descriptor de estado para el módulo OSINT en Fase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsintStubStatus {
    /// Fase objetivo de implementación completa.
    pub target_phase: u8,
    /// Indica si el motor de extracción ya está habilitado.
    pub enabled: bool,
}

/// Verifica disponibilidad del stub OSINT sin inicializar clientes HTTP ni scrapers en Fase 0.
pub fn phase0_readiness() -> AuroraResult<OsintStubStatus> {
    let status = OsintStubStatus {
        target_phase: 1,
        enabled: false,
    };
    debug!(
        target_phase = status.target_phase,
        enabled = status.enabled,
        "aurora-osint inicializado como stub de Fase 0"
    );
    Ok(status)
}

/// Punto de entrada reservado para Fase 1; retorna error controlado en Fase 0.
pub async fn not_implemented_yet() -> AuroraResult<()> {
    Err(AuroraError::Validacion(
        "aurora-osint se implementará en Fase 1".to_string(),
    ))
}
