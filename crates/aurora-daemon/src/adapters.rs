use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::{
    BobInferenceBackend, CanalContacto, CortexResult, DeterministicMockBob, HttpBobCortexClient,
    RolCognitivo,
};
use aurora_domain::PerfilPsicologico;
use aurora_factory::{
    DeterministicMockDocumentEngine, DeterministicMockVideoComposer, DeterministicSilentWavEngine,
    DocumentEngine, FactoryError, FactoryResult, FfmpegNvencComposer,
    HeadlessBrowserDocumentEngine, PiperCpuTtsEngine, ResultadoAudio, ResultadoVideo, TtsEngine,
    VideoComposer,
};
use aurora_pulse::{
    ChannelTransport, ComprobanteDespacho, ConfigSmtp, DeterministicMockTransport, OrdenDespacho,
    PulseError, PulseResult, SmtpEmailTransport,
};
use tracing::{info, warn};

use crate::config::ModoEjecucion;

/// Adaptador híbrido para BOB Córtex (`HttpBobCortexClient` -> `DeterministicMockBob` en fallback).
pub struct HybridBobBackend {
    pub real: HttpBobCortexClient,
    pub fallback: DeterministicMockBob,
    pub modo: ModoEjecucion,
}

#[async_trait]
impl BobInferenceBackend for HybridBobBackend {
    async fn inferir_estructurado(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        gramatica_gbnf: &str,
    ) -> CortexResult<String> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback
                    .inferir_estructurado(rol, contexto_usuario, gramatica_gbnf)
                    .await
            }
            ModoEjecucion::EstrictoReal => {
                self.real
                    .inferir_estructurado(rol, contexto_usuario, gramatica_gbnf)
                    .await
            }
            ModoEjecucion::RealConFallback => match self
                .real
                .inferir_estructurado(rol, contexto_usuario, gramatica_gbnf)
                .await
            {
                Ok(out) => Ok(out),
                Err(err) => {
                    warn!(
                        error = %err,
                        ?rol,
                        "BOB llama-server no disponible; usando respaldo determinista RealConFallback"
                    );
                    self.fallback
                        .inferir_estructurado(rol, contexto_usuario, gramatica_gbnf)
                        .await
                }
            },
        }
    }
}

/// Adaptador híbrido para Piper TTS en CPU (`PiperCpuTtsEngine` -> `DeterministicSilentWavEngine`).
pub struct HybridTtsEngine {
    pub real: PiperCpuTtsEngine,
    pub fallback: DeterministicSilentWavEngine,
    pub modo: ModoEjecucion,
    pub precheck_listo: bool,
}

#[async_trait]
impl TtsEngine for HybridTtsEngine {
    async fn sintetizar_voz(
        &self,
        guion: &str,
        perfil: PerfilPsicologico,
        salida_wav: &Path,
    ) -> FactoryResult<ResultadoAudio> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback.sintetizar_voz(guion, perfil, salida_wav).await
            }
            ModoEjecucion::EstrictoReal => {
                self.real.sintetizar_voz(guion, perfil, salida_wav).await
            }
            ModoEjecucion::RealConFallback => {
                if !self.precheck_listo {
                    warn!(
                        modelo = %self.real.config().modelo_onnx.display(),
                        "Piper o modelo .onnx ausente; usando síntesis WAV limpia de respaldo"
                    );
                    return self.fallback.sintetizar_voz(guion, perfil, salida_wav).await;
                }
                match self.real.sintetizar_voz(guion, perfil, salida_wav).await {
                    Ok(audio) => Ok(audio),
                    Err(err) => {
                        warn!(error = %err, "Fallo Piper TTS real; cayendo a respaldo WAV");
                        self.fallback.sintetizar_voz(guion, perfil, salida_wav).await
                    }
                }
            }
        }
    }
}

/// Adaptador híbrido para Chromium/Chrome Headless CPU (`HeadlessBrowserDocumentEngine` -> mock).
pub struct HybridDocumentEngine {
    pub real: HeadlessBrowserDocumentEngine,
    pub fallback: DeterministicMockDocumentEngine,
    pub modo: ModoEjecucion,
    pub binario_disponible: bool,
}

