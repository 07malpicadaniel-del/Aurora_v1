//! CLI de diagnóstico en vivo (`aurora-smoke`) para Aurora v2.
//!
//! Verifica los 6 subsistemas locales (BOB Qwen 3.8 27B en `:8085`, Piper TTS CPU,
//! Chromium Headless CPU, FFmpeg/NVENC, Browser CDP `:9222` y SMTP TLS) y,
//! si BOB está encendido, ejecuta una inferencia real con gramática GBNF estricta.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use aurora_cortex::{
    construir_contexto_perfilador, gbnf_diagnostico_comercial, parsear_respuesta_estricta,
    BobInferenceBackend, ConfigHttpBob, DiagnosticoComercial, HttpBobCortexClient, RolCognitivo,
};
use aurora_daemon::{AuroraDaemonConfig, EstadoSubsistema, SubsystemProbe};
use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    TipoGatilloVital, Ubicacion,
};
use aurora_memory::{GatilloVigente, IslaVertical, SubgrafoProspecto};
use chrono::Utc;
use reqwest::Client;
use uuid::Uuid;

const MODELO_PREFERIDO_DEFAULT: &str =
    "Qwen3.8-27B-UD-IQ3_S.gguf (Qwen 27B IQ3_S imatrix, VRAM<=14.5GB)";

async fn consultar_modelo_activo_bob(http_client: &Client, bob_url: &str) -> Option<String> {
    let url = format!("{}/props", bob_url.trim_end_matches('/'));
    let resp = http_client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let val: serde_json::Value = resp.json().await.ok()?;
    val.get("model_alias")
        .and_then(|v| v.as_str())
        .or_else(|| val.get("model_path").and_then(|v| v.as_str()))
        .map(str::to_string)
}

#[tokio::main]
async fn main() -> ExitCode {
    println!("==============================================================");
    println!(" Aurora v2 :: Smoke Test de Subsistemas & Inferencia BOB Real");
    println!(" Perfil GPU RTX 5060 Ti (16 GB): {MODELO_PREFERIDO_DEFAULT}");
    println!("==============================================================");

    let config = match AuroraDaemonConfig::cargar_desde_toml_y_entorno() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("[ERROR] No se pudo cargar configuración de aurora-daemon: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "Modo efectivo: {} | BOB URL: {} (slot_id = {})",
        config.modo_efectivo().etiqueta(),
        config.bob_url,
        config.bob_slot_id
    );
    println!();

    let http_client = match Client::builder().timeout(Duration::from_secs(5)).build() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("[ERROR] No se pudo construir cliente HTTP para sondas: {err}");
            return ExitCode::FAILURE;
        }
    };

    let reporte = SubsystemProbe::ejecutar(&config, &http_client).await;

    println!("--- Sonda de 6 Subsistemas Nativos ---");
    for estado in [
        &reporte.bob_llm,
        &reporte.piper_tts,
        &reporte.chromium_headless,
        &reporte.ffmpeg_nvenc,
        &reporte.cdp_browser,
        &reporte.smtp_transport,
    ] {
        imprimir_estado_subsistema(estado);
    }
    println!();

    if !reporte.bob_llm.modo_real_activo {
        println!("[INFO] BOB LLM no responde en {}. Smoke test completado sin inferencia.", config.bob_url);
        println!("       Para encender BOB (Qwen 27B IQ3_S imatrix) en Arch Linux:");
        println!();
        println!("       llama-server -m models/Qwen3.8-27B-UD-IQ3_S.gguf \\");
        println!("         --host 127.0.0.1 --port 8086 -ngl 99 -np 1 -c 8192 -b 512 -ctk q8_0 -ctv q8_0 -fa on");
        return ExitCode::SUCCESS;
    }

    if let Some(modelo_activo) = consultar_modelo_activo_bob(&http_client, &config.bob_url).await {
        println!(
            "Modelo activo en BOB ({}/props): {modelo_activo}",
            config.bob_url.trim_end_matches('/')
        );
    }

    println!(
        "--- Inferencia Real GBNF (Rol: {:?}, slot_id = {}) ---",
        RolCognitivo::Perfilador,
        config.bob_slot_id
    );

    let subgrafo = construir_subgrafo_veracruz_smoke();
    let contexto = construir_contexto_perfilador(&subgrafo);
    let gramatica = gbnf_diagnostico_comercial();

    let cliente_bob = match HttpBobCortexClient::new(ConfigHttpBob {
        url_base: config.bob_url.clone(),
        slot_id: config.bob_slot_id,
        n_predict: 1024,
        max_reintentos: 1,
        ..ConfigHttpBob::default()
    }) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("[ERROR] Configuración inválida para HttpBobCortexClient: {err}");
            return ExitCode::FAILURE;
        }
    };

    let inicio = Instant::now();
    let crudo = match cliente_bob
        .inferir_estructurado(RolCognitivo::Perfilador, &contexto, &gramatica)
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            eprintln!("[ERROR] Falló inferencia estructurada contra BOB: {err}");
            return ExitCode::FAILURE;
        }
    };
    let latencia_ms = inicio.elapsed().as_millis();

    let diagnostico: DiagnosticoComercial =
        match parsear_respuesta_estricta(&crudo, RolCognitivo::Perfilador) {
            Ok(diag) => diag,
            Err(err) => {
                eprintln!("[ERROR] La respuesta de BOB no cumplió el esquema DiagnosticoComercial: {err}");
                eprintln!("Salida recibida:\n{crudo}");
                return ExitCode::FAILURE;
            }
        };

    match serde_json::to_string_pretty(&diagnostico) {
        Ok(pretty) => {
            println!("Latencia de inferencia real GBNF: {latencia_ms} ms");
            println!("{pretty}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("[ERROR] No se pudo serializar DiagnosticoComercial: {err}");
            ExitCode::FAILURE
        }
    }
}

