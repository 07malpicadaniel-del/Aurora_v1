use std::path::PathBuf;
use std::process::ExitCode;

use aurora_daemon::{crear_router, AuroraDaemonState, DaemonConfig};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("aurora_daemon=info,aurora_pulse=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let bind_addr =
        std::env::var("AURORA_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8090".to_string());
    let bob_url = std::env::var("AURORA_BOB_URL")
        .unwrap_or_else(|_| aurora_cortex::DEFAULT_BOB_URL.to_string());
    let bob_slot_id = std::env::var("AURORA_BOB_SLOT_ID")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    let data_dir = std::env::var("AURORA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/aurora_daemon"));
    let modo_offline = matches!(
        std::env::var("AURORA_OFFLINE_MOCK").ok().as_deref(),
        Some("1" | "true" | "TRUE")
    );

    let config = DaemonConfig {
        bind_addr: bind_addr.clone(),
        bob_url: bob_url.clone(),
        bob_slot_id,
        data_dir,
        modo_offline,
    };

    let state = match AuroraDaemonState::inicializar_local(config) {
        Ok(s) => s,
        Err(err) => {
            error!(error = %err, "Fallo inicializando estado de aurora-daemon");
            return ExitCode::FAILURE;
        }
    };

    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(err) => {
            error!(bind_addr = %bind_addr, error = %err, "No se pudo enlazar puerto local");
            return ExitCode::FAILURE;
        }
    };

    info!(
        bind_addr = %bind_addr,
        bob_url = %bob_url,
        bob_slot_id,
        modo_offline,
        "aurora-daemon escuchando para Cockpit axis_v1 y automatizaciones locales"
    );

    let router = crear_router(state);
    if let Err(err) = axum::serve(listener, router)
        .with_graceful_shutdown(esperar_senal_apagado())
        .await
    {
        error!(error = %err, "Error ejecutando servidor HTTP axum");
        return ExitCode::FAILURE;
    }

    info!("aurora-daemon apagado limpiamente");
    ExitCode::SUCCESS
}

async fn esperar_senal_apagado() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = sig.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
