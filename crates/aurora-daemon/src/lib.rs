//! Crate `aurora-daemon` — Servicio HTTP/SSE persistente local para Aurora v2 (Fase 6).
//!
//! Expone toda la orquestación autónoma (`aurora-osint`, `aurora-memory`, `aurora-cortex`,
//! `aurora-factory`, `aurora-pulse`) en `127.0.0.1:8090` (o `AURORA_BIND_ADDR`) de forma
//! desacoplada de la ventana de escritorio `axis_v1` en Wayland/Hyprland.

pub mod error;
pub mod routes;
pub mod state;

pub use error::{DaemonError, DaemonResult};
pub use routes::crear_router;
pub use state::{
    AprobarOrdenRequest, AprobarOrdenResponse, AristaTelaranaDto, AuroraDaemonState, DaemonConfig,
    DpoConsolidarResponse, EventoDaemon, GatekeeperPendientesResponse, GatilloTelaranaDto,
    HealthResponse, HuntRequest, HuntResponse, NodoTelaranaDto, OptOutRequest, OptOutResponse,
    OrdenPendienteDto, PulseEstadoResponse, RechazarOrdenRequest, ResumenIslaDto,
    TelaranaResponse,
};
