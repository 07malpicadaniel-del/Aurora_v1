use std::path::PathBuf;
use std::sync::Arc;

use aurora_cortex::{
    BriefCreativo, CanalContacto, DeterministicMockBob, DiagnosticoComercial, EscenaReel,
    OrquestadorComercialBob, ToolDispatcher,
};
use aurora_domain::{
    CapacidadAdquisitiva, EstadoAprobacion, GatilloVital, MetadatosLfpdppp, PerfilPsicologico,
    Prospecto, SubverticalInmobiliaria, SubverticalSeguros, TipoActivoGenerado, TipoGatilloVital,
    Ubicacion, VerticalComercial,
};
use aurora_factory::{
    generar_html_dossier, generar_html_escena, generar_subtitulos_srt, leer_info_wav,
    length_scale_para_perfil, sanitizar_html, sha256_bytes, CodecVideoUsado, ConfigFabrica,
    ConfigFfmpeg, ConfigHeadlessBrowser, ConfigPiperCpu, DeterministicMockDocumentEngine,
    DeterministicMockVideoComposer, DeterministicSilentWavEngine, FabricaMultimodalAurora,
    FactoryError, FfmpegNvencComposer, HeadlessBrowserDocumentEngine, PiperCpuTtsEngine,
    SolicitudManufactura, TtsEngine, REEL_ALTO_PX, REEL_ANCHO_PX,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConsultaHibrida, DeterministicSimdEmbedder, IslaVertical,
    MotorMemoriaAurora, MotorRecuperacion,
};
use chrono::Utc;
use uuid::Uuid;

fn prospecto_ejemplo(perfil: PerfilPsicologico, opt_out: bool) -> Prospecto {
    let ahora = Utc::now();
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Mariana Elizondo Cantú".to_string(),
        aliases: vec!["Grupo Industrial Bajío".to_string()],
        ubicacion: Ubicacion {
            ciudad: "León".to_string(),
            estado: "Guanajuato".to_string(),
            zona: Some("Puerto Interior".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 8_400_000,
            patrimonio_estimado_mxn: Some(45_000_000),
        },
        perfil_psicologico: perfil,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "Directorio Industrial Guanajuato".to_string(),
            url_evidencia: "https://datos.ejemplo.gob.mx/gto/empresa-402".to_string(),
            capturado_en: ahora,
            opt_out,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::ExpansionLogistica,
            descripcion: "Apertura de nuevo centro logístico en corredor automotriz Bajío".to_string(),
            ocurrido_en: ahora,
            peso_urgencia: 0.90,
        }],
    }
}

async fn preparar_plan_desde_cortex(
    prospecto: &Prospecto,
) -> Result<aurora_cortex::PlanAtaqueComercial, Box<dyn std::error::Error>> {
    let embedder = Arc::new(DeterministicSimdEmbedder::default());
    let mut memoria = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    memoria.ingestar_lote(
        vec![prospecto.clone()],
        Vec::new(),
        IslaVertical::InmobiliariaComercialIndustrial,
    )?;

    let recuperacion = MotorRecuperacion::default();
    let subgrafos = recuperacion.consultar(
        &memoria,
        &ConsultaHibrida::nueva(
            "expansion centro logistico Bajio Guanajuato",
            Some(IslaVertical::InmobiliariaComercialIndustrial),
        ),
    )?;
    let subgrafo = subgrafos
        .first()
        .ok_or("no se recuperó subgrafo desde aurora-memory")?;

    let campana_id = Uuid::new_v4();
    let orquestador = OrquestadorComercialBob::new(DeterministicMockBob);
    let plan = orquestador
        .generar_plan_ataque(subgrafo, campana_id, &memoria)
        .await?;
    Ok(plan)
}