#[async_trait]
impl DocumentEngine for HybridDocumentEngine {
    async fn renderizar_pdf(&self, html: &str, salida_pdf: &Path) -> FactoryResult<u64> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => self.fallback.renderizar_pdf(html, salida_pdf).await,
            ModoEjecucion::EstrictoReal => self.real.renderizar_pdf(html, salida_pdf).await,
            ModoEjecucion::RealConFallback => {
                if !self.binario_disponible {
                    warn!(
                        bin = %self.real.config().binario_navegador.display(),
                        "Navegador headless no disponible; usando generador PDF de respaldo"
                    );
                    return self.fallback.renderizar_pdf(html, salida_pdf).await;
                }
                match self.real.renderizar_pdf(html, salida_pdf).await {
                    Ok(ms) => Ok(ms),
                    Err(err) => {
                        warn!(error = %err, "Error renderizando PDF en Chromium; cayendo a respaldo");
                        self.fallback.renderizar_pdf(html, salida_pdf).await
                    }
                }
            }
        }
    }

    async fn renderizar_frames_png(
        &self,
        html_escenas: &[String],
        directorio_frames: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback
                    .renderizar_frames_png(html_escenas, directorio_frames)
                    .await
            }
            ModoEjecucion::EstrictoReal => {
                self.real
                    .renderizar_frames_png(html_escenas, directorio_frames)
                    .await
            }
            ModoEjecucion::RealConFallback => {
                if !self.binario_disponible {
                    warn!(
                        bin = %self.real.config().binario_navegador.display(),
                        "Navegador headless no disponible; usando frames PNG de respaldo"
                    );
                    return self
                        .fallback
                        .renderizar_frames_png(html_escenas, directorio_frames)
                        .await;
                }
                match self
                    .real
                    .renderizar_frames_png(html_escenas, directorio_frames)
                    .await
                {
                    Ok(frames) => Ok(frames),
                    Err(err) => {
                        warn!(error = %err, "Error capturando frames PNG en Chromium; cayendo a respaldo");
                        self.fallback
                            .renderizar_frames_png(html_escenas, directorio_frames)
                            .await
                    }
                }
            }
        }
    }

    async fn renderizar_overlays_transparentes(
        &self,
        html_overlays: &[String],
        directorio_overlays: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback
                    .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
                    .await
            }
            ModoEjecucion::EstrictoReal => {
                self.real
                    .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
                    .await
            }
            ModoEjecucion::RealConFallback => {
                if !self.binario_disponible {
                    warn!(
                        bin = %self.real.config().binario_navegador.display(),
                        "Navegador headless no disponible; usando overlays PNG RGBA de respaldo"
                    );
                    return self
                        .fallback
                        .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
                        .await;
                }
                match self
                    .real
                    .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
                    .await
                {
                    Ok(overlays) => Ok(overlays),
                    Err(err) => {
                        warn!(
                            error = %err,
                            "Error capturando overlays PNG transparentes en Chromium; cayendo a respaldo"
                        );
                        self.fallback
                            .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
                            .await
                    }
                }
            }
        }
    }
}

/// Adaptador híbrido para FFmpeg NVENC/CPU (`FfmpegNvencComposer` -> `DeterministicMockVideoComposer`).
pub struct HybridVideoComposer {
    pub real: FfmpegNvencComposer,
    pub fallback: DeterministicMockVideoComposer,
    pub modo: ModoEjecucion,
    pub binario_disponible: bool,
}

