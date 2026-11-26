//! Crate `aurora-factory` — Fábrica Multimodal de Dossiers PDF, Locución TTS
//! en CPU/RAM y Reels Verticales con FFmpeg/NVENC bajo supervisión Gatekeeper (Fase 4).
//!
//! Invariantes de hardware y seguridad:
//! - Cero bypass de Gatekeeper: exige `EstadoAprobacion::Aprobado` y `!opt_out` antes de escribir en NVMe.
//! - TTS (`piper`) y maquetado HTML/PDF/PNG (`chromium --disable-gpu`) operan 100% en CPU/RAM.
//! - `FfmpegNvencComposer` prioriza `h264_nvenc` con fallback automático a `libx264`.

pub mod broll;
pub mod document;
pub mod error;
pub mod manifest;
pub mod pipeline;
pub mod tts;
pub mod video;

pub use aurora_domain::ObjetivoContenidoVideo;
pub use broll::{
    validar_firma_mp4_mov, validar_ruta_sin_traversal, ApiHiggsfieldClient, BibliotecaBroll,
    BriefHiggsfield, CatalogoBroll, ClipBroll, ColaBriefsHiggsfield, ConfigHiggsfield,
    EstadoBriefHiggsfield, HiggsfieldProvider, IndiceBroll, ManualQueueHiggsfieldClient,
    ResultadoGeneracionHiggsfield, SolicitudRegistroClipBroll, DEFAULT_HIGGSFIELD_API_URL,
    MIN_CLIPS_COBERTURA_VERTICAL, TODAS_LAS_VERTICALES,
};
pub use document::{
    construir_png_rgba_transparente_1080x1920, generar_html_dossier, generar_html_escena,
    generar_html_overlay_transparente, sanitizar_html, ConfigHeadlessBrowser,
    DeterministicMockDocumentEngine, DocumentEngine, HeadlessBrowserDocumentEngine,
    REEL_ALTO_PX, REEL_ANCHO_PX,
};
pub use error::{FactoryError, FactoryResult};
pub use manifest::{
    construir_metadatos_artefacto, fnv1a_64_bytes, sha256_bytes, MetadatosArtefacto,
    ModoComposicionVideo, PaqueteManufacturado, SolicitudManufactura,
};
pub use pipeline::{
    ConfigFabrica, FabricaMultimodalAurora, PaqueteVideoMonetizacion, SolicitudVideoMonetizacion,
};
pub use tts::{
    generar_wav_pcm_silencio, leer_info_wav, length_scale_para_perfil, ConfigPiperCpu,
    DeterministicSilentWavEngine, PiperCpuTtsEngine, ResultadoAudio, TtsEngine,
    DEFAULT_SAMPLE_RATE_HZ,
};
pub use video::{
    formatear_timestamp_srt, generar_subtitulos_srt, CodecVideoUsado, ConfigFfmpeg,
    DeterministicMockVideoComposer, FfmpegNvencComposer, ResultadoVideo, VideoComposer,
};
