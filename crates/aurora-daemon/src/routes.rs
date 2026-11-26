use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use aurora_council::{ReporteMisionProyecto, ResultadoDelegacion, SolicitudMisionProyecto};
use aurora_factory::ClipBroll;
use aurora_pulse::{EstadoCampanaProspecto, EventoTelemetria};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::config::ReporteSubsistemas;
use crate::error::{DaemonError, DaemonResult};
use crate::mobile_ui::{
    handler_mobile_portal, respuesta_no_autorizada, verificar_cabecera_bearer,
};
use crate::state::{
    AprobarOrdenRequest, AprobarOrdenResponse, AuroraDaemonState, BrollEstadoResponse,
    CatalogarProspectosRequest, CatalogarProspectosResponse, CouncilCuotasResponse,
    CrearOrdenForjaRequest, DelegarConsejoRequest, DpoConsolidarResponse, FiltroProspectosDbQuery,
    ForgeOrdenesResponse, GatekeeperPendientesResponse, GenerarBriefsHiggsfieldRequest,
    GenerarBriefsHiggsfieldResponse, GenerarMonetizacionRequest, GenerarMonetizacionResponse,
    HealthResponse, HuntRequest, HuntResponse, OptOutRequest, OptOutResponse, OrdenPendienteDto,
    ProspectosDbResponse, PublicarOrganicoRequest, PublicarOrganicoResponse, PulseEstadoResponse,
    PulseOrganicoResponse, RechazarOrdenRequest, RegistrarClipBrollRequest, RegistroOrdenForjaDto,
    SubdirectorAgentesResponse, SubdirectorMisionesResponse, TelaranaResponse,
};
use aurora_memory::SubgrafoProspecto;

/// Construye el router HTTP + portal móvil + stream SSE/WS completo para `aurora-daemon`.
pub fn crear_router(state: Arc<AuroraDaemonState>) -> Router {
    let api_router = Router::new()
        .route("/api/v1/health", get(handler_health))
        .route("/api/v1/health/subsistemas", get(handler_subsistemas))
        .route("/api/v1/osint/hunt", post(handler_hunt))
        .route("/api/v1/memory/telarana", get(handler_telarana))
        .route("/api/v1/prospectos/db", get(handler_prospectos_db))
        .route(
            "/api/v1/prospectos/catalogar",
            post(handler_prospectos_catalogar),
        )
        .route(
            "/api/v1/memory/subgrafo/{prospecto_id}",
            get(handler_subgrafo),
        )
        .route(
            "/api/v1/cortex/generar-plan/{prospecto_id}",
            post(handler_cortex_generar_plan),
        )
        .route(
            "/api/v1/gatekeeper/pendientes",
            get(handler_gatekeeper_pendientes),
        )
        .route(
            "/api/v1/gatekeeper/aprobar/{orden_id}",
            post(handler_gatekeeper_aprobar),
        )
        .route(
            "/api/v1/gatekeeper/rechazar/{orden_id}",
            post(handler_gatekeeper_rechazar),
        )
        .route("/api/v1/lfpdppp/opt-out", post(handler_lfpdppp_opt_out))
        .route("/api/v1/pulse/estado", get(handler_pulse_estado))
        .route("/api/v1/pulse/telemetria", post(handler_pulse_telemetria))
        .route("/api/v1/pulse/organico", get(handler_pulse_organico))
        .route(
            "/api/v1/pulse/organico/publicar",
            post(handler_pulse_organico_publicar),
        )
        .route("/api/v1/dpo/consolidar", post(handler_dpo_consolidar))
        .route("/api/v1/council/cuotas", get(handler_council_cuotas))
        .route("/api/v1/council/delegar", post(handler_council_delegar))
        .route(
            "/api/v1/subdirector/agentes",
            get(handler_subdirector_agentes),
        )
        .route(
            "/api/v1/subdirector/misiones",
            get(handler_subdirector_misiones_listar).post(handler_subdirector_misiones_lanzar),
        )
        .route(
            "/api/v1/forge/ordenes",
            get(handler_forge_ordenes_listar).post(handler_forge_ordenes_crear),
        )
        .route("/api/v1/factory/broll", get(handler_factory_broll_estado))
        .route(
            "/api/v1/factory/broll/registrar",
            post(handler_factory_broll_registrar),
        )
        .route(
            "/api/v1/factory/broll/briefs",
            post(handler_factory_broll_briefs),
        )
        .route(
            "/api/v1/content/generar-monetizacion",
            post(handler_content_generar_monetizacion),
        )
        .route("/api/v1/eventos/stream", get(handler_eventos_sse))
        .route("/api/v1/ws", get(handler_eventos_ws))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            middleware_auth_mobile,
        ));

    Router::new()
        .route("/mobile", get(handler_mobile_portal))
        .merge(api_router)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn middleware_auth_mobile(
    State(state): State<Arc<AuroraDaemonState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(token_esperado) = state.config().mobile_token_activo() {
        if !verificar_cabecera_bearer(request.headers(), token_esperado) {
            return respuesta_no_autorizada();
        }
    }
    next.run(request).await
}

