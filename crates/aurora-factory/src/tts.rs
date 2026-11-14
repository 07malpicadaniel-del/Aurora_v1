use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aurora_domain::PerfilPsicologico;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info};

use crate::error::{FactoryError, FactoryResult};

/// Tasa de muestreo estándar PCM para locución en `aurora-factory`.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 22_050;

/// Metadatos resultantes de sintetizar una locución WAV en CPU/RAM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoAudio {
    /// Ruta en NVMe donde se escribió el archivo `.wav`.
    pub ruta_wav: PathBuf,
    /// Duración exacta del audio en milisegundos.
    pub duracion_ms: u64,
    /// Frecuencia de muestreo en Hz (ej. `22050`).
    pub sample_rate_hz: u32,
    /// Número de canales (`1` = mono).
    pub canales: u16,
    /// Tiempo invertido durante la síntesis en milisegundos.
    pub tiempo_renderizado_ms: u64,
}

/// Calibra el parámetro `--length_scale` de Piper en función del [`PerfilPsicologico`].
///
/// Valores > 1.0 generan locución más pausada y analítica; valores < 1.0 imprimen ritmo más ágil.
#[must_use]
pub const fn length_scale_para_perfil(perfil: PerfilPsicologico) -> f32 {
    match perfil {
        PerfilPsicologico::AnaliticoFinanciero => 1.08,
        PerfilPsicologico::ConservadorPatrimonial => 1.12,
        PerfilPsicologico::DinamicoVisual => 0.90,
    }
}

/// Contrato asíncrono para motores TTS locales ejecutados estrictamente en CPU/RAM.
#[async_trait]
pub trait TtsEngine: Send + Sync {
    /// Sintetiza el `guion` adaptado al `perfil` y guarda el resultado PCM WAV en `salida_wav`.
    async fn sintetizar_voz(
        &self,
        guion: &str,
        perfil: PerfilPsicologico,
        salida_wav: &Path,
    ) -> FactoryResult<ResultadoAudio>;
}

/// Configuración para `PiperCpuTtsEngine` (ejecución exclusiva en CPU, sin CUDA/VRAM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPiperCpu {
    /// Ruta o nombre del binario `piper`.
    pub binario_piper: PathBuf,
    /// Ruta al modelo `.onnx` en español (ej. `es_MX-claude-high.onnx`).
    pub modelo_onnx: PathBuf,
    /// Identificador opcional de hablante para modelos multi-voz.
    pub speaker_id: Option<u32>,
    /// Timeout máximo del subproceso en milisegundos.
    pub timeout_ms: u64,
}

impl Default for ConfigPiperCpu {
    fn default() -> Self {
        Self {
            binario_piper: PathBuf::from("piper"),
            modelo_onnx: PathBuf::from("models/tts/es_MX-ald-medium.onnx"),
            speaker_id: None,
            timeout_ms: 45_000,
        }
    }
}

/// Motor real de locución local sobre `piper` ejecutándose en CPU/RAM (`tokio::process`).
#[derive(Debug, Clone)]
pub struct PiperCpuTtsEngine {
    config: ConfigPiperCpu,
}

impl PiperCpuTtsEngine {
    /// Crea una instancia validando configuración básica (sin cargar VRAM).
    #[must_use]
    pub const fn new(config: ConfigPiperCpu) -> Self {
        Self { config }
    }

    /// Devuelve la configuración activa.
    #[must_use]
    pub const fn config(&self) -> &ConfigPiperCpu {
        &self.config
    }

    /// Construye los argumentos deterministas de CLI para `piper` garantizando ejecución CPU pura.
    #[must_use]
    pub fn construir_args(&self, perfil: PerfilPsicologico, salida_wav: &Path) -> Vec<String> {
        let scale = length_scale_para_perfil(perfil);
        let mut args = vec![
            "--model".to_string(),
            self.config.modelo_onnx.display().to_string(),
            "--output_file".to_string(),
            salida_wav.display().to_string(),
            "--length_scale".to_string(),
            format!("{scale:.2}"),
        ];
        if let Some(spk) = self.config.speaker_id {
            args.push("--speaker".to_string());
            args.push(spk.to_string());
        }
        args
    }
}

