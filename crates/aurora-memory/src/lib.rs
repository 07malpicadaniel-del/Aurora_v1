//! Crate `aurora-memory` — Telaraña Topológica 4D, embeddings multilingües en CPU/RAM
//! y persistencia NVMe con WAL + purga física LFPDPPP (Fase 2).
//!
//! Opera estrictamente con 0 MB de VRAM sobre CPU (`rayon`) y RAM, reservando los 16 GB VRAM
//! de la RTX 5060 Ti para la inferencia LLM de BOB en la Fase 3.

pub mod embedder;
pub mod error;
pub mod islands;
pub mod retrieval;
pub mod storage;
pub mod temporal_graph;

pub use embedder::{
    l2_normalize, normalizar_espanol, simd_dot_product, ConfigEmbedderMultilingue,
    DeterministicSimdEmbedder, ModeloMultilingue, MultilingualCpuEmbedder, TextEmbedder,
    DIMENSION_BGE_M3, DIMENSION_E5_BASE, DIMENSION_E5_SMALL,
};
pub use error::{MemoryError, MemoryResult};
pub use islands::{EntradaVectorial, GestorIslas, IslaVectorial, IslaVertical, ResultadoSimilitud};
pub use retrieval::{
    ConsultaHibrida, GatilloVigente, MotorRecuperacion, SubgrafoProspecto, VecinoCirculoCercano,
};
pub use storage::MotorMemoriaAurora;
pub use temporal_graph::{
    AristaTelarana, ConfigDecaimiento4D, GrafoTemporal4D, RegistroNodoProspecto, VecinoPropagado,
};
