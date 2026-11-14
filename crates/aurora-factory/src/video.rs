use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aurora_cortex::EscenaReel;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::error::{FactoryError, FactoryResult};
use crate::tts::leer_info_wav;

/// Códec de video seleccionado por el compositor FFmpeg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodecVideoUsado {
    /// Aceleración hardware NVIDIA NVENC (`h264_nvenc`).
    H264Nvenc,
    /// Fallback por software en CPU (`libx264`).
    Libx264,
    /// Contenedor determinista para pruebas CI sin binario FFmpeg.
    MockContenedor,
}

impl CodecVideoUsado {
    /// Etiqueta del códec para observabilidad y logs.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::H264Nvenc => "h264_nvenc",
            Self::Libx264 => "libx264",
            Self::MockContenedor => "mock_mp4",
        }
    }
}

/// Metadatos resultantes del ensamblaje del Reel vertical `1080x1920`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoVideo {
    /// Ruta final del video `.mp4` en disco NVMe.
    pub ruta_mp4: PathBuf,
    /// Códec efectivo utilizado (`h264_nvenc` o fallback `libx264`).
    pub codec_usado: CodecVideoUsado,
    /// Duración total en milisegundos sincronizada con la pista de voz.
    pub duracion_ms: u64,
    /// Tiempo de codificación invertido en milisegundos.
    pub tiempo_renderizado_ms: u64,
}

/// Convierte milisegundos al formato estricto SRT `HH:MM:SS,mmm`.
#[must_use]
pub fn formatear_timestamp_srt(ms: u64) -> String {
    let horas = ms / 3_600_000;
    let resto_h = ms % 3_600_000;
    let minutos = resto_h / 60_000;
    let resto_m = resto_h % 60_000;
    let segundos = resto_m / 1_000;
    let millis = resto_m % 1_000;
    format!("{horas:02}:{minutos:02}:{segundos:02},{millis:03}")
}

/// Genera subtítulos en formato `.srt` sincronizados al tiempo total de locución WAV.
#[must_use]
pub fn generar_subtitulos_srt(escenas: &[EscenaReel], duracion_total_ms: u64) -> String {
    if escenas.is_empty() {
        return String::new();
    }

    let duracion_efectiva = if duracion_total_ms > 0 {
        duracion_total_ms
    } else {
        escenas
            .iter()
            .map(|e| u64::from(e.duracion_segundos.max(1)) * 1_000)
            .sum::<u64>()
            .max(1_000)
    };

    let peso_total: u64 = escenas
        .iter()
        .map(|e| u64::from(e.duracion_segundos.max(1)))
        .sum();

    let mut out = String::with_capacity(escenas.len() * 96);
    let mut cursor_ms = 0_u64;

    for (idx, escena) in escenas.iter().enumerate() {
        let fin_ms = if idx + 1 == escenas.len() {
            duracion_efectiva
        } else {
            let peso = u64::from(escena.duracion_segundos.max(1));
            let tramo = (duracion_efectiva * peso) / peso_total.max(1);
            (cursor_ms + tramo.max(1)).min(duracion_efectiva)
        };

        let texto_limpio: String = escena
            .texto_overlay
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let texto_final = if texto_limpio.is_empty() {
            format!("Escena {}", idx + 1)
        } else {
            texto_limpio
        };

        let _ = writeln!(out, "{}", idx + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            formatear_timestamp_srt(cursor_ms),
            formatear_timestamp_srt(fin_ms)
        );
        let _ = writeln!(out, "{texto_final}\n");

        cursor_ms = fin_ms;
    }

    out
}

/// Contrato asíncrono para ensamblaje y codificación de Reels verticales (`1080x1920`).
#[async_trait]
pub trait VideoComposer: Send + Sync {
    /// Compone `frames`, `audio_wav` y `srt` en `salida_mp4`.
    async fn ensamblar_reel_vertical(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> FactoryResult<ResultadoVideo>;
}

/// Configuración de `FfmpegNvencComposer` con fallback automático a `libx264`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFfmpeg {
    /// Binario `ffmpeg` en el sistema.
    pub binario_ffmpeg: PathBuf,
    /// Tasa de cuadros por segundo del Reel vertical (`30`).
    pub fps: u32,
    /// Intentar aceleración NVENC (`h264_nvenc`) antes de usar CPU `libx264`.
    pub usar_nvenc: bool,
    /// Preset de NVENC (`p4` equilibrado por defecto).
    pub preset_nvenc: String,
    /// Timeout máximo de codificación en milisegundos.
    pub timeout_ms: u64,
}