async fn handler_health(State(state): State<Arc<AuroraDaemonState>>) -> Json<HealthResponse> {
    Json(state.health().await)
}

async fn handler_subsistemas(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<ReporteSubsistemas> {
    Json(state.sondear_subsistemas().await)
}

async fn handler_hunt(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<HuntRequest>,
) -> DaemonResult<Json<HuntResponse>> {
    let resp = state.ejecutar_hunt(req).await?;
    Ok(Json(resp))
}

async fn handler_telarana(
    State(state): State<Arc<AuroraDaemonState>>,
) -> DaemonResult<Json<TelaranaResponse>> {
    let resp = state.obtener_telarana().await?;
    Ok(Json(resp))
}

async fn handler_subgrafo(
    State(state): State<Arc<AuroraDaemonState>>,
    Path(prospecto_id): Path<Uuid>,
) -> DaemonResult<Json<SubgrafoProspecto>> {
    let subgrafo = state.obtener_subgrafo(prospecto_id).await?;
    Ok(Json(subgrafo))
}

async fn handler_cortex_generar_plan(
    State(state): State<Arc<AuroraDaemonState>>,
    Path(prospecto_id): Path<Uuid>,
) -> DaemonResult<Json<OrdenPendienteDto>> {
    let dto = state.generar_plan_para_prospecto(prospecto_id).await?;
    Ok(Json(dto))
}

async fn handler_gatekeeper_pendientes(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<GatekeeperPendientesResponse> {
    Json(state.listar_gatekeeper_pendientes())
}

async fn handler_gatekeeper_aprobar(
    State(state): State<Arc<AuroraDaemonState>>,
    Path(orden_id): Path<Uuid>,
    body: Bytes,
) -> DaemonResult<Json<AprobarOrdenResponse>> {
    let req = if body.iter().all(|b| b.is_ascii_whitespace()) {
        None
    } else {
        let parsed: AprobarOrdenRequest = serde_json::from_slice(&body).map_err(|e| {
            DaemonError::SolicitudInvalida(format!("JSON inválido en aprobar_orden: {e}"))
        })?;
        Some(parsed)
    };

    let resp = state.aprobar_orden(orden_id, req).await?;
    Ok(Json(resp))
}

async fn handler_gatekeeper_rechazar(
    State(state): State<Arc<AuroraDaemonState>>,
    Path(orden_id): Path<Uuid>,
    Json(req): Json<RechazarOrdenRequest>,
) -> DaemonResult<Json<OrdenPendienteDto>> {
    let dto = state.rechazar_orden(orden_id, req)?;
    Ok(Json(dto))
}

async fn handler_lfpdppp_opt_out(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<OptOutRequest>,
) -> DaemonResult<Json<OptOutResponse>> {
    let resp = state.ejecutar_opt_out(req).await?;
    Ok(Json(resp))
}

async fn handler_pulse_estado(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<PulseEstadoResponse> {
    Json(state.estado_pulso())
}

async fn handler_pulse_telemetria(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(evento): Json<EventoTelemetria>,
) -> DaemonResult<Json<EstadoCampanaProspecto>> {
    let estado = state.registrar_evento_telemetria(evento).await?;
    Ok(Json(estado))
}

async fn handler_pulse_organico(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<PulseOrganicoResponse> {
    Json(state.obtener_estado_organico())
}

async fn handler_pulse_organico_publicar(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<PublicarOrganicoRequest>,
) -> DaemonResult<Json<PublicarOrganicoResponse>> {
    let resp = state.publicar_reel_organico(req).await?;
    Ok(Json(resp))
}

async fn handler_dpo_consolidar(
    State(state): State<Arc<AuroraDaemonState>>,
) -> DaemonResult<Json<DpoConsolidarResponse>> {
    let resp = state.consolidar_dpo().await?;
    Ok(Json(resp))
}

async fn handler_council_cuotas(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<CouncilCuotasResponse> {
    Json(state.obtener_cuotas_consejo().await)
}

async fn handler_council_delegar(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<DelegarConsejoRequest>,
) -> DaemonResult<Json<ResultadoDelegacion>> {
    let resp = state.delegar_consejo(req).await?;
    Ok(Json(resp))
}

async fn handler_subdirector_agentes(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<SubdirectorAgentesResponse> {
    Json(state.obtener_agentes_subdirector().await)
}

async fn handler_subdirector_misiones_listar(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<SubdirectorMisionesResponse> {
    Json(state.listar_misiones_subdirector())
}

async fn handler_subdirector_misiones_lanzar(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<SolicitudMisionProyecto>,
) -> DaemonResult<Json<ReporteMisionProyecto>> {
    let resp = state.lanzar_mision_subdirector(req).await?;
    Ok(Json(resp))
}

async fn handler_forge_ordenes_listar(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<ForgeOrdenesResponse> {
    Json(state.listar_ordenes_forge().await)
}

async fn handler_forge_ordenes_crear(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<CrearOrdenForjaRequest>,
) -> DaemonResult<Json<RegistroOrdenForjaDto>> {
    let resp = state.crear_y_ejecutar_orden_forge(req).await?;
    Ok(Json(resp))
}

async fn handler_factory_broll_estado(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Json<BrollEstadoResponse> {
    Json(state.obtener_estado_broll().await)
}

async fn handler_factory_broll_registrar(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<RegistrarClipBrollRequest>,
) -> DaemonResult<Json<ClipBroll>> {
    let clip = state.registrar_clip_broll(req).await?;
    Ok(Json(clip))
}

async fn handler_factory_broll_briefs(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<GenerarBriefsHiggsfieldRequest>,
) -> DaemonResult<Json<GenerarBriefsHiggsfieldResponse>> {
    let resp = state.generar_briefs_higgsfield(req).await?;
    Ok(Json(resp))
}

async fn handler_prospectos_db(
    State(state): State<Arc<AuroraDaemonState>>,
    Query(filtro): Query<FiltroProspectosDbQuery>,
) -> DaemonResult<Json<ProspectosDbResponse>> {
    let resp = state.obtener_prospectos_db(filtro).await?;
    Ok(Json(resp))
}

async fn handler_prospectos_catalogar(
    State(state): State<Arc<AuroraDaemonState>>,
    body: Bytes,
) -> DaemonResult<Json<CatalogarProspectosResponse>> {
    let req = if body.iter().all(|b| b.is_ascii_whitespace()) {
        CatalogarProspectosRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| {
            DaemonError::SolicitudInvalida(format!("JSON inválido en prospectos/catalogar: {e}"))
        })?
    };
    let resp = state.catalogar_prospectos(req).await?;
    Ok(Json(resp))
}

async fn handler_content_generar_monetizacion(
    State(state): State<Arc<AuroraDaemonState>>,
    Json(req): Json<GenerarMonetizacionRequest>,
) -> DaemonResult<Json<GenerarMonetizacionResponse>> {
    let resp = state.generar_video_monetizacion(req).await?;
    Ok(Json(resp))
}

async fn handler_eventos_sse(
    State(state): State<Arc<AuroraDaemonState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.suscribir_eventos();
    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(ev) => match serde_json::to_string(&ev) {
            Ok(json) => Some(Ok(Event::default().event(ev.tipo).data(json))),
            Err(_) => None,
        },
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("aurora-keepalive"),
    )
}

async fn handler_eventos_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AuroraDaemonState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| atender_websocket_eventos(socket, state))
}

async fn atender_websocket_eventos(mut socket: WebSocket, state: Arc<AuroraDaemonState>) {
    let mut rx = state.suscribir_eventos();
    loop {
        tokio::select! {
            rec = rx.recv() => {
                match rec {
                    Ok(ev) => {
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if socket.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                if let Some(Ok(Message::Ping(payload))) = incoming {
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                } else if matches!(incoming, Some(Ok(Message::Close(_))) | None) {
                    break;
                }
            }
        }
    }
}
