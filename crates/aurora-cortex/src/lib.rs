//! Crate `aurora-cortex` — Córtex Comercial de BOB, gramáticas GBNF estrictas
//! y registro determinista de herramientas bajo supervisión Gatekeeper (Fase 3).
//!
//! Diseñado para operar con **un único modelo residente en los 16 GB VRAM**
//! de la RTX 5060 Ti (`llama-server` en `http://127.0.0.1:8085`, `slot_id = 0`),
//! alternando dinámicamente roles cognitivos mediante prompt de sistema y gramática GBNF.

pub mod engine;
pub mod error;
pub mod grammar;
pub mod roles;
pub mod tools;

pub use engine::{
    BobInferenceBackend, ConfigHttpBob, DeterministicMockBob, HttpBobCortexClient,
    OrquestadorComercialBob, PlanAtaqueComercial, DEFAULT_BOB_URL,
};
pub use error::{CortexError, CortexResult};
pub use grammar::{
    gbnf_ajuste_auditor, gbnf_brief_creativo, gbnf_diagnostico_comercial, gbnf_tool_call,
    parsear_respuesta_estricta, validar_sintaxis_gbnf, ValidarRespuestaCortex,
};
pub use roles::{
    construir_contexto_auditor, construir_contexto_director_creativo,
    construir_contexto_perfilador, AjusteEstrategiaAuditor, BriefCreativo, CanalContacto,
    DiagnosticoComercial, EscenaReel, MetricasCampana, RolCognitivo,
};
pub use tools::{
    AuroraToolCall, CalentamientoPendiente, DiagnosticoToolValidado, OrdenProduccionActivo,
    ParametrosOrdenProduccion, PlataformaCalentamiento, ResultadoEjecucionTool,
    TipoInteraccionPeriferica, ToolDispatcher,
};