#[async_trait]
impl VideoComposer for HybridVideoComposer {
    async fn ensamblar_reel_vertical(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> FactoryResult<ResultadoVideo> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback
                    .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
                    .await
            }
            ModoEjecucion::EstrictoReal => {
                if !self.binario_disponible {
                    return Err(FactoryError::VideoFfmpegFallido(format!(
                        "binario ffmpeg '{}' requerido en modo estricto_real",
                        self.real.config().binario_ffmpeg.display()
                    )));
                }
                self.real
                    .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
                    .await
            }
            ModoEjecucion::RealConFallback => {
                if !self.binario_disponible {
                    warn!("Binario ffmpeg ausente; usando compositor MP4 de respaldo");
                    return self
                        .fallback
                        .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
                        .await;
                }
                match self
                    .real
                    .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
                    .await
                {
                    Ok(vid) => Ok(vid),
                    Err(err) => {
                        warn!(error = %err, "Fallo FFmpeg ensamblando Reel; cayendo a respaldo");
                        self.fallback
                            .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
                            .await
                    }
                }
            }
        }
    }

    async fn ensamblar_reel_con_broll(
        &self,
        clips_broll: &[PathBuf],
        overlays_png: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> FactoryResult<ResultadoVideo> {
        match self.modo {
            ModoEjecucion::SimuladoOffline => {
                self.fallback
                    .ensamblar_reel_con_broll(clips_broll, overlays_png, audio_wav, srt, salida_mp4)
                    .await
            }
            ModoEjecucion::EstrictoReal => {
                if !self.binario_disponible {
                    return Err(FactoryError::VideoFfmpegFallido(format!(
                        "binario ffmpeg '{}' requerido en modo estricto_real",
                        self.real.config().binario_ffmpeg.display()
                    )));
                }
                self.real
                    .ensamblar_reel_con_broll(clips_broll, overlays_png, audio_wav, srt, salida_mp4)
                    .await
            }
            ModoEjecucion::RealConFallback => {
                if !self.binario_disponible {
                    warn!("Binario ffmpeg ausente; usando compositor B-Roll de respaldo");
                    return self
                        .fallback
                        .ensamblar_reel_con_broll(
                            clips_broll,
                            overlays_png,
                            audio_wav,
                            srt,
                            salida_mp4,
                        )
                        .await;
                }
                match self
                    .real
                    .ensamblar_reel_con_broll(clips_broll, overlays_png, audio_wav, srt, salida_mp4)
                    .await
                {
                    Ok(vid) => Ok(vid),
                    Err(err) => {
                        warn!(
                            error = %err,
                            "Fallo FFmpeg ensamblando Reel B-Roll; cayendo a respaldo"
                        );
                        self.fallback
                            .ensamblar_reel_con_broll(
                                clips_broll,
                                overlays_png,
                                audio_wav,
                                srt,
                                salida_mp4,
                            )
                            .await
                    }
                }
            }
        }
    }
}

/// Enrutador híbrido de despacho multicanal (`CorreoEjecutivo` real vía SMTP rustls + fallback seguro).
pub struct HybridChannelTransport {
    pub smtp: Option<Arc<SmtpEmailTransport>>,
    pub smtp_config: ConfigSmtp,
    pub fallback: DeterministicMockTransport,
    pub modo: ModoEjecucion,
}

#[async_trait]
impl ChannelTransport for HybridChannelTransport {
    async fn enviar_paquete(&self, envio: &OrdenDespacho) -> PulseResult<ComprobanteDespacho> {
        if matches!(self.modo, ModoEjecucion::SimuladoOffline) {
            return self.fallback.enviar_paquete(envio).await;
        }

        if envio.canal == CanalContacto::CorreoEjecutivo {
            if let Some(ref smtp) = self.smtp {
                if self.smtp_config.habilitado {
                    // Si SMTP está habilitado explícitamente, errores de destinatario, opt-out o red se propagan reales.
                    return smtp.enviar_paquete(envio).await;
                }
            }

            if matches!(self.modo, ModoEjecucion::EstrictoReal) {
                return Err(PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: None,
                    detalle: "smtp.habilitado=false en modo estricto_real".to_string(),
                });
            }

            warn!(
                prospecto_id = %envio.prospecto.id,
                "SMTP deshabilitado en aurora-daemon; despachando CorreoEjecutivo por simulación segura RealConFallback"
            );
            return self.fallback.enviar_paquete(envio).await;
        }

        if matches!(self.modo, ModoEjecucion::EstrictoReal) {
            return Err(PulseError::DespachoFallido {
                canal: format!("{:?}", envio.canal),
                tracking_id: None,
                detalle: "solo CorreoEjecutivo (SMTP) cuenta con conector nativo en esta versión; otros canales requieren modo real_con_fallback".to_string(),
            });
        }

        info!(
            canal = ?envio.canal,
            prospecto_id = %envio.prospecto.id,
            "Canal sin conector externo directo aún; registrando despacho auditado en RealConFallback"
        );
        self.fallback.enviar_paquete(envio).await
    }
}