#[tokio::test]
async fn barrera_gatekeeper_estricta_impide_escribir_archivos(
) -> Result<(), Box<dyn std::error::Error>> {
    let prospecto = prospecto_ejemplo(PerfilPsicologico::AnaliticoFinanciero, false);
    let plan = preparar_plan_desde_cortex(&prospecto).await?;

    let dir_base = std::env::temp_dir().join(format!("aurora_factory_block_{}", Uuid::new_v4()));
    let fabrica = FabricaMultimodalAurora::new(
        ConfigFabrica {
            directorio_base: dir_base.clone(),
        },
        DeterministicSilentWavEngine,
        DeterministicMockDocumentEngine,
        DeterministicMockVideoComposer,
    );

    // 1. Orden en PendienteGatekeeper debe ser rechazada sin escribir nada en disco.
    let solicitud_pendiente = SolicitudManufactura {
        orden: plan.orden_produccion.clone(),
        prospecto: prospecto.clone(),
        diagnostico: plan.diagnostico.clone(),
        brief: plan.brief_creativo.clone(),
    };
    let err_pendiente = fabrica.manufacturar(&solicitud_pendiente).await;
    assert!(matches!(
        err_pendiente,
        Err(FactoryError::NoAprobadoPorGatekeeper {
            estado_actual: EstadoAprobacion::PendienteGatekeeper,
            ..
        })
    ));
    assert!(!dir_base.exists());

    // 2. Orden en Rechazado debe ser rechazada sin escribir nada en disco.
    let mut orden_rechazada = plan.orden_produccion.clone();
    orden_rechazada.rechazar_gatekeeper("Requiere ajustar tono de gancho antes de producir")?;
    let solicitud_rechazada = SolicitudManufactura {
        orden: orden_rechazada,
        prospecto: prospecto.clone(),
        diagnostico: plan.diagnostico.clone(),
        brief: plan.brief_creativo.clone(),
    };
    let err_rechazada = fabrica.manufacturar(&solicitud_rechazada).await;
    assert!(matches!(
        err_rechazada,
        Err(FactoryError::NoAprobadoPorGatekeeper {
            estado_actual: EstadoAprobacion::Rechazado,
            ..
        })
    ));
    assert!(!dir_base.exists());

    // 3. Prospecto con opt_out = true bloqueado incluso si la orden ya estaba aprobada.
    let mut orden_aprobada = plan.orden_produccion;
    orden_aprobada.aprobar_gatekeeper()?;
    let mut prospecto_opt_out = prospecto;
    prospecto_opt_out.lfpdppp.opt_out = true;
    let solicitud_opt_out = SolicitudManufactura {
        orden: orden_aprobada,
        prospecto: prospecto_opt_out,
        diagnostico: plan.diagnostico,
        brief: plan.brief_creativo,
    };
    let err_opt_out = fabrica.manufacturar(&solicitud_opt_out).await;
    assert!(matches!(
        err_opt_out,
        Err(FactoryError::ProspectoOptOut { .. })
    ));
    assert!(!dir_base.exists());

    Ok(())
}

