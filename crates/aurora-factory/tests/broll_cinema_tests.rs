use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use aurora_cortex::{
    BriefCreativo, CanalContacto, DiagnosticoComercial, EscenaReel, OrdenProduccionActivo,
    ParametrosOrdenProduccion,
};
use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    SubverticalInmobiliaria, SubverticalSeguros, TipoGatilloVital, Ubicacion, VerticalComercial,
};
use aurora_factory::{
    construir_png_rgba_transparente_1080x1920, generar_html_overlay_transparente,
    generar_subtitulos_srt, BibliotecaBroll, CodecVideoUsado, ConfigFabrica, ConfigFfmpeg,
    ConfigHeadlessBrowser, ConfigPiperCpu, DeterministicMockDocumentEngine,
    DeterministicMockVideoComposer, DeterministicSilentWavEngine, DocumentEngine,
    EstadoBriefHiggsfield, FabricaMultimodalAurora, FactoryError, FfmpegNvencComposer,
    HeadlessBrowserDocumentEngine, ModoComposicionVideo, PiperCpuTtsEngine, SolicitudManufactura,
    SolicitudRegistroClipBroll, TtsEngine, VideoComposer,
};
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

fn dir_temporal(prefijo: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn construir_mp4_valido_sintetico(etiqueta: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&24_u32.to_be_bytes());
    out.extend_from_slice(b"ftyp");
    out.extend_from_slice(b"isom");
    out.extend_from_slice(&512_u32.to_be_bytes());
    out.extend_from_slice(b"isom");
    out.extend_from_slice(b"mp42");

    let payload = format!("HIGGSFIELD_BROLL_{etiqueta}");
    let free_len = (8 + payload.len()) as u32;
    out.extend_from_slice(&free_len.to_be_bytes());
    out.extend_from_slice(b"free");
    out.extend_from_slice(payload.as_bytes());
    out
}

fn solicitud_aprobada(
    vertical: VerticalComercial,
    perfil: PerfilPsicologico,
) -> Result<SolicitudManufactura, Box<dyn std::error::Error>> {
    let ahora = Utc::now();
    let prospecto = Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Sofía Villarreal Garza".to_string(),
        aliases: vec!["Desarrollos Norte S.A.".to_string()],
        ubicacion: Ubicacion {
            ciudad: "Monterrey".to_string(),
            estado: "Nuevo León".to_string(),
            zona: Some("Apodaca Industrial".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 12_000_000,
            patrimonio_estimado_mxn: Some(40_000_000),
        },
        perfil_psicologico: perfil,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "DENUE_INEGI".to_string(),
            url_evidencia: "https://www.inegi.org.mx/app/mapa/denue/".to_string(),
            capturado_en: ahora,
            opt_out: false,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::ExpansionLogistica,
            descripcion: "Expansión de flotilla y bodega en Apodaca".to_string(),
            ocurrido_en: ahora,
            peso_urgencia: 0.91,
        }],
    };

    let diagnostico = DiagnosticoComercial {
        prospecto_id: prospecto.id,
        vertical_optima: vertical,
        producto_gancho: "Nave Industrial Clase A + Blindaje Patrimonial".to_string(),
        dolor_principal: "Riesgo de saturación logística sin cobertura patrimonial".to_string(),
        angulo_entrada: "Retorno proyectado con deducibilidad fiscal inmediata".to_string(),
        probabilidad_cierre: 0.87,
        justificacion_topologica: "Coincidencia con clúster industrial del corredor norte".to_string(),
    };

    let brief = BriefCreativo {
        prospecto_id: prospecto.id,
        perfil_psicologico: perfil,
        gancho_3s: "Tu expansión logística en Apodaca puede ser deducible este trimestre".to_string(),
        tono_narrativo: "Directo ejecutivo".to_string(),
        guion_tts: "Hola Sofía, analizamos tu expansión logística en Apodaca y estructuramos una propuesta de alto retorno.".to_string(),
        escenas_reel: vec![
            EscenaReel {
                indice: 1,
                duracion_segundos: 4,
                descripcion_visual: "Toma arquitectónica de parque industrial".to_string(),
                texto_overlay: "Expansión Clase A <script>alert(1)</script>".to_string(),
            },
            EscenaReel {
                indice: 2,
                duracion_segundos: 5,
                descripcion_visual: "Indicadores financieros de plusvalía".to_string(),
                texto_overlay: "TIR 16.4% anual en corredor Apodaca".to_string(),
            },
        ],
        secciones_dossier_pdf: vec![
            "Resumen Ejecutivo".to_string(),
            "Proyección Financiera".to_string(),
        ],
        canal_sugerido: CanalContacto::CorreoEjecutivo,
    };

    let mut orden = OrdenProduccionActivo::nueva_pendiente(ParametrosOrdenProduccion {
        campana_id: Uuid::new_v4(),
        prospecto_id: prospecto.id,
        gancho_3s: brief.gancho_3s.clone(),
        guion_tts: brief.guion_tts.clone(),
        escenas_reel: brief.escenas_reel.clone(),
        secciones_dossier_pdf: brief.secciones_dossier_pdf.clone(),
        canal_sugerido: brief.canal_sugerido,
    })?;
    orden.aprobar_gatekeeper()?;

    Ok(SolicitudManufactura {
        orden,
        prospecto,
        diagnostico,
        brief,
    })
}