impl Default for ConfigFfmpeg {
    fn default() -> Self {
        Self {
            binario_ffmpeg: PathBuf::from("ffmpeg"),
            fps: 30,
            usar_nvenc: true,
            preset_nvenc: "p4".to_string(),
            timeout_ms: 90_000,
        }
    }
}

/// Compositor real sobre `ffmpeg` que prioriza `h264_nvenc` y cae a `libx264` si falta hardware.
#[derive(Debug, Clone)]
pub struct FfmpegNvencComposer {
    config: ConfigFfmpeg,
}

impl FfmpegNvencComposer {
    /// Crea una instancia del compositor FFmpeg/NVENC.
    #[must_use]
    pub const fn new(config: ConfigFfmpeg) -> Self {
        Self { config }
    }

    /// Devuelve la configuración activa.
    #[must_use]
    pub const fn config(&self) -> &ConfigFfmpeg {
        &self.config
    }

    /// Construye argumentos CLI de `ffmpeg` para animar keyframes a `1080x1920`, quemar SRT y mezclar audio.
    pub fn construir_args_ffmpeg(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        duracion_total_ms: u64,
        salida_mp4: &Path,
        codec: CodecVideoUsado,
    ) -> FactoryResult<Vec<String>> {
        if frames.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "se requiere al menos un keyframe PNG para construir el Reel".to_string(),
            ));
        }

        let fps = self.config.fps.max(15);
        let dur_frame_s = ((duracion_total_ms.max(1_000) as f64) / 1_000.0) / (frames.len() as f64);
        let dur_frame_s = dur_frame_s.max(1.0);
        let frames_por_escena = ((dur_frame_s * f64::from(fps)).round() as u64).max(15);

        let mut args = vec!["-y".to_string(), "-hide_banner".to_string()];
        for frame in frames {
            args.push("-loop".to_string());
            args.push("1".to_string());
            args.push("-t".to_string());
            args.push(format!("{dur_frame_s:.3}"));
            args.push("-i".to_string());
            args.push(frame.display().to_string());
        }
        args.push("-i".to_string());
        args.push(audio_wav.display().to_string());

        let mut filter = String::new();
        for i in 0..frames.len() {
            let _ = write!(
                filter,
                "[{i}:v]scale=1080:1920:force_original_aspect_ratio=decrease,pad=1080:1920:(ow-iw)/2:(oh-ih)/2,setsar=1,zoompan=z='min(zoom+0.0006,1.06)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d={frames_por_escena}:s=1080x1920:fps={fps}[v{i}];"
            );
        }
        for i in 0..frames.len() {
            let _ = write!(filter, "[v{i}]");
        }
        let srt_escapado = escapar_ruta_filtro_ffmpeg(srt);
        let _ = write!(
            filter,
            "concat=n={}:v=1:a=0[vcat];[vcat]subtitles='{srt_escapado}':force_style='FontSize=18,PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,BorderStyle=3,MarginV=120'[vout]",
            frames.len()
        );

        args.push("-filter_complex".to_string());
        args.push(filter);
        args.push("-map".to_string());
        args.push("[vout]".to_string());
        args.push("-map".to_string());
        args.push(format!("{}:a", frames.len()));

        match codec {
            CodecVideoUsado::H264Nvenc => {
                args.push("-c:v".to_string());
                args.push("h264_nvenc".to_string());
                args.push("-preset".to_string());
                args.push(self.config.preset_nvenc.clone());
                args.push("-cq".to_string());
                args.push("23".to_string());
            }
            CodecVideoUsado::Libx264 | CodecVideoUsado::MockContenedor => {
                args.push("-c:v".to_string());
                args.push("libx264".to_string());
                args.push("-preset".to_string());
                args.push("medium".to_string());
                args.push("-crf".to_string());
                args.push("21".to_string());
            }
        }

        args.extend([
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "192k".to_string(),
            "-shortest".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
            salida_mp4.display().to_string(),
        ]);

        Ok(args)
    }

    /// Comprueba si `h264_nvenc` está disponible y operativo en el hardware actual;
    /// ante cualquier ausencia devuelve `false` para activar el fallback `libx264`.
    pub async fn detectar_nvenc_disponible(&self) -> bool {
        if !self.config.usar_nvenc {
            return false;
        }

        let probe_encoders = Command::new(&self.config.binario_ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(4), probe_encoders).await else {
            return false;
        };

        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !stdout.contains("h264_nvenc") {
            return false;
        }

        // Prueba relámpago de apertura real del encoder NVENC para no fallar si no hay GPU activa.
        let probe_hw = Command::new(&self.config.binario_ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=64x64:d=0.1",
                "-frames:v",
                "1",
                "-c:v",
                "h264_nvenc",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();

        matches!(
            tokio::time::timeout(Duration::from_secs(5), probe_hw).await,
            Ok(Ok(res)) if res.status.success()
        )
    }

    async fn ejecutar_ffmpeg(&self, args: &[String]) -> FactoryResult<()> {
        let timeout_ms = self.config.timeout_ms.max(1_000);
        let mut cmd = Command::new(&self.config.binario_ffmpeg);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let salida = tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output())
            .await
            .map_err(|_| FactoryError::SubprocesoTimeout {
                comando: self.config.binario_ffmpeg.display().to_string(),
                timeout_ms,
            })?
            .map_err(|e| {
                FactoryError::VideoFfmpegFallido(format!(
                    "no se pudo lanzar '{}': {e}",
                    self.config.binario_ffmpeg.display()
                ))
            })?;

        if !salida.status.success() {
            let stderr = String::from_utf8_lossy(&salida.stderr);
            return Err(FactoryError::VideoFfmpegFallido(format!(
                "ffmpeg falló con código {:?}: {}",
                salida.status.code(),
                stderr.trim()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl VideoComposer for FfmpegNvencComposer {
    #[tracing::instrument(skip(self, frames), fields(frames = frames.len(), salida = %salida_mp4.display()))]
    async fn ensamblar_reel_vertical(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> FactoryResult<ResultadoVideo> {
        validar_insumos_video(frames, audio_wav, srt).await?;
        let wav_bytes =
            tokio::fs::read(audio_wav)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: audio_wav.to_path_buf(),
                    source,
                })?;
        let (_, _, duracion_ms) = leer_info_wav(&wav_bytes)?;

        if let Some(parent) = salida_mp4.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        let inicio = Instant::now();
        let codec_preferido = if self.detectar_nvenc_disponible().await {
            CodecVideoUsado::H264Nvenc
        } else {
            debug!("NVENC no disponible; usando fallback CPU libx264");
            CodecVideoUsado::Libx264
        };

        let args_pref = self.construir_args_ffmpeg(
            frames,
            audio_wav,
            srt,
            duracion_ms,
            salida_mp4,
            codec_preferido,
        )?;

        let codec_final = match self.ejecutar_ffmpeg(&args_pref).await {
            Ok(()) => codec_preferido,
            Err(err_nvenc) if codec_preferido == CodecVideoUsado::H264Nvenc => {
                warn!(error = %err_nvenc, "Fallo h264_nvenc; activando fallback automático a libx264");
                let args_cpu = self.construir_args_ffmpeg(
                    frames,
                    audio_wav,
                    srt,
                    duracion_ms,
                    salida_mp4,
                    CodecVideoUsado::Libx264,
                )?;
                self.ejecutar_ffmpeg(&args_cpu).await?;
                CodecVideoUsado::Libx264
            }
            Err(err) => return Err(err),
        };

        let tiempo_renderizado_ms = inicio.elapsed().as_millis() as u64;
        info!(
            codec = codec_final.as_str(),
            duracion_ms,
            tiempo_renderizado_ms,
            "Reel vertical MP4 ensamblado correctamente"
        );

        Ok(ResultadoVideo {
            ruta_mp4: salida_mp4.to_path_buf(),
            codec_usado: codec_final,
            duracion_ms,
            tiempo_renderizado_ms,
        })
    }
}

/// Compositor determinista offline que valida keyframes/WAV/SRT y escribe un MP4 trazable.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicMockVideoComposer;

#[async_trait]
impl VideoComposer for DeterministicMockVideoComposer {
    #[tracing::instrument(skip(self, frames), fields(frames = frames.len(), salida = %salida_mp4.display()))]
    async fn ensamblar_reel_vertical(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> FactoryResult<ResultadoVideo> {
        validar_insumos_video(frames, audio_wav, srt).await?;

        let wav_bytes =
            tokio::fs::read(audio_wav)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: audio_wav.to_path_buf(),
                    source,
                })?;
        let (_, _, duracion_ms) = leer_info_wav(&wav_bytes)?;

        if let Some(parent) = salida_mp4.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        let inicio = Instant::now();
        let mp4_bytes = construir_mp4_isobmff_minimo(frames.len(), duracion_ms);
        tokio::fs::write(salida_mp4, &mp4_bytes)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: salida_mp4.to_path_buf(),
                source,
            })?;

        let tiempo_renderizado_ms = inicio.elapsed().as_millis() as u64;
        debug!(
            bytes = mp4_bytes.len(),
            duracion_ms,
            "Contenedor MP4 ISO BMFF determinista escrito en NVMe"
        );

        Ok(ResultadoVideo {
            ruta_mp4: salida_mp4.to_path_buf(),
            codec_usado: CodecVideoUsado::MockContenedor,
            duracion_ms,
            tiempo_renderizado_ms,
        })
    }
}

async fn validar_insumos_video(
    frames: &[PathBuf],
    audio_wav: &Path,
    srt: &Path,
) -> FactoryResult<()> {
    if frames.is_empty() {
        return Err(FactoryError::DatosInvalidos(
            "se requiere al menos un frame PNG para ensamblar video".to_string(),
        ));
    }

    for frame in frames {
        let bytes = tokio::fs::read(frame)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: frame.clone(),
                source,
            })?;
        if bytes.len() < 8 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(FactoryError::RenderDocumentoFallido(format!(
                "el frame '{}' no posee firma PNG válida",
                frame.display()
            )));
        }
    }

    let srt_content = tokio::fs::read_to_string(srt)
        .await
        .map_err(|source| FactoryError::IoArtefacto {
            ruta: srt.to_path_buf(),
            source,
        })?;
    if srt_content.trim().is_empty() {
        return Err(FactoryError::DatosInvalidos(format!(
            "el archivo de subtítulos '{}' está vacío",
            srt.display()
        )));
    }

    let wav_bytes =
        tokio::fs::read(audio_wav)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: audio_wav.to_path_buf(),
                source,
            })?;
    let _ = leer_info_wav(&wav_bytes)?;
    Ok(())
}

fn escapar_ruta_filtro_ffmpeg(ruta: &Path) -> String {
    ruta.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

fn construir_mp4_isobmff_minimo(num_frames: usize, duracion_ms: u64) -> Vec<u8> {
    // Caja `ftyp` ISO Base Media File Format estándar (isom/mp42)
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(&24_u32.to_be_bytes());
    ftyp.extend_from_slice(b"ftyp");
    ftyp.extend_from_slice(b"isom");
    ftyp.extend_from_slice(&512_u32.to_be_bytes());
    ftyp.extend_from_slice(b"isom");
    ftyp.extend_from_slice(b"mp42");

    // Caja `free` trazable con metadatos internos (1080x1920, frames y duración).
    let meta = format!("AURORA_V2_REEL_1080x1920;frames={num_frames};duracion_ms={duracion_ms}");
    let free_len = (8 + meta.len()) as u32;
    let mut out = Vec::with_capacity(ftyp.len() + free_len as usize);
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&free_len.to_be_bytes());
    out.extend_from_slice(b"free");
    out.extend_from_slice(meta.as_bytes());
    out
}
