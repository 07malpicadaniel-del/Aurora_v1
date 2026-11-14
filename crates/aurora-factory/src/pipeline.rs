use std::path::PathBuf;

use aurora_domain::{validar_activo, ActivoGenerado, TipoActivoGenerado};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::document::{
    generar_html_dossier, generar_html_escena, DocumentEngine,
};
use crate::error::{FactoryError, FactoryResult};
use crate::manifest::{
    construir_metadatos_artefacto, PaqueteManufacturado, SolicitudManufactura,
};
use crate::tts::TtsEngine;
use crate::video::{generar_subtitulos_srt, VideoComposer};

/// Configuración del orquestador de manufactura multimodal en disco NVMe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFabrica {
    /// Directorio raíz donde se aíslan los paquetes por `<prospecto_id>/<orden_id>`.
    pub directorio_base: PathBuf,
}

impl Default for ConfigFabrica {
    fn default() -> Self {
        Self {
            directorio_base: PathBuf::from("data/aurora_factory"),
        }
    }
}

/// Orquestador multimodal de Fase 4:
/// Valida Gatekeeper (`Aprobado`) + LFPDPPP (`!opt_out`), ejecuta en paralelo TTS + HTML->PDF/PNG
/// en CPU/RAM y ensambla el Reel vertical MP4 con subtítulos SRT.
#[derive(Debug, Clone)]
pub struct FabricaMultimodalAurora<T, D, V>
where
    T: TtsEngine,
    D: DocumentEngine,
    V: VideoComposer,
{
    config: ConfigFabrica,
    tts: T,
    document: D,
    video: V,
}