#[tokio::test]
async fn manufactura_end_to_end_cortex_aprobacion_a_factory_nvme(
) -> Result<(), Box<dyn std::error::Error>> {
    let prospecto = prospecto_ejemplo(PerfilPsicologico::DinamicoVisual, false);
    let mut plan = preparar_plan_desde_cortex(&prospecto).await?;

    // Simulación explícita de aprobación en Gatekeeper (human-in-the-box).
    plan.orden_produccion.aprobar_gatekeeper()?;
    assert_eq!(
        plan.orden_produccion.estado(),
        EstadoAprobacion::Aprobado
    );

    let dir_base = std::env::temp_dir().join(format!("aurora_factory_ok_{}", Uuid::new_v4()));
    let fabrica = FabricaMultimodalAurora::new(
        ConfigFabrica {
            directorio_base: dir_base.clone(),
        },
        DeterministicSilentWavEngine,
        DeterministicMockDocumentEngine,
        DeterministicMockVideoComposer,
    );

    let solicitud = SolicitudManufactura {
        orden: plan.orden_produccion.clone(),
        prospecto: prospecto.clone(),
        diagnostico: plan.diagnostico.clone(),
        brief: plan.brief_creativo.clone(),
    };

    let paquete = fabrica.manufacturar(&solicitud).await?;

    // Verificación física y cabeceras reales de PDF, WAV, PNGs y MP4.
    let pdf_bytes = tokio::fs::read(&paquete.dossier_pdf.ruta_nvme).await?;
    assert!(pdf_bytes.starts_with(b"%PDF-1.4"));
    assert!(String::from_utf8_lossy(&pdf_bytes).contains("%%EOF"));
    assert_eq!(paquete.dossier_pdf.sha256_hex.len(), 64);
    assert_eq!(paquete.dossier_pdf.sha256_hex, sha256_bytes(&pdf_bytes));

    let wav_bytes = tokio::fs::read(&paquete.nota_voz_wav.ruta_nvme).await?;
    assert!(wav_bytes.starts_with(b"RIFF"));
    let (sr, ch, dur_wav) = leer_info_wav(&wav_bytes)?;
    assert_eq!(sr, 22_050);
    assert_eq!(ch, 1);
    assert!(dur_wav >= 1_800);
    assert_eq!(paquete.nota_voz_wav.sha256_hex, sha256_bytes(&wav_bytes));

    let frame_1_bytes = tokio::fs::read(paquete.directorio_salida.join("frames/frame_001.png")).await?;
    assert!(frame_1_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    let ancho_ihdr = u32::from_be_bytes([
        frame_1_bytes[16],
        frame_1_bytes[17],
        frame_1_bytes[18],
        frame_1_bytes[19],
    ]);
    let alto_ihdr = u32::from_be_bytes([
        frame_1_bytes[20],
        frame_1_bytes[21],
        frame_1_bytes[22],
        frame_1_bytes[23],
    ]);
    assert_eq!(ancho_ihdr, REEL_ANCHO_PX);
    assert_eq!(alto_ihdr, REEL_ALTO_PX);

    let srt_text = tokio::fs::read_to_string(&paquete.subtitulos_srt.ruta_nvme).await?;
    assert!(srt_text.contains(" --> "));

    let mp4_bytes = tokio::fs::read(&paquete.reel_vertical_mp4.ruta_nvme).await?;
    assert!(mp4_bytes.len() > 24);
    assert_eq!(&mp4_bytes[4..8], b"ftyp");
    assert_eq!(
        paquete.reel_vertical_mp4.sha256_hex,
        sha256_bytes(&mp4_bytes)
    );

    assert_eq!(paquete.activos_actualizados.len(), 4);
    for activo in &paquete.activos_actualizados {
        assert_eq!(activo.estado, EstadoAprobacion::Aprobado);
        assert!(!activo.contenido_o_ruta.is_empty());
        if activo.tipo != TipoActivoGenerado::MensajeDirecto {
            assert!(PathBuf::from(&activo.contenido_o_ruta).exists());
        }
    }

    let _ = ToolDispatcher::verificar_habilitacion_lfpdppp;
    let _ = tokio::fs::remove_dir_all(&dir_base).await;
    Ok(())
}

#[test]
fn sanitizacion_html_y_plantillas_diferenciadas_inmobiliaria_y_seguros(
) -> Result<(), Box<dyn std::error::Error>> {
    let payload_malicioso = "<script>alert(\"xss\")</script> & 'comilla'";
    assert_eq!(
        sanitizar_html(payload_malicioso),
        "&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt; &amp; &#x27;comilla&#x27;"
    );

    let mut prospecto = prospecto_ejemplo(PerfilPsicologico::ConservadorPatrimonial, false);
    prospecto.nombre_normalizado = "Carlos <script>alert(1)</script> & Co.".to_string();
    prospecto.ubicacion.ciudad = "Monterrey\"><img src=x onerror=alert(2)>".to_string();
    prospecto.gatillos[0].descripcion = "Expansión 'crítica' <b onclick=evil()>2026</b>".to_string();

    let escena = EscenaReel {
        indice: 1,
        duracion_segundos: 3,
        descripcion_visual: "Plano con <script>evil()</script>".to_string(),
        texto_overlay: "ROI & <Blindaje> \"Patrimonial\"".to_string(),
    };

    let brief = BriefCreativo {
        prospecto_id: prospecto.id,
        perfil_psicologico: prospecto.perfil_psicologico,
        gancho_3s: "Protección <script>hack()</script> 'fiscal'".to_string(),
        tono_narrativo: "Sobrio & institucional".to_string(),
        guion_tts: "Guion limpio de ejemplo".to_string(),
        escenas_reel: vec![escena.clone()],
        secciones_dossier_pdf: vec!["Sección <iframe src=x>".to_string()],
        canal_sugerido: CanalContacto::CorreoEjecutivo,
    };

    let diag_inm = DiagnosticoComercial {
        prospecto_id: prospecto.id,
        vertical_optima: VerticalComercial::Inmobiliaria(
            SubverticalInmobiliaria::ComercialIndustrial,
        ),
        producto_gancho: "Nave <script>inm()</script>".to_string(),
        dolor_principal: "Capital inmovilizado".to_string(),
        angulo_entrada: "Sale & Leaseback".to_string(),
        probabilidad_cierre: 0.85,
        justificacion_topologica: "Señal de expansión industrial".to_string(),
    };

    let diag_seg = DiagnosticoComercial {
        vertical_optima: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
        producto_gancho: "Hombre Clave <script>seg()</script>".to_string(),
        ..diag_inm.clone()
    };

    let html_inm = generar_html_dossier(&prospecto, &diag_inm, &brief);
    let html_seg = generar_html_dossier(&prospecto, &diag_seg, &brief);
    let html_slide = generar_html_escena(&escena, &brief, &diag_seg, true);

    for html in [&html_inm, &html_seg, &html_slide] {
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img src=x"));
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("<b onclick="));
    }

    assert!(html_inm.contains("PORTAFOLIO INMOBILIARIO ESTRATÉGICO"));
    assert!(html_inm.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html_seg.contains("ARQUITECTURA DE BLINDAJE PATRIMONIAL Y SEGUROS"));
    assert!(html_slide.contains("GANCHO INICIAL 3S"));
    assert!(html_slide.contains("1080px"));
    assert!(html_slide.contains("1920px"));

    Ok(())
}

