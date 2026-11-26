use std::process::ExitCode;
use std::sync::Arc;

use aurora_daemon::{
    crear_router, iniciar_scheduler_dpo, AuroraDaemonConfig, AuroraDaemonState, ModoEjecucion,
};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("aurora_daemon=info,aurora_pulse=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = match AuroraDaemonConfig::cargar_desde_toml_y_entorno() {
        Ok(cfg) => cfg,
        Err(err) => {
            error!(error = %err, "Error cargando configuración de aurora-daemon");
            return ExitCode::FAILURE;
        }
    };

    let state = match AuroraDaemonState::inicializar_local(config.clone()) {
        Ok(s) => s,
        Err(err) => {
            error!(error = %err, "Fallo inicializando estado de aurora-daemon");
            return ExitCode::FAILURE;
        }
    };

    let subsistemas = state.sondear_subsistemas().await;
    info!(
        modo_ejecucion = config.modo_efectivo().etiqueta(),
        data_dir = %config.data_dir_resuelto().display(),
        subsistemas = %subsistemas.resumen_log(),
        "Sondeo inicial de subsistemas completado"
    );

    if matches!(config.modo_efectivo(), ModoEjecucion::EstrictoReal) {
        let faltantes = subsistemas.faltantes_en_modo_estricto();
        if !faltantes.is_empty() {
            error!(
                faltantes = ?faltantes,
                "Modo estricto_real activo y faltan subsistemas críticos; abortando arranque"
            );
            return ExitCode::FAILURE;
        }
    } else if !subsistemas.bob_llm.modo_real_activo {
        warn!(
            bob_url = %config.bob_url,
            "BOB llama-server no detectado al arranque; operando con respaldo híbrido RealConFallback hasta que suba :8085"
        );
    }

    let listener = match TcpListener::bind(&config.bind_addr).await {
        Ok(l) => l,
        Err(err) => {
            error!(bind_addr = %config.bind_addr, error = %err, "No se pudo enlazar puerto local");
            return ExitCode::FAILURE;
        }
    };

    if config.es_bind_remoto_o_tailscale() && config.mobile_token_activo().is_none() {
        warn!(
            bind_addr = %config.bind_addr,
            "aurora-daemon enlazado fuera de loopback sin AURORA_MOBILE_TOKEN; define un Bearer token para acceso seguro vía Tailscale"
        );
    }

    info!(
        bind_addr = %config.bind_addr,
        bob_url = %config.bob_url,
        bob_slot_id = config.bob_slot_id,
        modo_ejecucion = config.modo_efectivo().etiqueta(),
        bearer_auth = config.mobile_token_activo().is_some(),
        "aurora-daemon escuchando para Cockpit axis_v1 y Portal Móvil /mobile"
    );

    let scheduler_handle = iniciar_scheduler_dpo(Arc::clone(&state));
    let state_shutdown = Arc::clone(&state);
    let router = crear_router(state);
    if let Err(err) = axum::serve(listener, router)
        .with_graceful_shutdown(esperar_senal_apagado())
        .await
    {
        error!(error = %err, "Error ejecutando servidor HTTP axum");
        scheduler_handle.detener_y_esperar().await;
        if let Err(shutdown_err) = state_shutdown.apagar_ordenadamente().await {
            error!(error = %shutdown_err, "Error compactando WAL tras fallo de servidor");
        }
        return ExitCode::FAILURE;
    }

    scheduler_handle.detener_y_esperar().await;

    if let Err(err) = state_shutdown.apagar_ordenadamente().await {
        error!(error = %err, "Error durante apagado ordenado (flush/compactación WAL)");
        return ExitCode::FAILURE;
    }

    info!("aurora-daemon apagado limpiamente con WAL 4D y cola Gatekeeper persistidos");
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
