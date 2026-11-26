use std::path::PathBuf;
use std::sync::Arc;

use aurora_cortex::{BriefCreativo, CanalContacto, DiagnosticoComercial, EscenaReel};
use aurora_domain::{
    validar_activo, ActivoGenerado, ObjetivoContenidoVideo, PerfilPsicologico, TipoActivoGenerado,
    VerticalComercial,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::broll::BibliotecaBroll;
use crate::document::{
    generar_html_dossier, generar_html_escena, generar_html_overlay_transparente, DocumentEngine,
};
use crate::error::{FactoryError, FactoryResult};
use crate::manifest::{
    construir_metadatos_artefacto, MetadatosArtefacto, ModoComposicionVideo, PaqueteManufacturado,
    SolicitudManufactura,
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
    broll: Option<Arc<Mutex<BibliotecaBroll>>>,
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
            broll: None,
        }
    }

    /// Conecta la biblioteca B-Roll persistente en NVMe para habilitar composición cinematográfica
    /// con degradación automática a diapositivas PNG cuando falten clips para la vertical.
    #[must_use]
    pub fn con_biblioteca_broll(mut self, biblioteca: Arc<Mutex<BibliotecaBroll>>) -> Self {
        self.broll = Some(biblioteca);
        self
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

        // 1b. Consultar BibliotecaBroll si está conectada (rotación justa por usos_acumulados).
        let clips_seleccionados = if let Some(ref broll_lock) = self.broll {
            let mut bib = broll_lock.lock().await;
            bib.seleccionar_clips_para_reel(
                &solicitud.diagnostico.vertical_optima,
                solicitud.prospecto.perfil_psicologico,
                solicitud.brief.escenas_reel.len().max(1),
            )
        } else {
            Vec::new()
        };

        let usar_broll = !clips_seleccionados.is_empty();
        let modo_video = if usar_broll {
            ModoComposicionVideo::CinematograficoBroll
        } else {
            ModoComposicionVideo::DiapositivasEstaticas
        };
        let clips_broll_usados: Vec<uuid::Uuid> =
            clips_seleccionados.iter().map(|c| c.id).collect();
        let rutas_clips_broll: Vec<PathBuf> = clips_seleccionados
            .iter()
            .map(|c| c.ruta_nvme.clone())
            .collect();

        let total_escenas = solicitud.brief.escenas_reel.len();
        let html_visuales: Vec<String> = if usar_broll {
            solicitud
                .brief
                .escenas_reel
                .iter()
                .enumerate()
                .map(|(idx, escena)| {
                    generar_html_overlay_transparente(
                        escena,
                        idx,
                        total_escenas,
                        &solicitud.diagnostico.vertical_optima,
                    )
                })
                .collect()
        } else {
            solicitud
                .brief
                .escenas_reel
                .iter()
                .enumerate()
                .map(|(idx, escena)| {
                    generar_html_escena(
                        escena,
                        &solicitud.brief,
                        &solicitud.diagnostico,
                        idx == 0,
                    )
                })
                .collect()
        };

        // 2. Rama concurrente CPU/RAM: síntesis de voz + maquetado HTML -> PDF & PNG/Overlays
        let fut_tts = self.tts.sintetizar_voz(
            &solicitud.brief.guion_tts,
            solicitud.prospecto.perfil_psicologico,
            &ruta_wav,
        );
        let fut_docs = async {
            let tiempo_pdf = self.document.renderizar_pdf(&html_dossier, &ruta_pdf).await?;
            let visuales = if usar_broll {
                let dir_overlays = dir_orden.join("overlays");
                self.document
                    .renderizar_overlays_transparentes(&html_visuales, &dir_overlays)
                    .await?
            } else {
                self.document
                    .renderizar_frames_png(&html_visuales, dir_frames)
                    .await?
            };
            Ok::<_, FactoryError>((tiempo_pdf, visuales))
        };

        let (audio_res, (tiempo_pdf_ms, visuales)) = tokio::try_join!(fut_tts, fut_docs)?;

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

        // 4. Ensamblaje de Reel vertical MP4 (cinematográfico B-Roll o respaldo diapositivas PNG)
        let video_res = if usar_broll {
            self.video
                .ensamblar_reel_con_broll(
                    &rutas_clips_broll,
                    &visuales,
                    &ruta_wav,
                    &ruta_srt,
                    &ruta_mp4,
                )
                .await?
        } else {
            self.video
                .ensamblar_reel_vertical(&visuales, &ruta_wav, &ruta_srt, &ruta_mp4)
                .await?
        };

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
            modo_video = ?modo_video,
            clips_broll = clips_broll_usados.len(),
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
            modo_video,
            clips_broll_usados,
            generado_en: ahora,
        })
    }

    /// Manufactura un video vertical 9:16 de Carril Verde (cero PII) para monetización/alcance
    /// o venta educativa, respetando duración objetivo (`>= 61s` en `MonetizacionAlcance`),
    /// componiendo con clips B-Roll de Higgsfield cuando existen o degradando a diapositivas PNG.
    #[tracing::instrument(
        skip(self, solicitud),
        fields(
            campana_id = %solicitud.campana_id,
            vertical = solicitud.vertical.etiqueta(),
            objetivo = solicitud.objetivo.etiqueta()
        )
    )]
    pub async fn manufacturar_video_monetizacion(
        &self,
        solicitud: &SolicitudVideoMonetizacion,
    ) -> FactoryResult<PaqueteVideoMonetizacion> {
        solicitud.validar()?;

        let dir_salida = self
            .config
            .directorio_base
            .join("monetizacion")
            .join(solicitud.campana_id.to_string());
        let dir_frames = dir_salida.join("frames");

        tokio::fs::create_dir_all(&dir_salida)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: dir_salida.clone(),
                source,
            })?;

        match self
            .ejecutar_monetizacion_en_directorio(solicitud, &dir_salida, &dir_frames)
            .await
        {
            Ok(paquete) => Ok(paquete),
            Err(err) => {
                warn!(
                    campana_id = %solicitud.campana_id,
                    error = %err,
                    "Limpiando directorio parcial de video de monetización tras fallo"
                );
                let _ = tokio::fs::remove_dir_all(&dir_salida).await;
                Err(err)
            }
        }
    }

    async fn ejecutar_monetizacion_en_directorio(
        &self,
        solicitud: &SolicitudVideoMonetizacion,
        dir_salida: &std::path::Path,
        dir_frames: &std::path::Path,
    ) -> FactoryResult<PaqueteVideoMonetizacion> {
        let ruta_wav = dir_salida.join("locucion_monetizacion.wav");
        let ruta_srt = dir_salida.join("subtitulos.srt");
        let ruta_mp4 = dir_salida.join("reel_monetizacion.mp4");

        let clips_seleccionados = if let Some(ref broll_lock) = self.broll {
            let mut bib = broll_lock.lock().await;
            bib.seleccionar_clips_para_reel(
                &solicitud.vertical,
                solicitud.perfil_objetivo,
                solicitud.escenas_reel.len().max(1),
            )
        } else {
            Vec::new()
        };

        let usar_broll = !clips_seleccionados.is_empty();
        let modo_video = if usar_broll {
            ModoComposicionVideo::CinematograficoBroll
        } else {
            ModoComposicionVideo::DiapositivasEstaticas
        };
        let clips_broll_usados: Vec<Uuid> = clips_seleccionados.iter().map(|c| c.id).collect();
        let rutas_clips_broll: Vec<PathBuf> = clips_seleccionados
            .iter()
            .map(|c| c.ruta_nvme.clone())
            .collect();

        let brief_educativo = BriefCreativo {
            prospecto_id: Uuid::nil(),
            perfil_psicologico: solicitud.perfil_objetivo,
            gancho_3s: solicitud.titulo.clone(),
            tono_narrativo: "Educativo de alta retención, claro y sin datos personales".to_string(),
            guion_tts: solicitud.guion_tts.clone(),
            escenas_reel: solicitud.escenas_reel.clone(),
            secciones_dossier_pdf: vec![solicitud.titulo.clone()],
            canal_sugerido: CanalContacto::LinkedIn,
        };
        let diagnostico_educativo = DiagnosticoComercial {
            prospecto_id: Uuid::nil(),
            vertical_optima: solicitud.vertical,
            producto_gancho: solicitud.titulo.clone(),
            dolor_principal: solicitud.descripcion.clone(),
            angulo_entrada: solicitud.descripcion.clone(),
            probabilidad_cierre: 0.75,
            justificacion_topologica: "Contenido orgánico educativo de Carril Verde sin PII"
                .to_string(),
        };

        let total_escenas = solicitud.escenas_reel.len();
        let html_visuales: Vec<String> = if usar_broll {
            solicitud
                .escenas_reel
                .iter()
                .enumerate()
                .map(|(idx, escena)| {
                    generar_html_overlay_transparente(escena, idx, total_escenas, &solicitud.vertical)
                })
                .collect()
        } else {
            solicitud
                .escenas_reel
                .iter()
                .enumerate()
                .map(|(idx, escena)| {
                    generar_html_escena(escena, &brief_educativo, &diagnostico_educativo, idx == 0)
                })
                .collect()
        };

        let fut_tts = self.tts.sintetizar_voz(
            &solicitud.guion_tts,
            solicitud.perfil_objetivo,
            &ruta_wav,
        );
        let fut_visuales = async {
            if usar_broll {
                let dir_overlays = dir_salida.join("overlays");
                self.document
                    .renderizar_overlays_transparentes(&html_visuales, &dir_overlays)
                    .await
            } else {
                self.document
                    .renderizar_frames_png(&html_visuales, dir_frames)
                    .await
            }
        };

        let (audio_res, visuales) = tokio::try_join!(fut_tts, fut_visuales)?;

        let inicio_srt = std::time::Instant::now();
        let duracion_escenas_seg: u32 = solicitud
            .escenas_reel
            .iter()
            .map(|e| u32::from(e.duracion_segundos))
            .sum();
        let duracion_srt_ms = audio_res
            .duracion_ms
            .max(u64::from(duracion_escenas_seg) * 1_000);
        let contenido_srt = generar_subtitulos_srt(&solicitud.escenas_reel, duracion_srt_ms);
        if contenido_srt.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "generar_subtitulos_srt devolvió contenido vacío para monetización".to_string(),
            ));
        }
        tokio::fs::write(&ruta_srt, contenido_srt.as_bytes())
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: ruta_srt.clone(),
                source,
            })?;
        let tiempo_srt_ms = inicio_srt.elapsed().as_millis() as u64;

        let video_res = if usar_broll {
            self.video
                .ensamblar_reel_con_broll(
                    &rutas_clips_broll,
                    &visuales,
                    &ruta_wav,
                    &ruta_srt,
                    &ruta_mp4,
                )
                .await?
        } else {
            self.video
                .ensamblar_reel_vertical(&visuales, &ruta_wav, &ruta_srt, &ruta_mp4)
                .await?
        };

        let nota_voz_wav = construir_metadatos_artefacto(
            &ruta_wav,
            Some(audio_res.duracion_ms),
            audio_res.tiempo_renderizado_ms,
        )
        .await?;
        let subtitulos_srt =
            construir_metadatos_artefacto(&ruta_srt, Some(duracion_srt_ms), tiempo_srt_ms).await?;
        let reel_vertical_mp4 = construir_metadatos_artefacto(
            &ruta_mp4,
            Some(video_res.duracion_ms),
            video_res.tiempo_renderizado_ms,
        )
        .await?;

        let duracion_estimada_seg = duracion_escenas_seg.max((video_res.duracion_ms / 1_000) as u32);
        let elegible_creator_rewards_tiktok =
            duracion_estimada_seg >= ObjetivoContenidoVideo::MonetizacionAlcance.duracion_objetivo_min_seg();

        info!(
            campana_id = %solicitud.campana_id,
            objetivo = solicitud.objetivo.etiqueta(),
            modo_video = ?modo_video,
            duracion_estimada_seg,
            elegible_creator_rewards_tiktok,
            "Video vertical de monetización/alcance sellado en NVMe"
        );

        Ok(PaqueteVideoMonetizacion {
            campana_id: solicitud.campana_id,
            vertical: solicitud.vertical,
            objetivo: solicitud.objetivo,
            titulo: solicitud.titulo.clone(),
            descripcion: solicitud.descripcion.clone(),
            hashtags: solicitud.hashtags.clone(),
            directorio_salida: dir_salida.to_path_buf(),
            nota_voz_wav,
            subtitulos_srt,
            reel_vertical_mp4,
            modo_video,
            clips_broll_usados,
            duracion_estimada_seg,
            elegible_creator_rewards_tiktok,
            generado_en: Utc::now(),
        })
    }
}