fn imprimir_estado_subsistema(estado: &EstadoSubsistema) {
    let etiqueta = if estado.modo_real_activo {
        "REAL    "
    } else if estado.disponible {
        "MOCK    "
    } else {
        "FALLBACK"
    };
    println!(
        "  [{etiqueta}] {:<20} | {} | {}",
        estado.nombre, estado.ruta_o_endpoint, estado.detalle
    );
}

fn construir_subgrafo_veracruz_smoke() -> SubgrafoProspecto {
    let ahora = Utc::now();
    let gatillo = GatilloVital {
        tipo: TipoGatilloVital::ExpansionLogistica,
        descripcion: "Construcción de centro de distribución refrigerado de 8,500 m² rumbo al recinto portuario de Veracruz y cobertura Hombre Clave para dirección general.".to_string(),
        ocurrido_en: ahora,
        peso_urgencia: 0.94,
    };

    let prospecto = Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Grupo Logístico Sotavento S.A. de C.V.".to_string(),
        aliases: vec!["Logística Sotavento Boca del Río".to_string()],
        ubicacion: Ubicacion {
            ciudad: "Boca del Río".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Corredor Industrial Portuario / Costa de Oro".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 48_000_000,
            patrimonio_estimado_mxn: Some(135_000_000),
        },
        perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "aurora_smoke_cli_veracruz".to_string(),
            url_evidencia: "https://siem.economia.gob.mx/directorio-veracruz-logistica-demo"
                .to_string(),
            capturado_en: ahora,
            opt_out: false,
        },
        gatillos: vec![gatillo.clone()],
    };

    SubgrafoProspecto {
        prospecto,
        isla: IslaVertical::SegurosHombreClave,
        score_compuesto: 0.93,
        similitud_vectorial: 0.91,
        score_temporal_4d: 0.96,
        boost_circulo_cercano: 0.08,
        gatillos_vigentes: vec![GatilloVigente {
            gatillo,
            peso_vigente_4d: 0.94,
            antiguedad_horas: 6.0,
        }],
        vecinos_circulo_cercano: Vec::new(),
        clusteres_afines: vec![
            "expansión portuaria veracruz".to_string(),
            "hombre clave dirección".to_string(),
            "nave industrial boca del río".to_string(),
        ],
    }
}