#[test]
fn biblioteca_broll_persiste_rota_por_usos_detecta_brechas_y_genera_briefs(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = dir_temporal("aurora_broll_index");
    let broll_dir = tmp.join("assets/broll");
    let fuentes = tmp.join("downloads");
    fs::create_dir_all(&fuentes)?;

    let vertical_ind =
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial);

    let mut bib = BibliotecaBroll::abrir(&broll_dir)?;
    // Inicialmente todas las 8 verticales tienen brecha (< 3 clips).
    assert_eq!(bib.detectar_brechas_cobertura().len(), 8);

    // Registramos 3 clips maestros MP4 para ComercialIndustrial.
    let mut ids_registrados = Vec::new();
    for i in 1..=3 {
        let ruta_src = fuentes.join(format!("higgsfield_ind_{i}.mp4"));
        fs::write(&ruta_src, construir_mp4_valido_sintetico(&format!("clip_{i}")))?;

        let clip = bib.registrar_clip(SolicitudRegistroClipBroll {
            ruta_origen: ruta_src,
            vertical: vertical_ind,
            perfil_objetivo: Some(PerfilPsicologico::AnaliticoFinanciero),
            etiquetas: vec!["parque_industrial".to_string(), format!("toma_{i}")],
            duracion_ms: 5_000,
            ancho: 1080,
            alto: 1920,
            movimiento_camara: "fpv_architectural_glide".to_string(),
        })?;
        assert!(clip.ruta_nvme.is_file());
        assert_eq!(clip.sha256.len(), 64);
        assert_eq!(clip.usos_acumulados, 0);
        ids_registrados.push(clip.id);
    }

    // Ya no hay brecha en ComercialIndustrial (quedan 7 brechas).
    let brechas = bib.detectar_brechas_cobertura();
    assert_eq!(brechas.len(), 7);
    assert!(!brechas.contains(&vertical_ind));

    // Rotación justa por usos_acumulados en 3 selecciones de 1 clip consecutivas.
    let s1 = bib.seleccionar_clips_para_reel(
        &vertical_ind,
        PerfilPsicologico::AnaliticoFinanciero,
        1,
    );
    let s2 = bib.seleccionar_clips_para_reel(
        &vertical_ind,
        PerfilPsicologico::AnaliticoFinanciero,
        1,
    );
    let s3 = bib.seleccionar_clips_para_reel(
        &vertical_ind,
        PerfilPsicologico::AnaliticoFinanciero,
        1,
    );
    assert_eq!(s1.len(), 1);
    assert_eq!(s2.len(), 1);
    assert_eq!(s3.len(), 1);
    assert_ne!(s1[0].id, s2[0].id);
    assert_ne!(s2[0].id, s3[0].id);
    assert_ne!(s1[0].id, s3[0].id);

    // Generación y persistencia de briefs cinematográficos para Higgsfield.
    let briefs = bib.generar_briefs_higgsfield(
        VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
        PerfilPsicologico::ConservadorPatrimonial,
        2,
    )?;
    assert_eq!(briefs.len(), 2);
    assert_eq!(briefs[0].relacion_aspecto, "9:16");
    assert!((4..=6).contains(&briefs[0].duracion_objetivo_seg));
    assert_eq!(briefs[0].estado, EstadoBriefHiggsfield::PendienteGeneracion);

    let marcado = bib.marcar_brief_importado(briefs[0].id, ids_registrados[0])?;
    assert_eq!(marcado.estado, EstadoBriefHiggsfield::Importado);
    assert_eq!(marcado.clip_asociado_id, Some(ids_registrados[0]));

    // Reinstanciar desde disco NVMe y confirmar persistencia atómica intacta.
    drop(bib);
    let bib_recargada = BibliotecaBroll::abrir(&broll_dir)?;
    assert_eq!(bib_recargada.indice().clips.len(), 3);
    assert!(bib_recargada
        .indice()
        .clips
        .iter()
        .all(|c| c.usos_acumulados == 1));
    assert_eq!(bib_recargada.cola_briefs().briefs.len(), 2);
    assert_eq!(
        bib_recargada.cola_briefs().briefs[0].estado,
        EstadoBriefHiggsfield::Importado
    );

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[test]
fn broll_rechaza_path_traversal_y_archivos_sin_firma_ftyp(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = dir_temporal("aurora_broll_sec");
    let mut bib = BibliotecaBroll::abrir(tmp.join("broll"))?;
    let vertical = VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial);

    // 1. Path traversal estricto rechazado.
    let err_traversal = bib.registrar_clip(SolicitudRegistroClipBroll {
        ruta_origen: PathBuf::from("../etc/passwd"),
        vertical,
        perfil_objetivo: None,
        etiquetas: Vec::new(),
        duracion_ms: 4_000,
        ancho: 1080,
        alto: 1920,
        movimiento_camara: "dolly_in_slow".to_string(),
    });
    assert!(matches!(err_traversal, Err(FactoryError::DatosInvalidos(_))));

    // 2. Archivo local sin firma MP4/MOV (`ftyp`) rechazado.
    let falso_mp4 = tmp.join("no_es_video.mp4");
    fs::write(&falso_mp4, b"esto es texto plano malicioso sin cabecera ftyp")?;
    let err_firma = bib.registrar_clip(SolicitudRegistroClipBroll {
        ruta_origen: falso_mp4,
        vertical,
        perfil_objetivo: None,
        etiquetas: Vec::new(),
        duracion_ms: 4_000,
        ancho: 1080,
        alto: 1920,
        movimiento_camara: "dolly_in_slow".to_string(),
    });
    assert!(matches!(err_firma, Err(FactoryError::DatosInvalidos(_))));
    assert!(bib.indice().clips.is_empty());

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[test]
fn overlay_transparente_sanitiza_html_y_png_rgba_posee_canal_alfa() {
    let escena = EscenaReel {
        indice: 1,
        duracion_segundos: 4,
        descripcion_visual: "Fachada <b>industrial</b>".to_string(),
        texto_overlay: "ROI 16% <script>evil()</script>".to_string(),
    };
    let html = generar_html_overlay_transparente(
        &escena,
        0,
        3,
        &VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
    );
    assert!(html.contains("background: transparent"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;evil()&lt;/script&gt;"));

    let browser = HeadlessBrowserDocumentEngine::new(ConfigHeadlessBrowser::default());
    let args = browser.construir_args_screenshot_transparente(
        "file:///tmp/ov.html",
        &PathBuf::from("/tmp/ov.png"),
    );
    assert!(args.iter().any(|a| a == "--disable-gpu"));
    assert!(args.iter().any(|a| a == "--default-background-color=00000000"));

    // Verifica cabecera IHDR RGBA (`color_type = 6`) del PNG transparente determinista.
    let png = construir_png_rgba_transparente_1080x1920();
    assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(png[25], 6, "color_type debe ser 6 (RGBA con canal alfa)");

    // Verifica que FfmpegNvencComposer construya filter_complex con overlay=0:0 y scale=1080:1920.
    let composer = FfmpegNvencComposer::new(ConfigFfmpeg::default());
    let ff_args = composer
        .construir_args_ffmpeg_broll(
            &[PathBuf::from("/tmp/clip1.mp4")],
            &[PathBuf::from("/tmp/ov1.png")],
            &PathBuf::from("/tmp/voz.wav"),
            &PathBuf::from("/tmp/sub.srt"),
            5_000,
            &PathBuf::from("/tmp/out.mp4"),
            CodecVideoUsado::H264Nvenc,
        )
        .ok();
    assert!(ff_args.is_some());
    if let Some(args_vec) = ff_args {
        let joined = args_vec.join(" ");
        assert!(joined.contains("scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,setsar=1"));
        assert!(joined.contains("overlay=0:0:enable='between(t,"));
        assert!(joined.contains("h264_nvenc"));
    }
}

#[tokio::test]
async fn fabrica_alterna_cinematografico_broll_y_fallback_diapositivas_sin_error(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = dir_temporal("aurora_factory_cinema_modes");
    let broll_dir = tmp.join("broll");
    let out_dir = tmp.join("artefactos");
    let src_dir = tmp.join("src");
    fs::create_dir_all(&src_dir)?;

    let vertical_con_broll =
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial);
    let vertical_sin_broll = VerticalComercial::Seguros(SubverticalSeguros::Educativo);

    let mut bib = BibliotecaBroll::abrir(&broll_dir)?;
    for i in 1..=2 {
        let clip_src = src_dir.join(format!("clip_{i}.mp4"));
        fs::write(&clip_src, construir_mp4_valido_sintetico(&format!("ind_{i}")))?;
        bib.registrar_clip(SolicitudRegistroClipBroll {
            ruta_origen: clip_src,
            vertical: vertical_con_broll,
            perfil_objetivo: Some(PerfilPsicologico::AnaliticoFinanciero),
            etiquetas: vec!["industrial".to_string()],
            duracion_ms: 5_000,
            ancho: 1080,
            alto: 1920,
            movimiento_camara: "dolly_in_slow".to_string(),
        })?;
    }

    let broll_shared = Arc::new(Mutex::new(bib));
    let fabrica = FabricaMultimodalAurora::new(
        ConfigFabrica {
            directorio_base: out_dir,
        },
        DeterministicSilentWavEngine,
        DeterministicMockDocumentEngine,
        DeterministicMockVideoComposer,
    )
    .con_biblioteca_broll(Arc::clone(&broll_shared));

    // 1. Vertical CON clips B-Roll -> CinematograficoBroll con overlays PNG RGBA.
    let sol_broll =
        solicitud_aprobada(vertical_con_broll, PerfilPsicologico::AnaliticoFinanciero)?;
    let paq_broll = fabrica.manufacturar(&sol_broll).await?;
    assert_eq!(paq_broll.modo_video, ModoComposicionVideo::CinematograficoBroll);
    assert_eq!(paq_broll.clips_broll_usados.len(), 2);
    assert!(paq_broll.reel_vertical_mp4.ruta_nvme.is_file());
    assert!(paq_broll.directorio_salida.join("overlays/overlay_001.png").is_file());

    // 2. Vertical SIN clips B-Roll -> degradación automática a DiapositivasEstaticas sin fallo.
    let sol_fallback =
        solicitud_aprobada(vertical_sin_broll, PerfilPsicologico::ConservadorPatrimonial)?;
    let paq_fallback = fabrica.manufacturar(&sol_fallback).await?;
    assert_eq!(
        paq_fallback.modo_video,
        ModoComposicionVideo::DiapositivasEstaticas
    );
    assert!(paq_fallback.clips_broll_usados.is_empty());
    assert!(paq_fallback.reel_vertical_mp4.ruta_nvme.is_file());
    assert!(paq_fallback.directorio_salida.join("frames/frame_001.png").is_file());

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn prueba_real_nvenc_broll_en_vivo_si_habilitada(
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("AURORA_RUN_LIVE_NVENC").as_deref() != Ok("1") {
        return Ok(());
    }

    let tmp = dir_temporal("aurora_nvenc_live");
    let clip_sintetico = tmp.join("broll_src.mp4");
    let dir_overlays = tmp.join("overlays");
    let ruta_wav = tmp.join("voz_piper.wav");
    let ruta_srt = tmp.join("subtitulos.srt");
    let salida_mp4 = tmp.join("reel_broll_nvenc.mp4");

    // 1. Generar clip B-Roll sintético 720x1280 @ 30fps (4 s) con ffmpeg lavfi.
    let gen_out = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=720x1280:duration=4:rate=30",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            &clip_sintetico.display().to_string(),
        ])
        .output()
        .await?;
    assert!(
        gen_out.status.success(),
        "ffmpeg testsrc2 falló: {}",
        String::from_utf8_lossy(&gen_out.stderr)
    );

    // 2. Renderizar overlay PNG transparente 1080x1920 con navegador CPU o generador RGBA puro.
    let escena = EscenaReel {
        indice: 1,
        duracion_segundos: 4,
        descripcion_visual: "Parque industrial Clase A en Monterrey".to_string(),
        texto_overlay: "Retorno 16.4% anual en corredor Apodaca".to_string(),
    };
    let html_overlay = generar_html_overlay_transparente(
        &escena,
        0,
        1,
        &VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
    );

    let chrome_bin = PathBuf::from("/usr/bin/google-chrome-stable");
    let overlays = if chrome_bin.is_file() {
        let browser = HeadlessBrowserDocumentEngine::new(ConfigHeadlessBrowser {
            binario_navegador: chrome_bin,
            ..ConfigHeadlessBrowser::default()
        });
        browser
            .renderizar_overlays_transparentes(&[html_overlay], &dir_overlays)
            .await?
    } else {
        DeterministicMockDocumentEngine
            .renderizar_overlays_transparentes(&[html_overlay], &dir_overlays)
            .await?
    };

    // Verificar también que nuestro PNG RGBA puro en Rust decodifica limpio con ffprobe.
    let png_rust_puro = tmp.join("overlay_rust_rgba.png");
    fs::write(&png_rust_puro, construir_png_rgba_transparente_1080x1920())?;
    let probe_png = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,pix_fmt,width,height",
            "-of",
            "csv=p=0",
            &png_rust_puro.display().to_string(),
        ])
        .output()
        .await?;
    assert!(probe_png.status.success());
    let png_info = String::from_utf8_lossy(&probe_png.stdout).trim().to_string();
    println!("LIVE_PNG_RGBA_RUST={png_info}");
    assert!(png_info.contains("png") && png_info.contains("1080,1920"));

    // 3. Sintetizar audio es_MX con Piper TTS en CPU/RAM (fallback silencioso si faltara).
    let home = std::env::var("HOME").unwrap_or_default();
    let piper_bin = PathBuf::from(format!("{home}/.local/bin/piper"));
    let piper_model = PathBuf::from(format!(
        "{home}/.local/share/aurora/models/tts/es_MX-ald-medium.onnx"
    ));
    let audio_res = if piper_bin.is_file() && piper_model.is_file() {
        let piper = PiperCpuTtsEngine::new(ConfigPiperCpu {
            binario_piper: piper_bin,
            modelo_onnx: piper_model,
            ..ConfigPiperCpu::default()
        });
        piper
            .sintetizar_voz(
                "Hola Sofía, tu expansión industrial en Monterrey tiene un retorno proyectado superior al dieciséis por ciento anual.",
                PerfilPsicologico::AnaliticoFinanciero,
                &ruta_wav,
            )
            .await?
    } else {
        DeterministicSilentWavEngine
            .sintetizar_voz(
                "Prueba de audio determinista",
                PerfilPsicologico::AnaliticoFinanciero,
                &ruta_wav,
            )
            .await?
    };
    println!("LIVE_PIPER_WAV_MS={}", audio_res.duracion_ms);

    // 4. Generar subtítulos .srt sincronizados.
    let srt = generar_subtitulos_srt(&[escena], audio_res.duracion_ms);
    fs::write(&ruta_srt, srt.as_bytes())?;

    // 5. Ensamblar Reel cinematográfico real con FfmpegNvencComposer::ensamblar_reel_con_broll.
    let composer = FfmpegNvencComposer::new(ConfigFfmpeg::default());
    let res_video = composer
        .ensamblar_reel_con_broll(&[clip_sintetico], &overlays, &ruta_wav, &ruta_srt, &salida_mp4)
        .await?;
    println!(
        "LIVE_NVENC_RESULT=codec:{} duracion_ms:{} render_ms:{}",
        res_video.codec_usado.as_str(),
        res_video.duracion_ms,
        res_video.tiempo_renderizado_ms
    );
    assert_eq!(res_video.codec_usado, CodecVideoUsado::H264Nvenc);
    assert!(salida_mp4.is_file());

    // 6. Verificar con ffprobe salida exacta 1080x1920 h264 + audio aac.
    let probe_mp4 = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,r_frame_rate",
            "-of",
            "default=noprint_wrappers=1",
            &salida_mp4.display().to_string(),
        ])
        .output()
        .await?;
    assert!(probe_mp4.status.success());
    let mp4_info = String::from_utf8_lossy(&probe_mp4.stdout).to_string();
    println!("LIVE_FFPROBE_MP4:\n{mp4_info}");
    assert!(mp4_info.contains("codec_name=h264"));
    assert!(mp4_info.contains("width=1080"));
    assert!(mp4_info.contains("height=1920"));
    assert!(mp4_info.contains("codec_name=aac"));

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn higgsfield_provider_fallback_manual_api_y_objetivo_monetizacion_61s(
) -> Result<(), Box<dyn std::error::Error>> {
    use aurora_factory::{
        ApiHiggsfieldClient, ConfigHiggsfield, HiggsfieldProvider, ManualQueueHiggsfieldClient,
        ObjetivoContenidoVideo, SolicitudVideoMonetizacion,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let tmp = dir_temporal("aurora_higgsfield_monetizacion");
    let broll_dir = tmp.join("broll");
    let factory_dir = tmp.join("factory");
    let vertical = VerticalComercial::Seguros(SubverticalSeguros::HombreClave);

    let bib_lock = Arc::new(tokio::sync::Mutex::new(BibliotecaBroll::abrir(&broll_dir)?));
    let brief = {
        let mut bib = bib_lock.lock().await;
        bib.generar_briefs_higgsfield(vertical, PerfilPsicologico::AnaliticoFinanciero, 1)?
            .into_iter()
            .next()
            .ok_or("no se generó brief Higgsfield")?
    };

    let manual = ManualQueueHiggsfieldClient;
    assert!(!manual.modo_real_configurado());
    let res_manual = manual.solicitar_o_encolar_clip(&brief, &bib_lock).await?;
    assert!(!res_manual.modo_real_api);
    assert!(res_manual.clip_registrado.is_none());

    let api_sin_key = ApiHiggsfieldClient::new(ConfigHiggsfield::default())?;
    assert!(!api_sin_key.modo_real_configurado());
    let res_fallback = api_sin_key
        .solicitar_o_encolar_clip(&brief, &bib_lock)
        .await?;
    assert!(!res_fallback.modo_real_api);
    assert!(res_fallback.clip_registrado.is_none());

    let mp4_bytes = construir_mp4_valido_sintetico("higgsfield_api_mock");
    let mp4_para_srv = mp4_bytes.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let srv_handle = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                mp4_para_srv.len()
            );
            let _ = stream.write_all(header.as_bytes()).await;
            let _ = stream.write_all(&mp4_para_srv).await;
        }
    });

    let api_real = ApiHiggsfieldClient::new(ConfigHiggsfield {
        api_key: Some("hf_test_key_local".to_string()),
        api_url: format!("http://{addr}/v1/generations"),
        timeout_ms: 5_000,
    })?;
    assert!(api_real.modo_real_configurado());

    let res_api = api_real
        .solicitar_o_encolar_clip(&brief, &bib_lock)
        .await?;
    let _ = srv_handle.await;
    assert!(res_api.modo_real_api);
    let clip = res_api
        .clip_registrado
        .ok_or("ApiHiggsfieldClient debía registrar clip MP4 automáticamente")?;
    assert!(clip.ruta_nvme.is_file());
    assert_eq!(res_api.brief_encolado.estado, EstadoBriefHiggsfield::Importado);

    let fabrica = FabricaMultimodalAurora::new(
        ConfigFabrica {
            directorio_base: factory_dir,
        },
        DeterministicSilentWavEngine,
        DeterministicMockDocumentEngine,
        DeterministicMockVideoComposer,
    )
    .con_biblioteca_broll(Arc::clone(&bib_lock));

    let solicitud_corta = SolicitudVideoMonetizacion {
        campana_id: Uuid::new_v4(),
        vertical,
        objetivo: ObjetivoContenidoVideo::MonetizacionAlcance,
        titulo: "Video demasiado corto para Creator Rewards".to_string(),
        descripcion: "Debe rechazarse por ser menor a 61 segundos.".to_string(),
        hashtags: vec!["HombreClave".to_string()],
        guion_tts: "Guion breve de prueba.".to_string(),
        escenas_reel: vec![EscenaReel {
            indice: 1,
            duracion_segundos: 30,
            descripcion_visual: "Escena corta".to_string(),
            texto_overlay: "Solo 30 segundos".to_string(),
        }],
        perfil_objetivo: PerfilPsicologico::AnaliticoFinanciero,
    };
    let err_corta = fabrica
        .manufacturar_video_monetizacion(&solicitud_corta)
        .await;
    assert!(matches!(err_corta, Err(FactoryError::DatosInvalidos(_))));

    let solicitud_65s = SolicitudVideoMonetizacion {
        campana_id: Uuid::new_v4(),
        vertical,
        objetivo: ObjetivoContenidoVideo::MonetizacionAlcance,
        titulo: "Cómo deducir Seguro Hombre Clave en México".to_string(),
        descripcion: "Análisis educativo sobre Art. 27 LISR sin PII.".to_string(),
        hashtags: vec!["HombreClave".to_string(), "LISR".to_string()],
        guion_tts: "Guía educativa sobre protección de socios clave y deducibilidad fiscal corporativa en México.".to_string(),
        escenas_reel: vec![
            EscenaReel {
                indice: 1,
                duracion_segundos: 20,
                descripcion_visual: "Apertura ejecutiva".to_string(),
                texto_overlay: "Art. 27 Fracc. XI LISR".to_string(),
            },
            EscenaReel {
                indice: 2,
                duracion_segundos: 25,
                descripcion_visual: "Desglose cuantitativo".to_string(),
                texto_overlay: "Protege liquidez corporativa".to_string(),
            },
            EscenaReel {
                indice: 3,
                duracion_segundos: 20,
                descripcion_visual: "Cierre educativo inbound".to_string(),
                texto_overlay: "Guarda este análisis".to_string(),
            },
        ],
        perfil_objetivo: PerfilPsicologico::AnaliticoFinanciero,
    };

    let paquete = fabrica
        .manufacturar_video_monetizacion(&solicitud_65s)
        .await?;
    assert_eq!(paquete.modo_video, ModoComposicionVideo::CinematograficoBroll);
    assert_eq!(paquete.clips_broll_usados, vec![clip.id]);
    assert!(paquete.duracion_estimada_seg >= 61);
    assert!(paquete.elegible_creator_rewards_tiktok);
    assert!(paquete.reel_vertical_mp4.ruta_nvme.is_file());
    assert_eq!(paquete.reel_vertical_mp4.sha256_hex.len(), 64);

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}