/// Solicitud estructurada para fabricar un video vertical de monetización o venta educativa (cero PII).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolicitudVideoMonetizacion {
    /// Identificador de la campaña orgánica/monetización.
    pub campana_id: Uuid,
    /// Vertical comercial educativa (Inmobiliaria o Seguros).
    pub vertical: VerticalComercial,
    /// Objetivo dual (`MonetizacionAlcance` >= 61s o `VentaDirectaProspecto` 30–55s).
    pub objetivo: ObjetivoContenidoVideo,
    /// Título/gancho público libre de PII.
    pub titulo: String,
    /// Descripción pública educativa + CTA inbound sin PII.
    pub descripcion: String,
    /// Hashtags temáticos limpios.
    #[serde(default)]
    pub hashtags: Vec<String>,
    /// Guion completo en español de México para Piper TTS CPU.
    pub guion_tts: String,
    /// Secuencia de escenas verticales con texto overlay y duración en segundos.
    pub escenas_reel: Vec<EscenaReel>,
    /// Estilo narrativo/ritmo visual objetivo.
    pub perfil_objetivo: PerfilPsicologico,
}

impl SolicitudVideoMonetizacion {
    /// Valida integridad del guion y reglas de duración mínima para Creator Rewards en monetización.
    pub fn validar(&self) -> FactoryResult<()> {
        if self.titulo.trim().is_empty()
            || self.descripcion.trim().is_empty()
            || self.guion_tts.trim().is_empty()
        {
            return Err(FactoryError::DatosInvalidos(
                "titulo, descripcion y guion_tts son obligatorios para manufacturar video".to_string(),
            ));
        }
        if self.escenas_reel.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "escenas_reel debe incluir al menos una escena vertical".to_string(),
            ));
        }
        let duracion_total: u32 = self
            .escenas_reel
            .iter()
            .map(|e| u32::from(e.duracion_segundos))
            .sum();
        if self.objetivo == ObjetivoContenidoVideo::MonetizacionAlcance
            && duracion_total < self.objetivo.duracion_objetivo_min_seg()
        {
            return Err(FactoryError::DatosInvalidos(format!(
                "MonetizacionAlcance exige duración objetivo >= {} s (actual: {} s) para elegibilidad Creator Rewards",
                self.objetivo.duracion_objetivo_min_seg(),
                duracion_total
            )));
        }
        Ok(())
    }
}