impl<T, D, V> FabricaMultimodalAurora<T, D, V>
where
    T: TtsEngine,
    D: DocumentEngine,
    V: VideoComposer,
{
    /// Construye la fábrica inyectando los motores desacoplados (producción local o mocks CI).
    #[must_use]
    pub const fn new(config: ConfigFabrica, tts: T, document: D, video: V) -> Self {
        Self {
            config,
            tts,
            document,
            video,
        }
    }

    /// Devuelve referencia a la configuración de salida.
    #[must_use]
    pub const fn config(&self) -> &ConfigFabrica {
        &self.config
    }

    /// Ejecuta la manufactura completa tras superar estrictamente la barrera Zero-Bypass Gatekeeper.
    #[tracing::instrument(
        skip(self, solicitud),
        fields(
            orden_id = %solicitud.orden.orden_id,
            prospecto_id = %solicitud.prospecto.id,
            campana_id = %solicitud.orden.campana_id
        )
    )]
    pub async fn manufacturar(
        &self,
        solicitud: &SolicitudManufactura,
    ) -> FactoryResult<PaqueteManufacturado> {
        // 1. Barrera Zero-Bypass obligatoria ANTES de tocar el sistema de archivos.
        solicitud.validar_barrera_gatekeeper()?;

        let dir_orden = self
            .config
            .directorio_base
            .join(solicitud.prospecto.id.to_string())
            .join(solicitud.orden.orden_id.to_string());
        let dir_frames = dir_orden.join("frames");

        tokio::fs::create_dir_all(&dir_orden)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: dir_orden.clone(),
                source,
            })?;

        match self
            .ejecutar_pipeline_en_directorio(solicitud, &dir_orden, &dir_frames)
            .await
        {
            Ok(paquete) => Ok(paquete),
            Err(err) => {
                warn!(
                    orden_id = %solicitud.orden.orden_id,
                    error = %err,
                    "Limpiando directorio parcial tras fallo en manufactura"
                );
                let _ = tokio::fs::remove_dir_all(&dir_orden).await;
                Err(err)
            }
        }
    }

    async fn ejecutar_pipeline_en_directorio(
        &self,
        solicitud: &SolicitudManufactura,
        dir_orden: &std::path::Path,
        dir_frames: &std::path::Path,
    ) -> FactoryResult<PaqueteManufacturado> {
        let ruta_wav = dir_orden.join("nota_voz.wav");
        let ruta_pdf = dir_orden.join("dossier.pdf");
        let ruta_srt = dir_orden.join("subtitulos.srt");
        let ruta_mp4 = dir_orden.join("reel_vertical.mp4");

        let html_dossier =
            generar_html_dossier(&solicitud.prospecto, &solicitud.diagnostico, &solicitud.brief);
        let html_escenas: Vec<String> = solicitud
            .brief
            .escenas_reel
            .iter()
            .enumerate()
            .map(|(idx, escena)| {
                generar_html_escena(escena, &solicitud.brief, &solicitud.diagnostico, idx == 0)
            })
            .collect();

        // 2. Rama concurrente CPU/RAM: síntesis de voz + maquetado HTML -> PDF & PNG
        let fut_tts = self.tts.sintetizar_voz(
            &solicitud.brief.guion_tts,
            solicitud.prospecto.perfil_psicologico,
            &ruta_wav,
        );
        let fut_docs = async {
            let tiempo_pdf = self.document.renderizar_pdf(&html_dossier, &ruta_pdf).await?;
            let frames = self
                .document
                .renderizar_frames_png(&html_escenas, dir_frames)
                .await?;
            Ok::<_, FactoryError>((tiempo_pdf, frames))
        };

        let (audio_res, (tiempo_pdf_ms, frames)) = tokio::try_join!(fut_tts, fut_docs)?;

        // 3. Generación determinista de subtítulos SRT alineados al WAV generado
        let inicio_srt = std::time::Instant::now();
        let contenido_srt =
            generar_subtitulos_srt(&solicitud.brief.escenas_reel, audio_res.duracion_ms);
        if contenido_srt.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "generar_subtitulos_srt devolvió contenido vacío".to_string(),
            ));
        }
        tokio::fs::write(&ruta_srt, contenido_srt.as_bytes())
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: ruta_srt.clone(),
                source,
            })?;
        let tiempo_srt_ms = inicio_srt.elapsed().as_millis() as u64;

        // 4. Ensamblaje de Reel vertical MP4 (NVENC prioritario o fallback CPU libx264)
        let video_res = self
            .video
            .ensamblar_reel_vertical(&frames, &ruta_wav, &ruta_srt, &ruta_mp4)
            .await?;

        // 5. Cálculo de metadatos y sellado SHA-256 en NVMe
        let dossier_pdf = construir_metadatos_artefacto(&ruta_pdf, None, tiempo_pdf_ms).await?;
        let nota_voz_wav = construir_metadatos_artefacto(
            &ruta_wav,
            Some(audio_res.duracion_ms),
            audio_res.tiempo_renderizado_ms,
        )
        .await?;
        let subtitulos_srt =
            construir_metadatos_artefacto(&ruta_srt, Some(audio_res.duracion_ms), tiempo_srt_ms)
                .await?;
        let reel_vertical_mp4 = construir_metadatos_artefacto(
            &ruta_mp4,
            Some(video_res.duracion_ms),
            video_res.tiempo_renderizado_ms,
        )
        .await?;

        let mensaje_directo_texto = format!(
            "Hola {}, {}. {} — Comparto dossier y cápsula ejecutiva ({}).",
            solicitud.prospecto.nombre_normalizado,
            solicitud.brief.gancho_3s.trim(),
            solicitud.diagnostico.angulo_entrada.trim(),
            solicitud.diagnostico.producto_gancho.trim()
        );

        let ahora = Utc::now();
        let mut activos_actualizados: Vec<ActivoGenerado> =
            solicitud.orden.activos_iniciales().to_vec();
        for activo in &mut activos_actualizados {
            activo.actualizado_en = ahora;
            activo.contenido_o_ruta = match activo.tipo {
                TipoActivoGenerado::DossierPdf => dossier_pdf.ruta_nvme.display().to_string(),
                TipoActivoGenerado::NotaVozTts => nota_voz_wav.ruta_nvme.display().to_string(),
                TipoActivoGenerado::ReelVerticalMp4 => {
                    reel_vertical_mp4.ruta_nvme.display().to_string()
                }
                TipoActivoGenerado::MensajeDirecto => mensaje_directo_texto.clone(),
            };
            validar_activo(activo)?;
        }

        info!(
            orden_id = %solicitud.orden.orden_id,
            codec_video = video_res.codec_usado.as_str(),
            pdf_bytes = dossier_pdf.tamano_bytes,
            wav_ms = audio_res.duracion_ms,
            "PaqueteManufacturado sellado correctamente en NVMe"
        );

        Ok(PaqueteManufacturado {
            orden_id: solicitud.orden.orden_id,
            campana_id: solicitud.orden.campana_id,
            prospecto_id: solicitud.prospecto.id,
            directorio_salida: dir_orden.to_path_buf(),
            dossier_pdf,
            nota_voz_wav,
            reel_vertical_mp4,
            subtitulos_srt,
            mensaje_directo_texto,
            activos_actualizados,
            generado_en: ahora,
        })
    }
}
