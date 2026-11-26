//! Dominio canónico de Aurora v2.
//!
//! Define los modelos comerciales (Inmobiliario y Asegurador en México),
//! trazabilidad obligatoria bajo la LFPDPPP, validación determinista previa a ingesta
//! y el adaptador de proyección hacia `bob_betta/vector_db`.

pub mod bridge;
pub mod error;
pub mod models;
pub mod validation;

pub use bridge::{uuid_a_node_id_u64, GatilloProyectado, IntoVectorPayload, VectorPayload};
pub use error::{AuroraError, AuroraResult};
pub use models::{
    ActivoGenerado, CampanaVertical, CapacidadAdquisitiva, CatalogacionCompraBob, EstadoAprobacion,
    GatilloVital, MetadatosLfpdppp, NivelProbabilidadCompra, ObjetivoContenidoVideo,
    PerfilPsicologico, Prospecto, RelacionTopologica, SubverticalInmobiliaria, SubverticalSeguros,
    TipoActivoGenerado, TipoGatilloVital, TipoVinculo, Ubicacion, VerticalComercial,
};
pub use validation::{
    validar_activo, validar_campana, validar_catalogacion_compra, validar_gatillo,
    validar_peso_unitario, validar_prospecto, validar_relacion, validar_url_publica,
};