#[tokio::test]
async fn sincronizacion_srt_ritmo_tts_y_args_cpu_fallback_nvenc(
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Validación SRT continua y sin huecos
    let escenas = vec![
        EscenaReel {
            indice: 1,
            duracion_segundos: 3,
            descripcion_visual: "Gancho".to_string(),
            texto_overlay: "ROI 14.8% anual comprobado".to_string(),
        },
        EscenaReel {
            indice: 2,
            duracion_segundos: 4,
            descripcion_visual: "Detalle".to_string(),
            texto_overlay: "Deducibilidad Art. 27 LISR".to_string(),
        },
        EscenaReel {
            indice: 3,
            duracion_segundos: 3,
            descripcion_visual: "Cierre".to_string(),
            texto_overlay: "Solicita corrida financiera".to_string(),
        },
    ];
    let srt = generar_subtitulos_srt(&escenas, 12_500);
    assert!(srt.contains("1\n00:00:00,000 --> "));
    assert!(srt.contains(" --> 00:00:12,500\nSolicita corrida financiera"));
    assert!(srt.contains("Deducibilidad Art. 27 LISR"));

    // 2. Diferenciación de velocidad TTS en CPU/RAM según PerfilPsicologico
    let dir_tts = std::env::temp_dir().join(format!("aurora_tts_cmp_{}", Uuid::new_v4()));
    let tts = DeterministicSilentWavEngine;
    let guion = "Evaluamos una estrategia patrimonial con deducibilidad fiscal y retorno competitivo en México";
    let audio_conservador = tts
        .sintetizar_voz(
            guion,
            PerfilPsicologico::ConservadorPatrimonial,
            &dir_tts.join("conservador.wav"),
        )
        .await?;
    let audio_dinamico = tts
        .sintetizar_voz(
            guion,
            PerfilPsicologico::DinamicoVisual,
            &dir_tts.join("dinamico.wav"),
        )
        .await?;
    assert!(audio_conservador.duracion_ms > audio_dinamico.duracion_ms);
    assert!(length_scale_para_perfil(PerfilPsicologico::AnaliticoFinanciero) > 1.0);
    assert!(length_scale_para_perfil(PerfilPsicologico::DinamicoVisual) < 1.0);

    // Verificar argumentos CLI de Piper CPU (sin CUDA)
    let piper = PiperCpuTtsEngine::new(ConfigPiperCpu::default());
    let args_piper = piper.construir_args(
        PerfilPsicologico::AnaliticoFinanciero,
        &dir_tts.join("out.wav"),
    );
    assert!(!args_piper.iter().any(|a| a.contains("cuda")));
    assert!(args_piper.contains(&"--length_scale".to_string()));
    assert!(args_piper.contains(&"1.08".to_string()));

    // 3. Verificar --disable-gpu en HeadlessBrowserDocumentEngine
    let browser = HeadlessBrowserDocumentEngine::new(ConfigHeadlessBrowser::default());
    let args_pdf = browser.construir_args_pdf("file:///tmp/dossier.html", &dir_tts.join("a.pdf"));
    let args_png =
        browser.construir_args_screenshot("file:///tmp/escena.html", &dir_tts.join("f.png"));
    assert!(args_pdf.contains(&"--headless=new".to_string()));
    assert!(args_pdf.contains(&"--disable-gpu".to_string()));
    assert!(args_png.contains(&"--disable-gpu".to_string()));
    assert!(args_png.contains(&"--window-size=1080,1920".to_string()));

    // 4. Verificar argumentos NVENC vs fallback libx264 y detección segura sin binario
    let composer_sin_gpu = FfmpegNvencComposer::new(ConfigFfmpeg {
        binario_ffmpeg: PathBuf::from("ffmpeg_inexistente_ci_test"),
        ..ConfigFfmpeg::default()
    });
    assert!(!composer_sin_gpu.detectar_nvenc_disponible().await);

    let frames = vec![dir_tts.join("frame_001.png")];
    let args_nvenc = composer_sin_gpu.construir_args_ffmpeg(
        &frames,
        &audio_dinamico.ruta_wav,
        &dir_tts.join("sub.srt"),
        5_000,
        &dir_tts.join("out_nvenc.mp4"),
        CodecVideoUsado::H264Nvenc,
    )?;
    let args_cpu = composer_sin_gpu.construir_args_ffmpeg(
        &frames,
        &audio_dinamico.ruta_wav,
        &dir_tts.join("sub.srt"),
        5_000,
        &dir_tts.join("out_cpu.mp4"),
        CodecVideoUsado::Libx264,
    )?;
    assert!(args_nvenc.contains(&"h264_nvenc".to_string()));
    assert!(args_cpu.contains(&"libx264".to_string()));

    let _ = tokio::fs::remove_dir_all(&dir_tts).await;
    Ok(())
}