#[async_trait]
impl TtsEngine for PiperCpuTtsEngine {
    #[tracing::instrument(skip(self, guion), fields(perfil = perfil.como_str(), salida = %salida_wav.display()))]
    async fn sintetizar_voz(
        &self,
        guion: &str,
        perfil: PerfilPsicologico,
        salida_wav: &Path,
    ) -> FactoryResult<ResultadoAudio> {
        if guion.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el guion TTS no puede estar vacío".to_string(),
            ));
        }

        if let Some(parent) = salida_wav.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        let inicio = Instant::now();
        let args = self.construir_args(perfil, salida_wav);
        let timeout_ms = self.config.timeout_ms.max(500);

        let mut cmd = Command::new(&self.config.binario_piper);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            FactoryError::TtsFallido(format!(
                "no se pudo iniciar '{}': {e}",
                self.config.binario_piper.display()
            ))
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(guion.as_bytes())
                .await
                .map_err(|e| FactoryError::TtsFallido(format!("error escribiendo guion a piper: {e}")))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| FactoryError::TtsFallido(format!("error cerrando stdin de piper: {e}")))?;
        }

        let salida_proc = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| FactoryError::SubprocesoTimeout {
            comando: self.config.binario_piper.display().to_string(),
            timeout_ms,
        })?
        .map_err(|e| FactoryError::TtsFallido(format!("fallo esperando a piper: {e}")))?;

        if !salida_proc.status.success() {
            let _ = tokio::fs::remove_file(salida_wav).await;
            let stderr = String::from_utf8_lossy(&salida_proc.stderr);
            return Err(FactoryError::TtsFallido(format!(
                "piper finalizó con código {:?}: {}",
                salida_proc.status.code(),
                stderr.trim()
            )));
        }

        let wav_bytes =
            tokio::fs::read(salida_wav)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: salida_wav.to_path_buf(),
                    source,
                })?;
        let (sample_rate_hz, canales, duracion_ms) = leer_info_wav(&wav_bytes)?;
        let tiempo_renderizado_ms = inicio.elapsed().as_millis() as u64;

        info!(
            duracion_ms,
            sample_rate_hz,
            tiempo_renderizado_ms,
            "Síntesis TTS en CPU completada con Piper"
        );

        Ok(ResultadoAudio {
            ruta_wav: salida_wav.to_path_buf(),
            duracion_ms,
            sample_rate_hz,
            canales,
            tiempo_renderizado_ms,
        })
    }
}

/// Generador determinista en Rust puro de WAV PCM mono 22050 Hz / 16-bit para pruebas offline y CI.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicSilentWavEngine;