/// Resultado sellado con SHA-256 de la manufactura de un video vertical de monetización/alcance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaqueteVideoMonetizacion {
    /// Identificador de la campaña orgánica/monetización.
    pub campana_id: Uuid,
    /// Vertical comercial del contenido.
    pub vertical: VerticalComercial,
    /// Objetivo dual aplicado.
    pub objetivo: ObjetivoContenidoVideo,
    /// Título público sin PII.
    pub titulo: String,
    /// Descripción educativa con CTA inbound.
    pub descripcion: String,
    /// Hashtags asignados.
    pub hashtags: Vec<String>,
    /// Carpeta aislada en NVMe (`<artefactos>/monetizacion/<campana_id>`).
    pub directorio_salida: PathBuf,
    /// Audio neuronal Piper `es_MX` en CPU.
    pub nota_voz_wav: MetadatosArtefacto,
    /// Subtítulos dinámicos `.srt`.
    pub subtitulos_srt: MetadatosArtefacto,
    /// Reel vertical `1080x1920` `.mp4` codificado con `h264_nvenc` / fallback CPU.
    pub reel_vertical_mp4: MetadatosArtefacto,
    /// Indica si se compuso con B-Roll Higgsfield o diapositivas PNG.
    pub modo_video: ModoComposicionVideo,
    /// Clips B-Roll utilizados en la composición.
    pub clips_broll_usados: Vec<Uuid>,
    /// Duración total estimada del Reel en segundos.
    pub duracion_estimada_seg: u32,
    /// `true` si cumple `>= 61 s` para TikTok Creator Rewards.
    pub elegible_creator_rewards_tiktok: bool,
    /// Marca temporal UTC de manufactura.
    pub generado_en: DateTime<Utc>,
}
