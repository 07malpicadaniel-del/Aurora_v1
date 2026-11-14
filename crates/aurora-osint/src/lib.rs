//! Crate `aurora-osint` — Enjambre de extracción OSINT asíncrona, destilación
//! de gatillos vitales y resolución de entidades con cumplimiento LFPDPPP (Fase 1).
//!
//! Opera exclusivamente sobre CPU (`rayon`) y RAM (`tokio` + `reqwest` con `rustls-tls`)
//! sin instanciar navegadores pesados ni consumir VRAM de inferencia.

pub mod cdp_fetcher;
pub mod distiller;
pub mod error;
pub mod pipeline;
pub mod resolver;
pub mod sources;

pub use cdp_fetcher::{ConfigCdpFetcher, HttpCdpBrowserFetcher};

pub use distiller::{
    clasificar_perfil_psicologico, detectar_gatillos, estimar_capacidad_adquisitiva,
    extraer_contactos, limpiar_ruido_html, normalizar_nombre_persona, normalizar_para_busqueda,
    ContactosExtraidos, DestiladorOsint, ProspectoDestilado,
};
pub use error::{OsintError, OsintResult};
pub use pipeline::{PipelineOsint, ResultadoIngestaOsint};
pub use resolver::{EntidadResuelta, EntityResolver};
pub use sources::{
    CoMencionCruda, ConsultaObjetivo, DirectorioPublicoAdapter, DynamicBrowserFetcher,
    EnjambreOsint, FormatoCrudo, HttpSwarmClient, OsintSource, PoliticaHttp, RegistroCrudo,
    SenalesAbiertasAdapter,
};