#[async_trait]
impl TtsEngine for DeterministicSilentWavEngine {
    #[tracing::instrument(skip(self, guion), fields(perfil = perfil.como_str(), salida = %salida_wav.display()))]
    async fn sintetizar_voz(
        &self,
        guion: &str,
        perfil: PerfilPsicologico,
        salida_wav: &Path,
    ) -> FactoryResult<ResultadoAudio> {
        if guion.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el guion TTS no puede estar vacío".to_string(),
            ));
        }

        let inicio = Instant::now();
        let palabras = guion.split_whitespace().count().max(1);
        let scale = f64::from(length_scale_para_perfil(perfil));
        // Base ~360 ms por palabra escalado por ritmo del perfil psicológico; rango acotado [1.8 s, 120 s].
        let duracion_estimada_ms = ((palabras as f64) * 360.0 * scale)
            .round()
            .clamp(1_800.0, 120_000.0) as u64;

        let sample_rate_hz = DEFAULT_SAMPLE_RATE_HZ;
        let canales: u16 = 1;
        let wav_bytes = generar_wav_pcm_silencio(sample_rate_hz, canales, duracion_estimada_ms);

        if let Some(parent) = salida_wav.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        tokio::fs::write(salida_wav, &wav_bytes)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: salida_wav.to_path_buf(),
                source,
            })?;

        let (_, _, duracion_real_ms) = leer_info_wav(&wav_bytes)?;
        let tiempo_renderizado_ms = inicio.elapsed().as_millis() as u64;

        debug!(
            palabras,
            scale,
            duracion_real_ms,
            bytes = wav_bytes.len(),
            "Archivo WAV RIFF determinista generado en CPU/RAM"
        );

        Ok(ResultadoAudio {
            ruta_wav: salida_wav.to_path_buf(),
            duracion_ms: duracion_real_ms,
            sample_rate_hz,
            canales,
            tiempo_renderizado_ms,
        })
    }
}

/// Construye un archivo WAV PCM 16-bit válido completo con cabecera RIFF de 44 bytes.
#[must_use]
pub fn generar_wav_pcm_silencio(sample_rate_hz: u32, canales: u16, duracion_ms: u64) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let bytes_por_muestra = u64::from(bits_per_sample / 8) * u64::from(canales.max(1));
    let num_muestras = (u64::from(sample_rate_hz) * duracion_ms) / 1000;
    let data_len = (num_muestras * bytes_por_muestra).min(u64::from(u32::MAX - 36)) as u32;

    let byte_rate = sample_rate_hz * u32::from(canales.max(1)) * u32::from(bits_per_sample / 8);
    let block_align = canales.max(1) * (bits_per_sample / 8);
    let riff_size = 36_u32.saturating_add(data_len);

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes()); // PCM chunk size
    out.extend_from_slice(&1_u16.to_le_bytes()); // AudioFormat = 1 (PCM)
    out.extend_from_slice(&canales.max(1).to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(44 + data_len as usize, 0_u8);
    out
}

/// Inspecciona una cabecera RIFF/WAVE y devuelve `(sample_rate_hz, canales, duracion_ms)`.
pub fn leer_info_wav(bytes: &[u8]) -> FactoryResult<(u32, u16, u64)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(FactoryError::TtsFallido(
            "archivo WAV inválido: firma RIFF/WAVE ausente".to_string(),
        ));
    }

    let mut offset = 12_usize;
    let mut canales: Option<u16> = None;
    let mut sample_rate: Option<u32> = None;
    let mut byte_rate: Option<u32> = None;
    let mut data_len: Option<u32> = None;

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let payload_start = offset + 8;
        let payload_end = payload_start.saturating_add(chunk_size).min(bytes.len());

        if chunk_id == b"fmt " && payload_end >= payload_start + 16 {
            let p = &bytes[payload_start..payload_end];
            canales = Some(u16::from_le_bytes([p[2], p[3]]));
            sample_rate = Some(u32::from_le_bytes([p[4], p[5], p[6], p[7]]));
            byte_rate = Some(u32::from_le_bytes([p[8], p[9], p[10], p[11]]));
        } else if chunk_id == b"data" {
            data_len = Some((payload_end - payload_start) as u32);
        }

        let padded_size = chunk_size + (chunk_size % 2);
        let next = payload_start.saturating_add(padded_size);
        if next <= offset {
            break;
        }
        offset = next;
    }

    match (sample_rate, canales, byte_rate, data_len) {
        (Some(sr), Some(ch), Some(br), Some(dl)) if br > 0 => {
            let duracion_ms = (u64::from(dl) * 1000) / u64::from(br);
            Ok((sr, ch, duracion_ms))
        }
        _ => Err(FactoryError::TtsFallido(
            "archivo WAV sin chunks 'fmt ' o 'data' válidos".to_string(),
        )),
    }
}
