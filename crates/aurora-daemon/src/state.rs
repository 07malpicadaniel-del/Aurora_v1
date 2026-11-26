use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use aurora_cortex::{
    AuroraToolCall, BobInferenceBackend, BriefCreativo, CalentamientoPendiente, ConfigHttpBob,
    CortexResult, DeterministicMockBob, DiagnosticoComercial, HttpBobCortexClient,
    OrquestadorComercialBob, PlanAtaqueComercial, PlataformaCalentamiento, ResultadoEjecucionTool,
    RolCognitivo, TipoInteraccionPeriferica, ToolDispatcher,
};
use aurora_council::{
    BobLocalFallbackDriver, ClaudeCodeCliDriver, CodexCliDriver, DeterministicMockCouncilDriver,
    EstadoCuotaMiembro, GeminiCliDriver, MiembroConsejo, OpenCodeCliDriver, OrquestadorConsejo,
    PeticionDelegacion, QuotaGovernor, ResultadoDelegacion, TipoTareaConsejo,
};
use aurora_domain::{
    validar_prospecto, EstadoAprobacion, PerfilPsicologico, Prospecto, TipoGatilloVital,
    TipoVinculo, Ubicacion, VerticalComercial,
};
use aurora_factory::{
    validar_ruta_sin_traversal, BibliotecaBroll, BriefHiggsfield, ClipBroll, ConfigFabrica,
    ConfigFfmpeg, ConfigHeadlessBrowser, ConfigPiperCpu, DeterministicMockDocumentEngine,
    DeterministicMockVideoComposer, DeterministicSilentWavEngine, DocumentEngine,
    FabricaMultimodalAurora, FfmpegNvencComposer, HeadlessBrowserDocumentEngine,
    PaqueteManufacturado, PiperCpuTtsEngine, ResultadoAudio, ResultadoVideo, SolicitudManufactura,
    SolicitudRegistroClipBroll, TtsEngine, VideoComposer,
};
use aurora_forge::{
    listar_ramas_forge, validar_rutas_permitidas, ComandoValidacion, ConfigSandboxForja,
    CouncilForgeExecutor, EstadoOrdenIngenieria, OrdenIngenieria, ReporteForja,
    TipoOrdenIngenieria, WorktreeSandbox,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConfigEmbedderMultilingue, DeterministicSimdEmbedder, GatilloVigente,
    IslaVertical, MotorMemoriaAurora, MultilingualCpuEmbedder, SubgrafoProspecto, TextEmbedder,
    VecinoCirculoCercano,
};
use aurora_osint::{
    ConfigCdpFetcher, ConsultaObjetivo, DenueInegiAdapter, DestiladorOsint, DynamicBrowserFetcher,
    EnjambreOsint, EntityResolver, FormatoCrudo, HttpCdpBrowserFetcher, HttpSwarmClient,
    OsintError, OsintResult, OsintSource, PipelineOsint, PoliticaHttp, RegistroCrudo,
    SenalesAbiertasAdapter, WebPublicaMxAdapter,
};
use aurora_pulse::{
    ChannelTransport, ComprobanteDespacho, ConfigCalentamiento, ConfigConsolidacionNocturna,
    ControladorTasaPlataforma, DatasetPreferenciasDpo, DespachadorMulticanal,
    DeterministicMockTransport, DeterministicMockWarmupDriver, EstadoTasaPlataforma,
    EventoTelemetria, GestorCalentamientoPeriferico, MotorConsolidacionNocturna, PlataformaPulso,
    PulseResult, RecolectorTelemetria, RegistroInteraccion, ResumenGanchoTelemetria,
    SmtpEmailTransport, SocialWarmupDriver, TipoEventoTelemetria,
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::adapters::{
    HybridBobBackend, HybridChannelTransport, HybridDocumentEngine, HybridTtsEngine,
    HybridVideoComposer,
};
use crate::config::{
    expandir_tilde_path, resolver_binario_en_sistema, AuroraDaemonConfig, ModoEjecucion,
    ReporteSubsistemas, SubsystemProbe,
};
use crate::error::{DaemonError, DaemonResult};
use crate::scheduler::EstadoSchedulerDpo;

/// Envoltorio object-safe para conectar cualquier [`BobInferenceBackend`] con el daemon sin genéricos en router.
#[derive(Clone)]
pub struct DynBobBackend(pub Arc<dyn BobInferenceBackend>);

#[async_trait]
impl BobInferenceBackend for DynBobBackend {
    async fn inferir_estructurado(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        gramatica_gbnf: &str,
    ) -> CortexResult<String> {
        self.0
            .inferir_estructurado(rol, contexto_usuario, gramatica_gbnf)
            .await
    }
}

/// Envoltorio object-safe sobre [`TtsEngine`].
#[derive(Clone)]
pub struct DynTtsEngine(pub Arc<dyn TtsEngine>);

#[async_trait]
impl TtsEngine for DynTtsEngine {
    async fn sintetizar_voz(
        &self,
        guion: &str,
        perfil: PerfilPsicologico,
        salida_wav: &Path,
    ) -> aurora_factory::FactoryResult<ResultadoAudio> {
        self.0.sintetizar_voz(guion, perfil, salida_wav).await
    }
}

/// Envoltorio object-safe sobre [`DocumentEngine`].
#[derive(Clone)]
pub struct DynDocumentEngine(pub Arc<dyn DocumentEngine>);

#[async_trait]
impl DocumentEngine for DynDocumentEngine {
    async fn renderizar_pdf(
        &self,
        html: &str,
        salida_pdf: &Path,
    ) -> aurora_factory::FactoryResult<u64> {
        self.0.renderizar_pdf(html, salida_pdf).await
    }

    async fn renderizar_frames_png(
        &self,
        html_escenas: &[String],
        directorio_frames: &Path,
    ) -> aurora_factory::FactoryResult<Vec<PathBuf>> {
        self.0
            .renderizar_frames_png(html_escenas, directorio_frames)
            .await
    }

    async fn renderizar_overlays_transparentes(
        &self,
        html_overlays: &[String],
        directorio_overlays: &Path,
    ) -> aurora_factory::FactoryResult<Vec<PathBuf>> {
        self.0
            .renderizar_overlays_transparentes(html_overlays, directorio_overlays)
            .await
    }
}

/// Envoltorio object-safe sobre [`VideoComposer`].
#[derive(Clone)]
pub struct DynVideoComposer(pub Arc<dyn VideoComposer>);

#[async_trait]
impl VideoComposer for DynVideoComposer {
    async fn ensamblar_reel_vertical(
        &self,
        frames: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> aurora_factory::FactoryResult<ResultadoVideo> {
        self.0
            .ensamblar_reel_vertical(frames, audio_wav, srt, salida_mp4)
            .await
    }

    async fn ensamblar_reel_con_broll(
        &self,
        clips_broll: &[PathBuf],
        overlays_png: &[PathBuf],
        audio_wav: &Path,
        srt: &Path,
        salida_mp4: &Path,
    ) -> aurora_factory::FactoryResult<ResultadoVideo> {
        self.0
            .ensamblar_reel_con_broll(clips_broll, overlays_png, audio_wav, srt, salida_mp4)
            .await
    }
}

/// Envoltorio object-safe sobre [`SocialWarmupDriver`].
#[derive(Clone)]
pub struct DynWarmupDriver(pub Arc<dyn SocialWarmupDriver>);

#[async_trait]
impl SocialWarmupDriver for DynWarmupDriver {
    async fn ejecutar_interaccion(
        &self,
        accion: &CalentamientoPendiente,
    ) -> PulseResult<RegistroInteraccion> {
        self.0.ejecutar_interaccion(accion).await
    }
}

/// Envoltorio object-safe sobre [`ChannelTransport`].
#[derive(Clone)]
pub struct DynChannelTransport(pub Arc<dyn ChannelTransport>);

#[async_trait]
impl ChannelTransport for DynChannelTransport {
    async fn enviar_paquete(
        &self,
        envio: &aurora_pulse::OrdenDespacho,
    ) -> PulseResult<ComprobanteDespacho> {
        self.0.enviar_paquete(envio).await
    }
}

/// Fuente OSINT en memoria para inyección de registros públicos validados desde request o tests.
struct InlineOsintSource {
    nombre: String,
    registros: Vec<RegistroCrudo>,
}

#[async_trait]
impl OsintSource for InlineOsintSource {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        if consulta.limite_resultados > 0 && self.registros.len() > consulta.limite_resultados {
            Ok(self.registros[..consulta.limite_resultados].to_vec())
        } else {
            Ok(self.registros.clone())
        }
    }
}

/// Alias compatible hacia [`AuroraDaemonConfig`].
pub type DaemonConfig = AuroraDaemonConfig;

/// Respuesta de salud y telemetría de hardware para `GET /api/v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Estado general del demonio (`"ok"`).
    pub status: String,
    /// Versión semántica de `aurora-daemon`.
    pub version: String,
    /// Dirección de enlace HTTP del demonio.
    pub bind_addr: String,
    /// Indica si corre con adaptadores deterministas offline o motores locales reales.
    pub modo_offline: bool,
    /// Política activa de ejecución (`real_con_fallback`, `estricto_real` o `simulado_offline`).
    #[serde(default)]
    pub modo_ejecucion: ModoEjecucion,
    /// URL configurada de BOB (`127.0.0.1:8085`).
    pub bob_url: String,
    /// Disponibilidad actual de BOB (`true` si responde `/health` o en modo offline mock).
    pub bob_online: bool,
    /// Ranura única VRAM configurada para BOB.
    pub bob_slot_id: i32,
    /// Descripción del presupuesto GPU.
    pub hardware_vram_modelo: String,
    /// Descripción del presupuesto CPU/RAM.
    pub hardware_ram_sistema: String,
    /// Diagnóstico en vivo de cada subsistema nativo y puente HTTP/SMTP.
    #[serde(default)]
    pub subsistemas: ReporteSubsistemas,
    /// Total de prospectos activos en el hipergrafo 4D.
    pub nodos_4d: usize,
    /// Total de relaciones topológicas del Círculo Cercano.
    pub aristas_4d: usize,
    /// Órdenes en espera de revisión humana Gatekeeper.
    pub ordenes_pendientes: usize,
    /// Acciones de calentamiento periférico en espera de Gatekeeper.
    pub calentamientos_pendientes: usize,
    /// Eventos acumulados en el recolector de telemetría.
    pub telemetria_eventos: usize,
    /// Segundos activos desde el arranque del demonio.
    pub uptime_s: u64,
}

/// Solicitud para lanzar una cacería OSINT completa (`OSINT -> Memoria 4D -> Córtex BOB`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntRequest {
    /// Vertical y subvertical objetivo en México.
    pub vertical: VerticalComercial,
    /// Ciudad objetivo (ej. "Monterrey", "CDMX").
    pub ciudad: String,
    /// Estado objetivo (ej. "Nuevo León", "CDMX").
    pub estado: String,
    /// Zona comercial o industrial opcional.
    pub zona: Option<String>,
    /// Palabras clave de intención o giro.
    #[serde(default)]
    pub palabras_clave: Vec<String>,
    /// Máximo de registros a recolectar.
    pub limite_resultados: Option<usize>,
    /// Si se permite respaldo CDP cuando esté disponible.
    pub requiere_js: Option<bool>,
    /// URL pública HTML o JSON opcional a consultar en vivo bajo política HTTP.
    pub fuente_url: Option<String>,
    /// Formato de `fuente_url` (`Html` por defecto si se omite).
    pub fuente_formato: Option<FormatoCrudo>,
    /// Registros crudos públicos pre-recolectados (útil en pruebas e ingesta asistida).
    pub registros_semilla: Option<Vec<RegistroCrudo>>,
}

/// Resumen emitido tras completar una cacería OSINT y perfilar en Córtex BOB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntResponse {
    /// Identificador único de campaña asignado a este lote.
    pub campana_id: Uuid,
    /// Isla vectorial vertical seleccionada.
    pub isla: IslaVertical,
    /// Total de registros crudos recolectados antes de deduplicación.
    pub registros_crudos_totales: usize,
    /// Cantidad de prospectos validados e ingestados en `aurora-memory`.
    pub prospectos_ingestados: usize,
    /// Cantidad de hiperaristas de Círculo Cercano creadas.
    pub relaciones_creadas: usize,
    /// Prospectos rechazados automáticamente por bandera `opt_out = true` LFPDPPP.
    pub excluidos_opt_out: Vec<Uuid>,
    /// Planes de ataque comercial encolados en `PendienteGatekeeper`.
    pub planes_encolados: Vec<OrdenPendienteDto>,
}

/// Proyección serializable de un gatillo con su peso vigente 4D.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatilloTelaranaDto {
    /// Categoría de evento vital/comercial.
    pub tipo: TipoGatilloVital,
    /// Descripción del evento capturado.
    pub descripcion: String,
    /// Fecha UTC del evento original.
    pub ocurrido_en: DateTime<Utc>,
    /// Peso base inicial en `[0.0, 1.0]`.
    pub peso_urgencia: f32,
    /// Peso tras decaimiento temporal exponencial $w(t)$.
    pub peso_vigente_4d: f32,
}

/// Nodo de prospecto enriquecido para visualización de la Telaraña 4D en `axis_v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodoTelaranaDto {
    /// UUID canónico del prospecto.
    pub prospecto_id: Uuid,
    /// ID `u64` determinista en `vector_db`.
    pub node_id: u64,
    /// Nombre normalizado libre de ruido HTML.
    pub nombre_normalizado: String,
    /// Isla vertical asignada.
    pub isla: IslaVertical,
    /// Perfil psicológico inferido.
    pub perfil_psicologico: PerfilPsicologico,
    /// Ciudad.
    pub ciudad: String,
    /// Estado.
    pub estado: String,
    /// Zona comercial opcional.
    pub zona: Option<String>,
    /// Ingreso anual estimado en MXN.
    pub ingreso_anual_mxn: u64,
    /// Patrimonio estimado opcional en MXN.
    pub patrimonio_estimado_mxn: Option<u64>,
    /// Relevancia temporal vigente 4D acumulada.
    pub relevancia_temporal_4d: f32,
    /// Fuente pública origen (LFPDPPP).
    pub fuente_origen: String,
    /// Evidencia verificable HTTP/HTTPS (LFPDPPP).
    pub url_evidencia: String,
    /// Gatillos ordenados por vigencia 4D.
    pub gatillos: Vec<GatilloTelaranaDto>,
}

/// Arista de relación topológica entre dos prospectos del Círculo Cercano.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AristaTelaranaDto {
    /// Prospecto origen.
    pub origen_id: Uuid,
    /// Prospecto destino.
    pub destino_id: Uuid,
    /// Naturaleza del vínculo.
    pub tipo_vinculo: TipoVinculo,
    /// Fuerza en `[0.0, 1.0]`.
    pub fuerza: f32,
    /// Contexto compartido detectado en OSINT.
    pub contexto_compartido: String,
}

/// Conteo de nodos por isla vectorial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumenIslaDto {
    /// Isla vertical.
    pub isla: IslaVertical,
    /// Número de prospectos en la isla.
    pub cantidad: usize,
}

/// Respuesta integral de la Telaraña Topológica 4D (`GET /api/v1/memory/telarana`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelaranaResponse {
    /// Constante de decaimiento $\lambda$ por día vigente en `GrafoTemporal4D`.
    pub lambda_por_dia: f32,
    /// Número total de nodos activos.
    pub total_nodos: usize,
    /// Número total de aristas únicas activas.
    pub total_aristas: usize,
    /// Distribución por isla vertical.
    pub resumen_islas: Vec<ResumenIslaDto>,
    /// Lista de nodos ordenados por relevancia temporal descendente.
    pub nodos: Vec<NodoTelaranaDto>,
    /// Lista de aristas del Círculo Cercano.
    pub aristas: Vec<AristaTelaranaDto>,
}

/// Vista pública de una orden Gatekeeper con toda la información creativa y de aprobación.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrdenPendienteDto {
    /// Identificador único de la orden de producción.
    pub orden_id: Uuid,
    /// Campaña origen.
    pub campana_id: Uuid,
    /// Prospecto objetivo.
    pub prospecto_id: Uuid,
    /// Nombre normalizado del prospecto.
    pub prospecto_nombre: String,
    /// Perfil psicológico del prospecto.
    pub perfil_psicologico: PerfilPsicologico,
    /// Ubicación resumida.
    pub ubicacion: String,
    /// Isla vectorial asociada.
    pub isla: IslaVertical,
    /// Estado actual en el flujo Gatekeeper.
    pub estado: EstadoAprobacion,
    /// Motivo de rechazo (cuando aplique).
    pub motivo_rechazo: Option<String>,
    /// Diagnóstico emitido por BOB `Perfilador`.
    pub diagnostico: DiagnosticoComercial,
    /// Brief creativo completo (gancho 3s, guion TTS, escenas Reel, secciones PDF).
    pub brief_creativo: BriefCreativo,
    /// Acción periférica asociada, si fue generada.
    pub calentamiento: Option<CalentamientoPendiente>,
    /// Paquete manufacturado en NVMe tras aprobar en Gatekeeper.
    pub paquete_manufacturado: Option<PaqueteManufacturado>,
    /// Comprobante de despacho multicanal, cuando haya sido enviado.
    pub comprobante_despacho: Option<ComprobanteDespacho>,
    /// Última actualización UTC.
    pub actualizado_en: DateTime<Utc>,
}

/// Respuesta de `GET /api/v1/gatekeeper/pendientes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatekeeperPendientesResponse {
    /// Número de órdenes actualmente en `PendienteGatekeeper`.
    pub total_pendientes: usize,
    /// Número total de órdenes registradas en historial de cola.
    pub total_en_cola: usize,
    /// Órdenes en `EstadoAprobacion::PendienteGatekeeper`.
    pub ordenes: Vec<OrdenPendienteDto>,
    /// Órdenes ya procesadas (aprobadas, manufacturadas, despachadas o rechazadas) para trazabilidad.
    pub historial_reciente: Vec<OrdenPendienteDto>,
}

/// Opciones opcionales al aprobar una orden en Gatekeeper.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AprobarOrdenRequest {
    /// Si es `true`, tras aprobar y calentar ejecuta despacho inmediato autorizado por operador.
    pub despachar_inmediatamente: Option<bool>,
    /// Correo destino explícito cuando el prospecto aún no lo tiene en alias OSINT.
    #[serde(default)]
    pub correo_destino: Option<String>,
}

/// Respuesta devuelta por `POST /api/v1/gatekeeper/aprobar/:orden_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AprobarOrdenResponse {
    /// Identificador de la orden procesada.
    pub orden_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Campaña asociada.
    pub campana_id: Uuid,
    /// Estado final alcanzado (`Aprobado` o `Despachado`).
    pub estado: EstadoAprobacion,
    /// Artefactos PDF, WAV, MP4 y SRT generados y verificados en NVMe.
    pub paquete_manufacturado: PaqueteManufacturado,
    /// Calentamiento periférico ejecutado (si estaba adjunto a la orden).
    pub calentamiento_ejecutado: Option<RegistroInteraccion>,
    /// Comprobante de despacho si se solicitó despacho inmediato.
    pub comprobante_despacho: Option<ComprobanteDespacho>,
}

/// Solicitud de rechazo formal en Gatekeeper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RechazarOrdenRequest {
    /// Motivo obligatorio auditado del rechazo.
    pub motivo: String,
}

/// Solicitud de baja y purga física inmediata LFPDPPP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptOutRequest {
    /// UUID del titular que ejerció su derecho ARCO / `opt_out`.
    pub prospecto_id: Uuid,
    /// Campaña opcional desde la que se originó la baja.
    pub campana_id: Option<Uuid>,
    /// Motivo o referencia documental de la baja.
    pub motivo: String,
}

/// Confirmación auditable de purga física LFPDPPP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptOutResponse {
    /// Prospecto purgado.
    pub prospecto_id: Uuid,
    /// Confirma compactación física de WAL/snapshot e índice HNSW.
    pub purgado_de_wal: bool,
    /// Confirma presencia permanente en blacklist local `opt_out`.
    pub en_blacklist: bool,
    /// Número de órdenes pendientes canceladas automáticamente para ese prospecto.
    pub ordenes_canceladas: usize,
    /// Mensaje resumen de cumplimiento.
    pub mensaje: String,
}

/// Respuesta de `GET /api/v1/pulse/estado`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseEstadoResponse {
    /// Estado actual de cuotas horarias/diarias y circuit breakers de todas las plataformas.
    pub plataformas: Vec<EstadoTasaPlataforma>,
    /// Campañas registradas en el bus de telemetría.
    pub campanas_activas: Vec<Uuid>,
    /// Estadísticas consolidadas por gancho creativo.
    pub estadisticas_ganchos: Vec<ResumenGanchoTelemetria>,
    /// Eventos acumulados en telemetría.
    pub telemetria_eventos_totales: usize,
    /// Interacciones de calentamiento recientes ejecutadas.
    pub calentamientos_recientes: Vec<RegistroInteraccion>,
    /// Despachos multicanal completados recientes.
    pub despachos_recientes: Vec<ComprobanteDespacho>,
    /// Estado del scheduler circadiano autónomo DPO en segundo plano.
    #[serde(default)]
    pub scheduler_dpo: EstadoSchedulerDpo,
}

/// Respuesta de `POST /api/v1/dpo/consolidar`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DpoConsolidarResponse {
    /// Número de campañas auditadas en la corrida (0 cuando no hay telemetría nueva).
    #[serde(default)]
    pub campanas_auditadas: usize,
    /// Indica si la consolidación terminó en modo idempotente *no-op* sin eventos nuevos.
    #[serde(default)]
    pub modo_noop: bool,
    /// Mensaje legible para el operador sobre el resultado de la consolidación.
    #[serde(default)]
    pub mensaje: String,
    /// Dataset JSONL generado y conteos de recalibración 4D aplicados.
    pub dataset: DatasetPreferenciasDpo,
}

/// Respuesta de `GET /api/v1/council/cuotas`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilCuotasResponse {
    /// Orden de cascada actualmente disponible (siempre termina en `BobLocal`).
    pub cascada_vigente: Vec<MiembroConsejo>,
    /// Estado detallado de ventanas 5h, topes diarios y cooldowns por miembro.
    pub miembros: Vec<EstadoCuotaMiembro>,
}

/// Solicitud para `POST /api/v1/council/delegar`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegarConsejoRequest {
    /// Especialidad de tarea del consejo multi-agente.
    pub tarea: TipoTareaConsejo,
    /// Contexto o consigna a resolver.
    pub contexto: String,
    /// UUID de un prospecto en memoria 4D para activar `PiiFirewall` automáticamente.
    #[serde(default)]
    pub prospecto_id: Option<Uuid>,
    /// Prospecto opcional en línea (alternativo a `prospecto_id`) para sanitización `PiiFirewall`.
    #[serde(default)]
    pub prospecto: Option<Prospecto>,
    /// Si es `true`, resuelve directo en `BobLocal` sin consumir cuotas de nube.
    #[serde(default)]
    pub forzar_local: Option<bool>,
    /// Timeout explícito opcional en milisegundos.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Registro persistente y visible de una orden de ingeniería ejecutada por `aurora-forge`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistroOrdenForjaDto {
    /// Definición y estado vigente de la orden de ingeniería.
    pub orden: OrdenIngenieria,
    /// Reporte verificable con rama `forge/<id>` y commit cuando superó las compuertas.
    pub reporte: Option<ReporteForja>,
    /// Motivo de aborto cuando el estado es `Fallida`.
    pub error: Option<String>,
    /// Última actualización UTC.
    pub actualizado_en: DateTime<Utc>,
}

/// Respuesta de `GET /api/v1/forge/ordenes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeOrdenesResponse {
    /// Indica si `aurora-forge` está habilitado en configuración.
    pub habilitado: bool,
    /// Total de órdenes registradas en el historial.
    pub total_ordenes: usize,
    /// Total de órdenes listas en Carril Ámbar esperando revisión humana.
    pub pendientes_revision_humana: usize,
    /// Ramas `forge/*` existentes actualmente en el repositorio Git configurado.
    pub ramas_forge: Vec<String>,
    /// Historial de órdenes de ingeniería y reportes.
    pub ordenes: Vec<RegistroOrdenForjaDto>,
}

/// Solicitud para `POST /api/v1/forge/ordenes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrearOrdenForjaRequest {
    /// Categoría de ingeniería autónoma permitida.
    pub tipo: TipoOrdenIngenieria,
    /// Descripción técnica precisa del cambio solicitado.
    pub descripcion_tecnica: String,
    /// Lista blanca explícita de rutas relativas autorizadas (prohibido incluir rutas protegidas).
    pub rutas_permitidas: Vec<String>,
    /// Compuerta determinista de validación (por defecto `CargoCheckClippyTest`).
    #[serde(default)]
    pub comando_validacion: Option<ComandoValidacion>,
    /// Repositorio Git opcional a usar para el worktree (si se omite, usa `forge.repo_root`).
    #[serde(default)]
    pub repo_root: Option<PathBuf>,
}

/// Respuesta de `GET /api/v1/factory/broll`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrollEstadoResponse {
    /// Directorio NVMe configurado para la biblioteca B-Roll.
    pub broll_dir: PathBuf,
    /// Número total de clips maestros registrados.
    pub total_clips: usize,
    /// Conteo de clips disponibles agrupado por vertical comercial.
    pub clips_por_vertical: BTreeMap<String, usize>,
    /// Inventario completo de clips B-Roll y sus usos acumulados.
    pub clips: Vec<ClipBroll>,
    /// Verticales comerciales con menos de 3 clips maestros (brechas de cobertura).
    pub brechas_cobertura: Vec<VerticalComercial>,
    /// Cola de briefs cinematográficos generados para Higgsfield.
    pub briefs_higgsfield: Vec<BriefHiggsfield>,
}

/// Solicitud para `POST /api/v1/factory/broll/registrar`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrarClipBrollRequest {
    /// Ruta local del clip `.mp4`/`.mov` descargado de Higgsfield (sin *path traversal*).
    pub ruta_origen: PathBuf,
    /// Vertical comercial correspondiente al clip.
    pub vertical: VerticalComercial,
    /// Perfil psicológico objetivo opcional.
    #[serde(default)]
    pub perfil_objetivo: Option<PerfilPsicologico>,
    /// Etiquetas semánticas opcionales.
    #[serde(default)]
    pub etiquetas: Vec<String>,
    /// Duración en milisegundos (opcional, por defecto 5000 ms).
    #[serde(default)]
    pub duracion_ms: Option<u64>,
    /// Ancho del clip en píxeles (opcional, por defecto 1080).
    #[serde(default)]
    pub ancho: Option<u32>,
    /// Alto del clip en píxeles (opcional, por defecto 1920).
    #[serde(default)]
    pub alto: Option<u32>,
    /// Movimiento de cámara cinematográfico (opcional, por defecto `dolly_in_slow`).
    #[serde(default)]
    pub movimiento_camara: Option<String>,
    /// Brief Higgsfield opcional a marcar como `Importado` tras registrar el clip.
    #[serde(default)]
    pub brief_id: Option<Uuid>,
}

/// Solicitud para `POST /api/v1/factory/broll/briefs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerarBriefsHiggsfieldRequest {
    /// Vertical comercial objetivo.
    pub vertical: VerticalComercial,
    /// Perfil psicológico objetivo.
    pub perfil_objetivo: PerfilPsicologico,
    /// Cantidad de briefs cinematográficos a generar (`1..=10`, por defecto `3`).
    #[serde(default)]
    pub cantidad: Option<usize>,
}

/// Respuesta de `POST /api/v1/factory/broll/briefs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerarBriefsHiggsfieldResponse {
    /// Nuevos briefs creados y persistidos en NVMe.
    pub briefs_generados: Vec<BriefHiggsfield>,
    /// Total acumulado en la cola de Higgsfield.
    pub total_en_cola: usize,
}

/// Evento emitido en tiempo real hacia `/api/v1/eventos/stream`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventoDaemon {
    /// Tipo estable del evento emitido.
    pub tipo: String,
    /// Marca temporal UTC de emisión.
    pub timestamp: DateTime<Utc>,
    /// Carga útil en JSON para consumo de clientes React / CLI.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistroOrdenInterno {
    plan: PlanAtaqueComercial,
    prospecto_nombre: String,
    perfil_psicologico: PerfilPsicologico,
    ubicacion: String,
    isla: IslaVertical,
    motivo_rechazo: Option<String>,
    calentamiento: Option<CalentamientoPendiente>,
    paquete_manufacturado: Option<PaqueteManufacturado>,
    comprobante_despacho: Option<ComprobanteDespacho>,
    actualizado_en: DateTime<Utc>,
}

impl RegistroOrdenInterno {
    fn a_dto(&self) -> OrdenPendienteDto {
        OrdenPendienteDto {
            orden_id: self.plan.orden_produccion.orden_id,
            campana_id: self.plan.campana_id,
            prospecto_id: self.plan.prospecto_id,
            prospecto_nombre: self.prospecto_nombre.clone(),
            perfil_psicologico: self.perfil_psicologico,
            ubicacion: self.ubicacion.clone(),
            isla: self.isla,
            estado: self.plan.orden_produccion.estado(),
            motivo_rechazo: self.motivo_rechazo.clone(),
            diagnostico: self.plan.diagnostico.clone(),
            brief_creativo: self.plan.brief_creativo.clone(),
            calentamiento: self.calentamiento.clone(),
            paquete_manufacturado: self.paquete_manufacturado.clone(),
            comprobante_despacho: self.comprobante_despacho.clone(),
            actualizado_en: self.actualizado_en,
        }
    }
}

/// Estado central compartido y thread-safe de `aurora-daemon`.
pub struct AuroraDaemonState {
    config: AuroraDaemonConfig,
    started_at: DateTime<Utc>,
    http_probe: Client,
    cdp_fetcher: Option<Arc<HttpCdpBrowserFetcher>>,
    memoria: RwLock<MotorMemoriaAurora>,
    osint_fuentes: RwLock<Vec<Arc<dyn OsintSource>>>,
    bob_slot_lock: Arc<tokio::sync::Mutex<()>>,
    orquestador: OrquestadorComercialBob<DynBobBackend>,
    broll: Arc<tokio::sync::Mutex<BibliotecaBroll>>,
    fabrica: FabricaMultimodalAurora<DynTtsEngine, DynDocumentEngine, DynVideoComposer>,
    rate_limiter: Arc<ControladorTasaPlataforma>,
    warmup_gestor: GestorCalentamientoPeriferico<DynWarmupDriver>,
    despachador: DespachadorMulticanal<DynChannelTransport>,
    telemetria: Arc<RecolectorTelemetria>,
    motor_dpo: MotorConsolidacionNocturna<DynBobBackend>,
    consejo: Arc<OrquestadorConsejo>,
    scheduler_estado: Mutex<EstadoSchedulerDpo>,
    cola_gatekeeper: Mutex<BTreeMap<Uuid, RegistroOrdenInterno>>,
    forge_ordenes: Mutex<Vec<RegistroOrdenForjaDto>>,
    calentamientos_recientes: Mutex<Vec<RegistroInteraccion>>,
    despachos_recientes: Mutex<Vec<ComprobanteDespacho>>,
    ruta_cola_persistida: Option<PathBuf>,
    ruta_forge_persistida: Option<PathBuf>,
    eventos_tx: broadcast::Sender<EventoDaemon>,
}

impl AuroraDaemonState {
    /// Construye el estado de producción local sobre `data_dir` XDG/NVMe, BOB (`127.0.0.1:8085`),
    /// `PiperCpuTtsEngine`, `HeadlessBrowserDocumentEngine`, `FfmpegNvencComposer`, CDP y SMTP rustls.
    pub fn inicializar_local(config: AuroraDaemonConfig) -> DaemonResult<Arc<Self>> {
        let data_dir = config.data_dir_resuelto();
        let wal_dir = config.wal_dir_efectivo();
        let factory_dir = config.artefactos_dir_efectivo();
        let dpo_dir = config.dpo_dir_efectivo();
        let queue_path = config.queue_path_efectivo();

        for dir in [&data_dir, &factory_dir, &dpo_dir] {
            fs::create_dir_all(dir).map_err(|e| {
                DaemonError::Interno(format!(
                    "no fue posible crear directorio de datos '{}': {e}",
                    dir.display()
                ))
            })?;
        }

        let modo = config.modo_efectivo();
        let embedder: Arc<dyn TextEmbedder> = match modo {
            ModoEjecucion::SimuladoOffline => Arc::new(DeterministicSimdEmbedder::default()),
            ModoEjecucion::EstrictoReal => Arc::new(MultilingualCpuEmbedder::new(
                ConfigEmbedderMultilingue::default(),
            )?),
            ModoEjecucion::RealConFallback => {
                if let Ok(cpu_embedder) =
                    MultilingualCpuEmbedder::new(ConfigEmbedderMultilingue::default())
                {
                    Arc::new(cpu_embedder)
                } else {
                    warn!("No se pudo inicializar MultilingualCpuEmbedder; usando DeterministicSimdEmbedder como respaldo");
                    Arc::new(DeterministicSimdEmbedder::default())
                }
            }
        };

        let memoria = MotorMemoriaAurora::abrir(&wal_dir, embedder, ConfigDecaimiento4D::default())?;

        let bob_cfg = ConfigHttpBob {
            url_base: config.bob_url.clone(),
            slot_id: config.bob_slot_id,
            ..ConfigHttpBob::default()
        };
        let bob_backend: Arc<dyn BobInferenceBackend> = Arc::new(HybridBobBackend {
            real: HttpBobCortexClient::new(bob_cfg)?,
            fallback: DeterministicMockBob,
            modo,
        });

        let piper_bin = config.piper_bin_efectivo();
        let piper_model = config.piper_model_resuelto();
        let piper_precheck =
            resolver_binario_en_sistema(&piper_bin, &[]).is_some() && piper_model.is_file();
        let piper_cfg = ConfigPiperCpu {
            binario_piper: piper_bin,
            modelo_onnx: piper_model,
            speaker_id: config.piper_speaker_id,
            ..ConfigPiperCpu::default()
        };
        let tts_engine: Arc<dyn TtsEngine> = Arc::new(HybridTtsEngine {
            real: PiperCpuTtsEngine::new(piper_cfg),
            fallback: DeterministicSilentWavEngine,
            modo,
            precheck_listo: piper_precheck,
        });

        let chromium_bin = config.chromium_bin_efectivo();
        let chromium_disponible = resolver_binario_en_sistema(&chromium_bin, &[]).is_some();
        let chromium_cfg = ConfigHeadlessBrowser {
            binario_navegador: chromium_bin,
            ..ConfigHeadlessBrowser::default()
        };
        let doc_engine: Arc<dyn DocumentEngine> = Arc::new(HybridDocumentEngine {
            real: HeadlessBrowserDocumentEngine::new(chromium_cfg),
            fallback: DeterministicMockDocumentEngine,
            modo,
            binario_disponible: chromium_disponible,
        });

        let ffmpeg_bin = config.ffmpeg_bin_efectivo();
        let ffmpeg_disponible = resolver_binario_en_sistema(&ffmpeg_bin, &[]).is_some();
        let ffmpeg_cfg = ConfigFfmpeg {
            binario_ffmpeg: ffmpeg_bin,
            usar_nvenc: config.usar_nvenc,
            ..ConfigFfmpeg::default()
        };
        let video_composer: Arc<dyn VideoComposer> = Arc::new(HybridVideoComposer {
            real: FfmpegNvencComposer::new(ffmpeg_cfg),
            fallback: DeterministicMockVideoComposer,
            modo,
            binario_disponible: ffmpeg_disponible,
        });

        let smtp_client = if config.smtp.habilitado && !matches!(modo, ModoEjecucion::SimuladoOffline)
        {
            Some(Arc::new(SmtpEmailTransport::new(config.smtp.clone())?))
        } else {
            None
        };

        let warmup_driver: Arc<dyn SocialWarmupDriver> = Arc::new(DeterministicMockWarmupDriver::new());
        let transport: Arc<dyn ChannelTransport> = Arc::new(HybridChannelTransport {
            smtp: smtp_client,
            smtp_config: config.smtp.clone(),
            fallback: DeterministicMockTransport::new(),
            modo,
        });

        let cdp_fetcher = if matches!(modo, ModoEjecucion::SimuladoOffline) {
            None
        } else {
            let cfg_cdp = ConfigCdpFetcher {
                cdp_http_url: config.cdp_url.clone(),
                ..ConfigCdpFetcher::default()
            };
            Some(Arc::new(HttpCdpBrowserFetcher::new(cfg_cdp)?))
        };

        Self::con_componentes(
            config,
            memoria,
            Vec::new(),
            bob_backend,
            tts_engine,
            doc_engine,
            video_composer,
            warmup_driver,
            transport,
            cdp_fetcher,
            factory_dir,
            dpo_dir,
            Some(queue_path),
        )
    }

    /// Construye un estado determinista completamente offline ideal para pruebas de integración HTTP.
    pub fn para_pruebas_offline(base_dir: &Path) -> DaemonResult<Arc<Self>> {
        let config = AuroraDaemonConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: base_dir.to_path_buf(),
            modo_offline: true,
            modo_ejecucion: ModoEjecucion::SimuladoOffline,
            osint_web_publica_habilitada: false,
            dpo_scheduler_habilitado: false,
            ..AuroraDaemonConfig::default()
        };

        let wal_dir = config.wal_dir_efectivo();
        let factory_dir = config.artefactos_dir_efectivo();
        let dpo_dir = config.dpo_dir_efectivo();
        let queue_path = config.queue_path_efectivo();

        fs::create_dir_all(base_dir).map_err(|e| {
            DaemonError::Interno(format!(
                "no fue posible crear directorio temporal '{}': {e}",
                base_dir.display()
            ))
        })?;

        let embedder: Arc<dyn TextEmbedder> = Arc::new(DeterministicSimdEmbedder::default());
        let memoria = MotorMemoriaAurora::abrir(&wal_dir, embedder, ConfigDecaimiento4D::default())?;

        Self::con_componentes(
            config,
            memoria,
            Vec::new(),
            Arc::new(DeterministicMockBob),
            Arc::new(DeterministicSilentWavEngine),
            Arc::new(DeterministicMockDocumentEngine),
            Arc::new(DeterministicMockVideoComposer),
            Arc::new(DeterministicMockWarmupDriver::new()),
            Arc::new(DeterministicMockTransport::new()),
            None,
            factory_dir,
            dpo_dir,
            Some(queue_path),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn con_componentes(
        config: AuroraDaemonConfig,
        memoria: MotorMemoriaAurora,
        fuentes_iniciales: Vec<Arc<dyn OsintSource>>,
        bob_backend: Arc<dyn BobInferenceBackend>,
        tts_engine: Arc<dyn TtsEngine>,
        doc_engine: Arc<dyn DocumentEngine>,
        video_composer: Arc<dyn VideoComposer>,
        warmup_driver: Arc<dyn SocialWarmupDriver>,
        transport: Arc<dyn ChannelTransport>,
        cdp_fetcher: Option<Arc<HttpCdpBrowserFetcher>>,
        factory_dir: PathBuf,
        dpo_dir: PathBuf,
        ruta_cola_persistida: Option<PathBuf>,
    ) -> DaemonResult<Arc<Self>> {
        let rate_limiter = Arc::new(ControladorTasaPlataforma::default());
        let bob_slot_lock = Arc::new(tokio::sync::Mutex::new(()));
        let orquestador = OrquestadorComercialBob::new(DynBobBackend(bob_backend.clone()));
        let motor_dpo = MotorConsolidacionNocturna::new(
            OrquestadorComercialBob::new(DynBobBackend(bob_backend.clone())),
            ConfigConsolidacionNocturna {
                directorio_salida_dpo: dpo_dir,
                ..ConfigConsolidacionNocturna::default()
            },
        );

        let governor = Arc::new(QuotaGovernor::abrir(
            config.construir_config_cuotas_consejo(),
        )?);
        let mut consejo_builder = OrquestadorConsejo::new(governor)
            .con_timeout_por_defecto_ms(config.council.timeout_ms);
        if matches!(config.modo_efectivo(), ModoEjecucion::SimuladoOffline) {
            consejo_builder = consejo_builder
                .con_driver(Arc::new(DeterministicMockCouncilDriver::exito(
                    MiembroConsejo::ClaudePro,
                    "[Consejo Offline] Propuesta generada para: {prompt}",
                )))
                .con_driver(Arc::new(DeterministicMockCouncilDriver::no_disponible(
                    MiembroConsejo::CodexPlus,
                )))
                .con_driver(Arc::new(DeterministicMockCouncilDriver::no_disponible(
                    MiembroConsejo::GeminiEstudiante,
                )))
                .con_driver(Arc::new(DeterministicMockCouncilDriver::no_disponible(
                    MiembroConsejo::OpenCodeLocal,
                )));
        } else {
            consejo_builder = consejo_builder
                .con_driver(Arc::new(ClaudeCodeCliDriver::new(
                    expandir_tilde_path(&config.council.claude_bin),
                    config.council.timeout_ms,
                )))
                .con_driver(Arc::new(CodexCliDriver::new(
                    expandir_tilde_path(&config.council.codex_bin),
                    config.council.timeout_ms,
                )))
                .con_driver(Arc::new(GeminiCliDriver::new(
                    expandir_tilde_path(&config.council.gemini_bin),
                    config.council.timeout_ms,
                )))
                .con_driver(Arc::new(OpenCodeCliDriver::new(
                    expandir_tilde_path(&config.council.opencode_bin),
                    config.council.timeout_ms,
                )));
        }
        let consejo = Arc::new(
            consejo_builder.con_driver(Arc::new(
                BobLocalFallbackDriver::new(DynBobBackend(bob_backend))
                    .con_slot_lock(Arc::clone(&bob_slot_lock)),
            )),
        );

        let ruta_forge_persistida = Some(config.forge_ordenes_path_efectivo());
        let forge_inicial = if let Some(ref ruta) = ruta_forge_persistida {
            cargar_ordenes_forge(ruta)?
        } else {
            Vec::new()
        };
        let broll = Arc::new(tokio::sync::Mutex::new(BibliotecaBroll::abrir(
            config.broll_dir_efectivo(),
        )?));
        let fabrica = FabricaMultimodalAurora::new(
            ConfigFabrica {
                directorio_base: factory_dir,
            },
            DynTtsEngine(tts_engine),
            DynDocumentEngine(doc_engine),
            DynVideoComposer(video_composer),
        )
        .con_biblioteca_broll(Arc::clone(&broll));
        let warmup_gestor = GestorCalentamientoPeriferico::new(
            DynWarmupDriver(warmup_driver),
            Arc::clone(&rate_limiter),
            ConfigCalentamiento::default(),
        );
        let despachador = DespachadorMulticanal::new(
            DynChannelTransport(transport),
            Arc::clone(&rate_limiter),
        );

        let http_probe = Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .map_err(|e| DaemonError::Interno(format!("error inicializando cliente HTTP: {e}")))?;

        let cola_inicial = if let Some(ref ruta) = ruta_cola_persistida {
            cargar_cola_gatekeeper(ruta)?
        } else {
            BTreeMap::new()
        };

        let (eventos_tx, _) = broadcast::channel(256);
        let scheduler_inicial = EstadoSchedulerDpo {
            habilitado: config.dpo_scheduler_habilitado,
            hora_utc_preferida: config.dpo_hora_utc,
            intervalo_minutos: config.dpo_intervalo_minutos,
            proxima_ejecucion: None,
            ultima_ejecucion: None,
            ciclos_completados: 0,
            ultimo_resultado: None,
        };

        Ok(Arc::new(Self {
            config,
            started_at: Utc::now(),
            http_probe,
            cdp_fetcher,
            memoria: RwLock::new(memoria),
            osint_fuentes: RwLock::new(fuentes_iniciales),
            bob_slot_lock,
            orquestador,
            broll,
            fabrica,
            rate_limiter,
            warmup_gestor,
            despachador,
            telemetria: Arc::new(RecolectorTelemetria::new()),
            motor_dpo,
            consejo,
            scheduler_estado: Mutex::new(scheduler_inicial),
            cola_gatekeeper: Mutex::new(cola_inicial),
            forge_ordenes: Mutex::new(forge_inicial),
            calentamientos_recientes: Mutex::new(Vec::new()),
            despachos_recientes: Mutex::new(Vec::new()),
            ruta_cola_persistida,
            ruta_forge_persistida,
            eventos_tx,
        }))
    }

    /// Devuelve la configuración activa del demonio.
    #[must_use]
    pub const fn config(&self) -> &AuroraDaemonConfig {
        &self.config
    }

    /// Devuelve el mutex exclusivo que protege `slot_id = 0` de BOB en VRAM.
    #[must_use]
    pub fn bob_slot_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.bob_slot_lock)
    }

    /// Devuelve una instantánea del estado actual del scheduler circadiano DPO.
    #[must_use]
    pub fn estado_scheduler_dpo(&self) -> EstadoSchedulerDpo {
        self.scheduler_estado.lock().clone()
    }

    /// Actualiza atómicamente los metadatos visibles del scheduler circadiano DPO.
    pub fn actualizar_estado_scheduler<F>(&self, f: F)
    where
        F: FnOnce(&mut EstadoSchedulerDpo),
    {
        let mut guard = self.scheduler_estado.lock();
        f(&mut guard);
    }

    /// Devuelve referencia al recolector de telemetría (para integración o pruebas).
    #[must_use]
    pub fn telemetria(&self) -> &RecolectorTelemetria {
        &self.telemetria
    }

    /// Suscribe un consumidor al stream de eventos en vivo del demonio.
    #[must_use]
    pub fn suscribir_eventos(&self) -> broadcast::Receiver<EventoDaemon> {
        self.eventos_tx.subscribe()
    }

    /// Registra una fuente OSINT adicional en caliente.
    pub async fn registrar_fuente_osint(&self, fuente: Arc<dyn OsintSource>) {
        self.osint_fuentes.write().await.push(fuente);
    }

    /// Devuelve la salud del demonio, diagnóstico por subsistema y métricas del grafo 4D.
    pub async fn health(&self) -> HealthResponse {
        let subsistemas = self.sondear_subsistemas().await;
        let modo_ejecucion = self.config.modo_efectivo();
        let modo_offline = matches!(modo_ejecucion, ModoEjecucion::SimuladoOffline);
        let bob_online = subsistemas.bob_llm.disponible;

        let (nodos_4d, aristas_4d) = {
            let mem = self.memoria.read().await;
            (mem.grafo().len_nodos(), mem.grafo().len_relaciones())
        };

        let (ordenes_pendientes, calentamientos_pendientes) = {
            let cola = self.cola_gatekeeper.lock();
            let mut ops = 0_usize;
            let mut warms = 0_usize;
            for reg in cola.values() {
                if reg.plan.orden_produccion.estado() == EstadoAprobacion::PendienteGatekeeper {
                    ops += 1;
                }
                if let Some(ref c) = reg.calentamiento {
                    if c.estado() == EstadoAprobacion::PendienteGatekeeper {
                        warms += 1;
                    }
                }
            }
            (ops, warms)
        };

        let uptime_s = (Utc::now() - self.started_at).num_seconds().max(0) as u64;

        HealthResponse {
            status: "ok".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            bind_addr: self.config.bind_addr.clone(),
            modo_offline,
            modo_ejecucion,
            bob_url: self.config.bob_url.clone(),
            bob_online,
            bob_slot_id: self.config.bob_slot_id,
            hardware_vram_modelo: format!(
                "NVIDIA RTX 5060 Ti 16 GB VRAM (BOB slot único id={})",
                self.config.bob_slot_id
            ),
            hardware_ram_sistema: "AMD Ryzen 9 / 64 GB RAM (OSINT + Memoria 4D + TTS CPU)"
                .to_string(),
            subsistemas,
            nodos_4d,
            aristas_4d,
            ordenes_pendientes,
            calentamientos_pendientes,
            telemetria_eventos: self.telemetria.total_eventos(),
            uptime_s,
        }
    }

    /// Ejecuta la sonda en vivo de los 6 subsistemas nativos (BOB, Piper, Chromium, FFmpeg, CDP, SMTP).
    pub async fn sondear_subsistemas(&self) -> ReporteSubsistemas {
        SubsystemProbe::ejecutar(&self.config, &self.http_probe).await
    }

    /// Compacta el WAL 4D y persiste la cola Gatekeeper antes de detener el proceso.
    pub async fn apagar_ordenadamente(&self) -> DaemonResult<()> {
        {
            let mut mem = self.memoria.write().await;
            mem.flush_y_compactar()?;
        }
        if let Some(ref ruta) = self.ruta_cola_persistida {
            let cola = self.cola_gatekeeper.lock();
            persistir_cola_gatekeeper(ruta, &cola)?;
        }
        if let Some(ref ruta) = self.ruta_forge_persistida {
            let ordenes = self.forge_ordenes.lock();
            persistir_ordenes_forge(ruta, &ordenes)?;
        }
        info!("WAL 4D compactado y colas persistidas en apagado ordenado");
        Ok(())
    }

    /// Ejecuta una cacería OSINT -> ingestión 4D -> perfilamiento BOB y encola en Gatekeeper.
    #[tracing::instrument(skip(self, req), fields(ciudad = %req.ciudad, vertical = req.vertical.etiqueta()))]
    pub async fn ejecutar_hunt(&self, req: HuntRequest) -> DaemonResult<HuntResponse> {
        if req.ciudad.trim().is_empty() || req.estado.trim().is_empty() {
            return Err(DaemonError::SolicitudInvalida(
                "los campos 'ciudad' y 'estado' son obligatorios para iniciar una cacería OSINT"
                    .to_string(),
            ));
        }

        let ubicacion = Ubicacion {
            ciudad: req.ciudad.trim().to_string(),
            estado: req.estado.trim().to_string(),
            zona: req.zona.as_ref().map(|z| z.trim().to_string()).filter(|z| !z.is_empty()),
        };

        let mut enjambre = EnjambreOsint::new(4);
        let mut tiene_fuente = false;

        if let Some(semillas) = req.registros_semilla {
            if !semillas.is_empty() {
                enjambre.registrar_fuente(Arc::new(InlineOsintSource {
                    nombre: "FuentePublicaSemilla".to_string(),
                    registros: semillas,
                }));
                tiene_fuente = true;
            }
        }

        if let Some(ref url_publica) = req.fuente_url {
            let trimmed = url_publica.trim();
            if !trimmed.is_empty() {
                if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                    return Err(DaemonError::SolicitudInvalida(
                        "fuente_url debe iniciar con http:// o https://".to_string(),
                    ));
                }
                let http_client = HttpSwarmClient::new(PoliticaHttp::default())?;
                let mut adaptador = SenalesAbiertasAdapter::desde_endpoint(
                    "FuentePublicaCockpit",
                    trimmed,
                    req.fuente_formato.unwrap_or(FormatoCrudo::Html),
                    http_client,
                );
                if req.requiere_js.unwrap_or(false) {
                    if let Some(ref cdp) = self.cdp_fetcher {
                        if cdp.probe_disponible().await {
                            let fetcher: Arc<dyn DynamicBrowserFetcher> = cdp.clone();
                            adaptador = adaptador.con_browser_fetcher(fetcher);
                        } else if matches!(
                            self.config.modo_efectivo(),
                            ModoEjecucion::EstrictoReal
                        ) {
                            return Err(DaemonError::Osint(OsintError::ConfiguracionHttp(
                                format!(
                                    "requiere_js=true en estricto_real pero CDP {} no responde",
                                    self.config.cdp_url
                                ),
                            )));
                        } else {
                            warn!(
                                cdp_url = %self.config.cdp_url,
                                "requiere_js=true pero endpoint CDP no responde; usando extracción HTML directa"
                            );
                        }
                    } else if matches!(self.config.modo_efectivo(), ModoEjecucion::EstrictoReal) {
                        return Err(DaemonError::Osint(OsintError::ConfiguracionHttp(
                            "requiere_js=true pero HttpCdpBrowserFetcher no está configurado"
                                .to_string(),
                        )));
                    }
                }
                enjambre.registrar_fuente(Arc::new(adaptador));
                tiene_fuente = true;
            }
        }

        {
            let registradas = self.osint_fuentes.read().await;
            for src in registradas.iter() {
                enjambre.registrar_fuente(Arc::clone(src));
                tiene_fuente = true;
            }
        }

        if !tiene_fuente && !matches!(self.config.modo_efectivo(), ModoEjecucion::SimuladoOffline) {
            let http_mx = HttpSwarmClient::new(PoliticaHttp::default())?;
            if let Some(ref token) = self.config.inegi_token {
                let denue = DenueInegiAdapter::new(Some(token.clone()), http_mx.clone());
                if denue.tiene_token() {
                    enjambre.registrar_fuente(Arc::new(denue));
                    tiene_fuente = true;
                }
            }
            if self.config.osint_web_publica_habilitada {
                let web_mx = if self.config.osint_web_publica_urls.is_empty() {
                    WebPublicaMxAdapter::new(http_mx)
                } else {
                    WebPublicaMxAdapter::con_urls(
                        "WebPublicaMX",
                        self.config.osint_web_publica_urls.clone(),
                        http_mx,
                    )
                };
                enjambre.registrar_fuente(Arc::new(web_mx));
                tiene_fuente = true;
            }
        }

        if !tiene_fuente {
            return Err(DaemonError::SolicitudInvalida(
                "no hay fuentes OSINT configuradas; proporciona 'fuente_url' HTTP/HTTPS o 'registros_semilla'"
                    .to_string(),
            ));
        }

        let consulta = ConsultaObjetivo {
            vertical: req.vertical,
            ubicacion,
            palabras_clave: req.palabras_clave,
            limite_resultados: req.limite_resultados.unwrap_or(15).clamp(1, 100),
            requiere_js: req.requiere_js.unwrap_or(false),
        };

        let pipeline = PipelineOsint::new(
            enjambre,
            DestiladorOsint::new(),
            EntityResolver::default(),
        );
        let ingesta = pipeline.ejecutar(&consulta).await?;

        let isla = isla_desde_vertical(&req.vertical);
        let prospectos_ingestados = ingesta.prospectos.len();
        let relaciones_creadas = ingesta.relaciones.len();
        let excluidos_opt_out = ingesta.excluidos_opt_out.clone();
        let registros_crudos_totales = ingesta.registros_crudos_totales;
        let ids_objetivo: Vec<Prospecto> = ingesta.prospectos.clone();

        {
            let mut mem = self.memoria.write().await;
            if !ingesta.prospectos.is_empty() {
                mem.ingestar_lote(ingesta.prospectos, ingesta.relaciones, isla)?;
            }
        }

        let campana_id = Uuid::new_v4();
        let mut planes_encolados = Vec::new();
        let ahora = Utc::now();

        for prospecto in &ids_objetivo {
            let _slot_guard = self.bob_slot_lock.lock().await;
            let mem = self.memoria.read().await;
            if mem.esta_en_blacklist(prospecto.id) {
                continue;
            }
            let subgrafo = match construir_subgrafo_por_uuid(&mem, prospecto.id, ahora) {
                Ok(sg) => sg,
                Err(err) => {
                    warn!(prospecto_id = %prospecto.id, error = %err, "Omitiendo prospecto sin subgrafo");
                    continue;
                }
            };

            let registro = self
                .construir_registro_orden(&mem, &subgrafo, campana_id)
                .await?;
            planes_encolados.push(registro);
        }

        let dtos = self.encolar_registros_gatekeeper(planes_encolados)?;

        self.emitir_evento(
            "HuntCompletado",
            serde_json::json!({
                "campana_id": campana_id,
                "prospectos_ingestados": prospectos_ingestados,
                "planes_encolados": dtos.len(),
            }),
        );

        Ok(HuntResponse {
            campana_id,
            isla,
            registros_crudos_totales,
            prospectos_ingestados,
            relaciones_creadas,
            excluidos_opt_out,
            planes_encolados: dtos,
        })
    }

    /// Genera una nueva propuesta comercial con BOB para un prospecto ya existente en la Telaraña 4D
    /// y la encola directamente en Sala Gatekeeper para revisión humana.
    #[tracing::instrument(skip(self), fields(prospecto_id = %prospecto_id))]
    pub async fn generar_plan_para_prospecto(
        &self,
        prospecto_id: Uuid,
    ) -> DaemonResult<OrdenPendienteDto> {
        let campana_id = Uuid::new_v4();
        let ahora = Utc::now();

        let registro = {
            let _slot_guard = self.bob_slot_lock.lock().await;
            let mem = self.memoria.read().await;
            if mem.esta_en_blacklist(prospecto_id) {
                return Err(DaemonError::OptOutActivo(prospecto_id));
            }
            let subgrafo = construir_subgrafo_por_uuid(&mem, prospecto_id, ahora)?;
            self.construir_registro_orden(&mem, &subgrafo, campana_id)
                .await?
        };

        let dtos = self.encolar_registros_gatekeeper(vec![registro])?;
        let dto = dtos.into_iter().next().ok_or_else(|| {
            DaemonError::Interno("no se pudo recuperar la orden recién encolada".to_string())
        })?;

        self.emitir_evento(
            "PlanGenerado",
            serde_json::json!({
                "orden_id": dto.orden_id,
                "prospecto_id": prospecto_id,
                "campana_id": campana_id,
            }),
        );

        Ok(dto)
    }

    async fn construir_registro_orden(
        &self,
        mem: &MotorMemoriaAurora,
        subgrafo: &SubgrafoProspecto,
        campana_id: Uuid,
    ) -> DaemonResult<RegistroOrdenInterno> {
        let prospecto = &subgrafo.prospecto;
        let plan = self
            .orquestador
            .generar_plan_ataque(subgrafo, campana_id, mem)
            .await?;

        let llamado_warm = AuroraToolCall::ProgramarCalentamientoPeriferico {
            prospecto_id: prospecto.id,
            plataforma: PlataformaCalentamiento::LinkedIn,
            tipo_interaccion: TipoInteraccionPeriferica::ComentarioValor,
            contexto_publico: format!(
                "{} ({})",
                prospecto.lfpdppp.fuente_origen,
                prospecto
                    .gatillos
                    .first()
                    .map_or("actividad empresarial pública", |g| g.descripcion.as_str())
            ),
        };

        let calentamiento = match ToolDispatcher.ejecutar(&llamado_warm, campana_id, mem) {
            Ok(ResultadoEjecucionTool::CalentamientoProgramado(cal)) => Some(*cal),
            _ => None,
        };

        let ubicacion_str = format!(
            "{}, {}",
            prospecto.ubicacion.ciudad, prospecto.ubicacion.estado
        );
        Ok(RegistroOrdenInterno {
            plan,
            prospecto_nombre: prospecto.nombre_normalizado.clone(),
            perfil_psicologico: prospecto.perfil_psicologico,
            ubicacion: ubicacion_str,
            isla: subgrafo.isla,
            motivo_rechazo: None,
            calentamiento,
            paquete_manufacturado: None,
            comprobante_despacho: None,
            actualizado_en: Utc::now(),
        })
    }

    fn encolar_registros_gatekeeper(
        &self,
        registros: Vec<RegistroOrdenInterno>,
    ) -> DaemonResult<Vec<OrdenPendienteDto>> {
        let mut dtos = Vec::with_capacity(registros.len());
        let mut cola = self.cola_gatekeeper.lock();
        for reg in registros {
            let dto = reg.a_dto();
            cola.insert(dto.orden_id, reg);
            dtos.push(dto);
        }
        if let Some(ref ruta) = self.ruta_cola_persistida {
            persistir_cola_gatekeeper(ruta, &cola)?;
        }
        Ok(dtos)
    }

    /// Devuelve la topología completa 4D (nodos, gatillos vigentes, aristas) para `axis_v1`.
    pub async fn obtener_telarana(&self) -> DaemonResult<TelaranaResponse> {
        let mem = self.memoria.read().await;
        let grafo = mem.grafo();
        let ahora = Utc::now();

        let mut conteos_isla: BTreeMap<String, (IslaVertical, usize)> = BTreeMap::new();
        let mut nodos = Vec::with_capacity(grafo.len_nodos());

        for reg in grafo.iter_nodos() {
            let entry = conteos_isla
                .entry(reg.isla.nombre().to_string())
                .or_insert((reg.isla, 0));
            entry.1 += 1;

            let relevancia_temporal_4d = grafo.relevancia_temporal_nodo(reg.node_id, ahora);
            let mut gatillos: Vec<GatilloTelaranaDto> = reg
                .prospecto
                .gatillos
                .iter()
                .map(|g| GatilloTelaranaDto {
                    tipo: g.tipo,
                    descripcion: g.descripcion.clone(),
                    ocurrido_en: g.ocurrido_en,
                    peso_urgencia: g.peso_urgencia,
                    peso_vigente_4d: grafo.peso_gatillo_vigente(g, ahora),
                })
                .collect();
            gatillos.sort_by(|a, b| b.peso_vigente_4d.total_cmp(&a.peso_vigente_4d));

            nodos.push(NodoTelaranaDto {
                prospecto_id: reg.prospecto.id,
                node_id: reg.node_id,
                nombre_normalizado: reg.prospecto.nombre_normalizado.clone(),
                isla: reg.isla,
                perfil_psicologico: reg.prospecto.perfil_psicologico,
                ciudad: reg.prospecto.ubicacion.ciudad.clone(),
                estado: reg.prospecto.ubicacion.estado.clone(),
                zona: reg.prospecto.ubicacion.zona.clone(),
                ingreso_anual_mxn: reg.prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn,
                patrimonio_estimado_mxn: reg
                    .prospecto
                    .capacidad_adquisitiva
                    .patrimonio_estimado_mxn,
                relevancia_temporal_4d,
                fuente_origen: reg.prospecto.lfpdppp.fuente_origen.clone(),
                url_evidencia: reg.prospecto.lfpdppp.url_evidencia.clone(),
                gatillos,
            });
        }

        nodos.sort_by(|a, b| {
            b.relevancia_temporal_4d
                .total_cmp(&a.relevancia_temporal_4d)
                .then_with(|| a.nombre_normalizado.cmp(&b.nombre_normalizado))
        });

        let aristas: Vec<AristaTelaranaDto> = grafo
            .iter_relaciones()
            .map(|r| AristaTelaranaDto {
                origen_id: r.origen_id,
                destino_id: r.destino_id,
                tipo_vinculo: r.tipo_vinculo,
                fuerza: r.fuerza,
                contexto_compartido: r.contexto_compartido.clone(),
            })
            .collect();

        let resumen_islas = conteos_isla
            .into_values()
            .map(|(isla, cantidad)| ResumenIslaDto { isla, cantidad })
            .collect();

        Ok(TelaranaResponse {
            lambda_por_dia: grafo.config().lambda_por_dia,
            total_nodos: nodos.len(),
            total_aristas: aristas.len(),
            resumen_islas,
            nodos,
            aristas,
        })
    }

    /// Consulta el subgrafo topológico detallado de un prospecto (404 si fue purgado o no existe).
    pub async fn obtener_subgrafo(&self, prospecto_id: Uuid) -> DaemonResult<SubgrafoProspecto> {
        let mem = self.memoria.read().await;
        if mem.esta_en_blacklist(prospecto_id) {
            return Err(DaemonError::ProspectoNoEncontrado(prospecto_id));
        }
        construir_subgrafo_por_uuid(&mem, prospecto_id, Utc::now())
    }

    /// Lista todas las órdenes en `EstadoAprobacion::PendienteGatekeeper` y resumen histórico reciente.
    pub fn listar_gatekeeper_pendientes(&self) -> GatekeeperPendientesResponse {
        let cola = self.cola_gatekeeper.lock();
        let mut ordenes = Vec::new();
        let mut historial_reciente = Vec::new();

        for reg in cola.values() {
            let dto = reg.a_dto();
            if dto.estado == EstadoAprobacion::PendienteGatekeeper {
                ordenes.push(dto);
            } else {
                historial_reciente.push(dto);
            }
        }

        ordenes.sort_by_key(|b| std::cmp::Reverse(b.actualizado_en));
        historial_reciente.sort_by_key(|b| std::cmp::Reverse(b.actualizado_en));
        historial_reciente.truncate(20);

        GatekeeperPendientesResponse {
            total_pendientes: ordenes.len(),
            total_en_cola: cola.len(),
            ordenes,
            historial_reciente,
        }
    }

    /// Aprueba una orden en Gatekeeper, dispara manufactura multimodal NVMe y programa pulso/calentamiento.
    #[tracing::instrument(skip(self, req), fields(orden_id = %orden_id))]
    pub async fn aprobar_orden(
        &self,
        orden_id: Uuid,
        req: Option<AprobarOrdenRequest>,
    ) -> DaemonResult<AprobarOrdenResponse> {
        let (despachar_inmediatamente, correo_destino) = match req {
            Some(r) => (r.despachar_inmediatamente.unwrap_or(false), r.correo_destino),
            None => (false, None),
        };

        // 1. Extraemos clon y validamos estado PendienteGatekeeper sin sostener lock de Mutex en `.await`.
        let mut reg = {
            let cola = self.cola_gatekeeper.lock();
            let reg = cola
                .get(&orden_id)
                .cloned()
                .ok_or(DaemonError::OrdenNoEncontrada(orden_id))?;

            if reg.plan.orden_produccion.estado() != EstadoAprobacion::PendienteGatekeeper {
                return Err(DaemonError::BloqueoGatekeeper(format!(
                    "la orden {orden_id} está en estado {:?}; se esperaba PendienteGatekeeper",
                    reg.plan.orden_produccion.estado()
                )));
            }
            reg
        };

        let prospecto = {
            let mem = self.memoria.read().await;
            if mem.esta_en_blacklist(reg.plan.prospecto_id) {
                return Err(DaemonError::OptOutActivo(reg.plan.prospecto_id));
            }
            let nodo = mem
                .grafo()
                .obtener_por_uuid(reg.plan.prospecto_id)
                .ok_or(DaemonError::ProspectoNoEncontrado(reg.plan.prospecto_id))?;
            let prospecto = nodo.prospecto.clone();
            validar_prospecto(&prospecto)?;
            prospecto
        };

        // 2. Aprobar orden + calentamiento en Gatekeeper Zero-Bypass
        reg.plan.orden_produccion.aprobar_gatekeeper()?;
        if let Some(ref mut cal) = reg.calentamiento {
            cal.aprobar_gatekeeper()?;
        }

        // 3. Manufactura multimodal local en NVMe (PDF + WAV + MP4 + SRT con SHA-256)
        let solicitud = SolicitudManufactura {
            orden: reg.plan.orden_produccion.clone(),
            prospecto: prospecto.clone(),
            diagnostico: reg.plan.diagnostico.clone(),
            brief: reg.plan.brief_creativo.clone(),
        };
        let paquete = self.fabrica.manufacturar(&solicitud).await?;

        // 4. Calentamiento periférico y despacho opcional en aurora-pulse
        let ahora = Utc::now();
        let mut calentamiento_ejecutado = None;
        let mut comprobante_despacho = None;

        {
            let mem = self.memoria.read().await;
            if let Some(ref cal) = reg.calentamiento {
                let registro_warm = self
                    .warmup_gestor
                    .ejecutar_calentamiento(cal, &mem, ahora)
                    .await?;
                self.calentamientos_recientes
                    .lock()
                    .push(registro_warm.clone());
                calentamiento_ejecutado = Some(registro_warm);
            }

            if despachar_inmediatamente {
                let comprobante = self
                    .despachador
                    .despachar_con_correo(
                        &paquete,
                        &prospecto,
                        reg.plan.brief_creativo.canal_sugerido,
                        correo_destino,
                        &mem,
                        ahora + Duration::hours(36),
                    )
                    .await?;
                self.telemetria.registrar_despacho(
                    &comprobante,
                    Some(reg.isla),
                    reg.plan.brief_creativo.gancho_3s.clone(),
                    reg.plan.brief_creativo.guion_tts.clone(),
                )?;
                self.despachos_recientes.lock().push(comprobante.clone());
                comprobante_despacho = Some(comprobante);
            }
        }

        let estado_final = if comprobante_despacho.is_some() {
            EstadoAprobacion::Despachado
        } else {
            EstadoAprobacion::Aprobado
        };

        reg.paquete_manufacturado = Some(paquete.clone());
        reg.comprobante_despacho.clone_from(&comprobante_despacho);
        reg.actualizado_en = Utc::now();

        let prospecto_id = reg.plan.prospecto_id;
        let campana_id = reg.plan.campana_id;

        {
            let mut cola = self.cola_gatekeeper.lock();
            cola.insert(orden_id, reg);
            if let Some(ref ruta) = self.ruta_cola_persistida {
                persistir_cola_gatekeeper(ruta, &cola)?;
            }
        }

        self.emitir_evento(
            "OrdenAprobadaManufacturada",
            serde_json::json!({
                "orden_id": orden_id,
                "prospecto_id": prospecto_id,
                "estado": estado_final,
            }),
        );

        info!(%orden_id, %prospecto_id, ?estado_final, "Orden aprobada, manufacturada y programada en Pulse");

        Ok(AprobarOrdenResponse {
            orden_id,
            prospecto_id,
            campana_id,
            estado: estado_final,
            paquete_manufacturado: paquete,
            calentamiento_ejecutado,
            comprobante_despacho,
        })
    }

    /// Rechaza formalmente una orden pendiente en Gatekeeper registrando el motivo de auditoría.
    pub fn rechazar_orden(
        &self,
        orden_id: Uuid,
        req: RechazarOrdenRequest,
    ) -> DaemonResult<OrdenPendienteDto> {
        if req.motivo.trim().is_empty() {
            return Err(DaemonError::SolicitudInvalida(
                "el motivo de rechazo en Gatekeeper no puede estar vacío".to_string(),
            ));
        }

        let dto = {
            let mut cola = self.cola_gatekeeper.lock();
            let reg = cola
                .get_mut(&orden_id)
                .ok_or(DaemonError::OrdenNoEncontrada(orden_id))?;

            if reg.plan.orden_produccion.estado() != EstadoAprobacion::PendienteGatekeeper {
                return Err(DaemonError::BloqueoGatekeeper(format!(
                    "no se puede rechazar la orden {orden_id} porque se encuentra en {:?}",
                    reg.plan.orden_produccion.estado()
                )));
            }

            reg.plan.orden_produccion.rechazar_gatekeeper(&req.motivo)?;
            if let Some(ref mut cal) = reg.calentamiento {
                let _ = cal.rechazar_gatekeeper(&req.motivo);
            }
            reg.motivo_rechazo = Some(req.motivo.trim().to_string());
            reg.actualizado_en = Utc::now();
            let dto = reg.a_dto();
            if let Some(ref ruta) = self.ruta_cola_persistida {
                persistir_cola_gatekeeper(ruta, &cola)?;
            }
            dto
        };

        self.emitir_evento(
            "OrdenRechazada",
            serde_json::json!({
                "orden_id": orden_id,
                "motivo": dto.motivo_rechazo,
            }),
        );

        Ok(dto)
    }

    /// Ejecuta baja LFPDPPP (`opt_out`), detiene telemetría y purga WAL + HNSW + cola pendiente.
    #[tracing::instrument(skip(self, req), fields(prospecto_id = %req.prospecto_id))]
    pub async fn ejecutar_opt_out(&self, req: OptOutRequest) -> DaemonResult<OptOutResponse> {
        let motivo = if req.motivo.trim().is_empty() {
            "Solicitud de exclusión LFPDPPP desde Cockpit".to_string()
        } else {
            req.motivo.trim().to_string()
        };

        let en_blacklist = {
            let mut mem = self.memoria.write().await;
            self.telemetria.procesar_solicitud_opt_out(
                req.prospecto_id,
                req.campana_id,
                &motivo,
                &mut mem,
            )?;
            mem.esta_en_blacklist(req.prospecto_id)
        };

        let mut ordenes_canceladas = 0_usize;
        {
            let mut cola = self.cola_gatekeeper.lock();
            for reg in cola.values_mut() {
                if reg.plan.prospecto_id == req.prospecto_id
                    && reg.plan.orden_produccion.estado() == EstadoAprobacion::PendienteGatekeeper
                {
                    let _ = reg
                        .plan
                        .orden_produccion
                        .rechazar_gatekeeper(format!("Cancelada por opt_out LFPDPPP: {motivo}"));
                    if let Some(ref mut cal) = reg.calentamiento {
                        let _ = cal.rechazar_gatekeeper("Exclusión opt_out LFPDPPP");
                    }
                    reg.motivo_rechazo = Some(motivo.clone());
                    reg.actualizado_en = Utc::now();
                    ordenes_canceladas += 1;
                }
            }
            if let Some(ref ruta) = self.ruta_cola_persistida {
                persistir_cola_gatekeeper(ruta, &cola)?;
            }
        }

        self.emitir_evento(
            "OptOutPurgadoLfpdppp",
            serde_json::json!({
                "prospecto_id": req.prospecto_id,
                "ordenes_canceladas": ordenes_canceladas,
            }),
        );

        Ok(OptOutResponse {
            prospecto_id: req.prospecto_id,
            purgado_de_wal: true,
            en_blacklist,
            ordenes_canceladas,
            mensaje: format!(
                "Prospecto {} excluido bajo LFPDPPP, WAL/HNSW compactado y {} orden(es) detenidas.",
                req.prospecto_id, ordenes_canceladas
            ),
        })
    }

    /// Devuelve métricas en vivo de cuotas por plataforma, circuit breakers y telemetría de campañas.
    pub fn estado_pulso(&self) -> PulseEstadoResponse {
        let ahora = Utc::now();
        let plataformas = PlataformaPulso::todas()
            .iter()
            .map(|&p| self.rate_limiter.estado_plataforma(p, ahora))
            .collect();

        let calentamientos_recientes = {
            let guard = self.calentamientos_recientes.lock();
            let inicio = guard.len().saturating_sub(25);
            guard[inicio..].to_vec()
        };

        let despachos_recientes = {
            let guard = self.despachos_recientes.lock();
            let inicio = guard.len().saturating_sub(25);
            guard[inicio..].to_vec()
        };

        PulseEstadoResponse {
            plataformas,
            campanas_activas: self.telemetria.campanas_registradas(),
            estadisticas_ganchos: self.telemetria.exportar_estadisticas_ganchos(),
            telemetria_eventos_totales: self.telemetria.total_eventos(),
            calentamientos_recientes,
            despachos_recientes,
            scheduler_dpo: self.estado_scheduler_dpo(),
        }
    }

    /// Registra manualmente un evento de telemetría comercial (para integraciones webhook o pruebas).
    pub async fn registrar_evento_telemetria(
        &self,
        evento: EventoTelemetria,
    ) -> DaemonResult<aurora_pulse::EstadoCampanaProspecto> {
        if matches!(evento.tipo, TipoEventoTelemetria::SolicitudBajaOptOut { .. }) {
            let mut mem = self.memoria.write().await;
            let st = self
                .telemetria
                .registrar_evento_con_memoria(evento, &mut mem)?;
            return Ok(st);
        }
        Ok(self.telemetria.registrar_evento(evento)?)
    }

    /// Ejecuta un ciclo de consolidación nocturna DPO y recalibra el `GrafoTemporal4D`,
    /// protegiendo el `slot_id = 0` de BOB mediante exclusión mutua asíncrona.
    #[tracing::instrument(skip(self))]
    pub async fn consolidar_dpo(&self) -> DaemonResult<DpoConsolidarResponse> {
        let dataset = {
            let _slot_guard = self.bob_slot_lock.lock().await;
            let mut mem = self.memoria.write().await;
            self.motor_dpo
                .consolidar(&self.telemetria, &mut mem)
                .await?
        };

        self.emitir_evento(
            "DpoConsolidado",
            serde_json::json!({
                "ruta_jsonl": dataset.ruta_jsonl,
                "entradas": dataset.entradas.len(),
                "nodos_4d_recalibrados": dataset.nodos_4d_recalibrados,
                "campanas_auditadas": dataset.campanas_auditadas.len(),
                "modo_noop": dataset.modo_noop,
                "mensaje": dataset.mensaje,
            }),
        );

        Ok(DpoConsolidarResponse {
            campanas_auditadas: dataset.campanas_auditadas.len(),
            modo_noop: dataset.modo_noop,
            mensaje: dataset.mensaje.clone(),
            dataset,
        })
    }

    /// Ejecuta una corrida del scheduler nocturno DPO y actualiza sus contadores observables.
    pub async fn ejecutar_ciclo_scheduler_dpo(&self) -> DaemonResult<DpoConsolidarResponse> {
        let resp = self.consolidar_dpo().await?;
        self.actualizar_estado_scheduler(|est| {
            est.ultima_ejecucion = Some(Utc::now());
            est.ciclos_completados = est.ciclos_completados.saturating_add(1);
            est.ultimo_resultado = Some(resp.mensaje.clone());
        });
        Ok(resp)
    }

    /// Devuelve el estado actual de ventanas 5h, topes diarios y cooldowns del Consejo Multi-Agente.
    pub async fn obtener_cuotas_consejo(&self) -> CouncilCuotasResponse {
        let gov = self.consejo.governor();
        let cascada_vigente = gov.orden_cascada_vigente().await;
        let miembros = gov.estado_todas_las_cuotas().await;
        CouncilCuotasResponse {
            cascada_vigente,
            miembros,
        }
    }

    /// Delega una tarea a través de [`OrquestadorConsejo`] con `PiiFirewall` LFPDPPP activo.
    #[tracing::instrument(skip(self, req), fields(tarea = req.tarea.etiqueta()))]
    pub async fn delegar_consejo(
        &self,
        req: DelegarConsejoRequest,
    ) -> DaemonResult<ResultadoDelegacion> {
        if req.contexto.trim().is_empty() {
            return Err(DaemonError::SolicitudInvalida(
                "el campo 'contexto' es obligatorio para delegar al consejo".to_string(),
            ));
        }

        let prospecto = if let Some(p) = req.prospecto {
            if p.lfpdppp.opt_out {
                return Err(DaemonError::OptOutActivo(p.id));
            }
            Some(p)
        } else if let Some(pid) = req.prospecto_id {
            let mem = self.memoria.read().await;
            if mem.esta_en_blacklist(pid) {
                return Err(DaemonError::OptOutActivo(pid));
            }
            let reg = mem
                .grafo()
                .obtener_por_uuid(pid)
                .ok_or(DaemonError::ProspectoNoEncontrado(pid))?;
            Some(reg.prospecto.clone())
        } else {
            None
        };

        let resultado = self
            .consejo
            .delegar(PeticionDelegacion {
                tarea: req.tarea,
                contexto: req.contexto,
                prospecto,
                forzar_local: req.forzar_local.unwrap_or(false),
                directorio_trabajo: None,
                timeout_ms: req.timeout_ms,
            })
            .await?;

        self.emitir_evento(
            "ConsejoDelegado",
            serde_json::json!({
                "solicitud_id": resultado.solicitud_id,
                "tarea": resultado.tarea.etiqueta(),
                "miembro_respondedor": resultado.miembro_respondedor.etiqueta(),
                "hubo_fallback": resultado.hubo_fallback,
                "latencia_ms": resultado.latencia_ms,
                "pii_sanitizado": resultado.pii_sanitizado,
            }),
        );

        Ok(resultado)
    }

    /// Lista las órdenes registradas de `aurora-forge` y las ramas `forge/*` pendientes de revisión.
    pub async fn listar_ordenes_forge(&self) -> ForgeOrdenesResponse {
        let repo_root = self.config.forge_repo_root_efectivo();
        let ramas_forge = listar_ramas_forge(&repo_root).await.unwrap_or_default();

        let ordenes = self.forge_ordenes.lock().clone();
        let pendientes_revision_humana = ordenes
            .iter()
            .filter(|o| o.orden.estado == EstadoOrdenIngenieria::PendienteRevisionHumana)
            .count();

        ForgeOrdenesResponse {
            habilitado: self.config.forge.habilitado,
            total_ordenes: ordenes.len(),
            pendientes_revision_humana,
            ramas_forge,
            ordenes,
        }
    }

    /// Registra y ejecuta una [`OrdenIngenieria`] en un `git worktree` aislado de `aurora-forge`.
    #[tracing::instrument(skip(self, req), fields(tipo = req.tipo.etiqueta()))]
    pub async fn crear_y_ejecutar_orden_forge(
        &self,
        req: CrearOrdenForjaRequest,
    ) -> DaemonResult<RegistroOrdenForjaDto> {
        if !self.config.forge.habilitado {
            return Err(DaemonError::SolicitudInvalida(
                "aurora-forge está deshabilitado en la configuración ([forge].habilitado = false)"
                    .to_string(),
            ));
        }

        validar_rutas_permitidas(&req.rutas_permitidas)?;

        let mut orden = OrdenIngenieria::nueva(
            req.tipo,
            req.descripcion_tecnica,
            req.rutas_permitidas,
            req.comando_validacion
                .unwrap_or(ComandoValidacion::CargoCheckClippyTest),
        );

        let repo_root = req
            .repo_root
            .map(|p| expandir_tilde_path(&p))
            .unwrap_or_else(|| self.config.forge_repo_root_efectivo());

        let sandbox = WorktreeSandbox::new(ConfigSandboxForja {
            repo_root,
            worktrees_dir: self.config.forge_worktrees_dir_efectivo(),
            timeout_segundos: self.config.forge.timeout_segundos,
        });
        let executor =
            CouncilForgeExecutor::new(Arc::clone(&self.consejo), self.config.council.timeout_ms);

        match sandbox.ejecutar_orden(&mut orden, &executor).await {
            Ok(reporte) => {
                let dto = RegistroOrdenForjaDto {
                    orden,
                    reporte: Some(reporte.clone()),
                    error: None,
                    actualizado_en: Utc::now(),
                };
                self.guardar_orden_forge(dto.clone())?;
                self.emitir_evento(
                    "OrdenForjaCompletada",
                    serde_json::json!({
                        "orden_id": dto.orden.id,
                        "rama_forge": reporte.rama_forge,
                        "estado": dto.orden.estado,
                        "archivos_modificados": reporte.archivos_modificados,
                    }),
                );
                Ok(dto)
            }
            Err(err) => {
                let dto = RegistroOrdenForjaDto {
                    orden,
                    reporte: None,
                    error: Some(err.to_string()),
                    actualizado_en: Utc::now(),
                };
                let _ = self.guardar_orden_forge(dto);
                Err(DaemonError::Forja(err))
            }
        }
    }

    /// Devuelve el inventario completo de la biblioteca B-Roll, usos, brechas y briefs Higgsfield.
    pub async fn obtener_estado_broll(&self) -> BrollEstadoResponse {
        let bib = self.broll.lock().await;
        BrollEstadoResponse {
            broll_dir: bib.directorio_raiz().to_path_buf(),
            total_clips: bib.indice().clips.len(),
            clips_por_vertical: bib.conteo_por_vertical(),
            clips: bib.indice().clips.clone(),
            brechas_cobertura: bib.detectar_brechas_cobertura(),
            briefs_higgsfield: bib.cola_briefs().briefs.clone(),
        }
    }

    /// Importa y valida un clip `.mp4`/`.mov` local descargado de Higgsfield en la biblioteca NVMe.
    #[tracing::instrument(skip(self, req), fields(vertical = req.vertical.etiqueta()))]
    pub async fn registrar_clip_broll(
        &self,
        req: RegistrarClipBrollRequest,
    ) -> DaemonResult<ClipBroll> {
        validar_ruta_sin_traversal(&req.ruta_origen)?;
        let ruta_resuelta = expandir_tilde_path(&req.ruta_origen);

        let mut bib = self.broll.lock().await;
        let clip = bib.registrar_clip(SolicitudRegistroClipBroll {
            ruta_origen: ruta_resuelta,
            vertical: req.vertical,
            perfil_objetivo: req.perfil_objetivo,
            etiquetas: req.etiquetas,
            duracion_ms: req.duracion_ms.unwrap_or(5_000),
            ancho: req.ancho.unwrap_or(1080),
            alto: req.alto.unwrap_or(1920),
            movimiento_camara: req
                .movimiento_camara
                .unwrap_or_else(|| "dolly_in_slow".to_string()),
        })?;

        if let Some(brief_id) = req.brief_id {
            let _ = bib.marcar_brief_importado(brief_id, clip.id);
        }

        self.emitir_evento(
            "ClipBrollRegistrado",
            serde_json::json!({
                "clip_id": clip.id,
                "vertical": clip.vertical.etiqueta(),
                "sha256": clip.sha256,
            }),
        );

        Ok(clip)
    }

    /// Genera prompts cinematográficos estructurados para Higgsfield y los persiste en la cola NVMe.
    #[tracing::instrument(skip(self, req), fields(vertical = req.vertical.etiqueta()))]
    pub async fn generar_briefs_higgsfield(
        &self,
        req: GenerarBriefsHiggsfieldRequest,
    ) -> DaemonResult<GenerarBriefsHiggsfieldResponse> {
        let mut bib = self.broll.lock().await;
        let briefs_generados = bib.generar_briefs_higgsfield(
            req.vertical,
            req.perfil_objetivo,
            req.cantidad.unwrap_or(3),
        )?;
        let total_en_cola = bib.cola_briefs().briefs.len();

        self.emitir_evento(
            "BriefsHiggsfieldGenerados",
            serde_json::json!({
                "vertical": req.vertical.etiqueta(),
                "generados": briefs_generados.len(),
                "total_en_cola": total_en_cola,
            }),
        );

        Ok(GenerarBriefsHiggsfieldResponse {
            briefs_generados,
            total_en_cola,
        })
    }

    fn guardar_orden_forge(&self, dto: RegistroOrdenForjaDto) -> DaemonResult<()> {
        let mut ordenes = self.forge_ordenes.lock();
        ordenes.push(dto);
        if let Some(ref ruta) = self.ruta_forge_persistida {
            persistir_ordenes_forge(ruta, &ordenes)?;
        }
        Ok(())
    }

    fn emitir_evento(&self, tipo: &str, payload: serde_json::Value) {
        let _ = self.eventos_tx.send(EventoDaemon {
            tipo: tipo.to_string(),
            timestamp: Utc::now(),
            payload,
        });
    }
}

fn construir_subgrafo_por_uuid(
    memoria: &MotorMemoriaAurora,
    prospecto_id: Uuid,
    ahora: DateTime<Utc>,
) -> DaemonResult<SubgrafoProspecto> {
    let grafo = memoria.grafo();
    let reg = grafo
        .obtener_por_uuid(prospecto_id)
        .ok_or(DaemonError::ProspectoNoEncontrado(prospecto_id))?;

    let score_temporal_4d = grafo.relevancia_temporal_nodo(reg.node_id, ahora);
    let (boost_circulo_cercano, vecinos_prop) =
        grafo.propagar_senal_circulo_cercano(reg.node_id, ahora, 2);

    let mut gatillos_vigentes: Vec<GatilloVigente> = reg
        .prospecto
        .gatillos
        .iter()
        .map(|g| GatilloVigente {
            gatillo: g.clone(),
            peso_vigente_4d: grafo.peso_gatillo_vigente(g, ahora),
            antiguedad_horas: ((ahora - g.ocurrido_en).num_seconds().max(0) as f64) / 3600.0,
        })
        .collect();
    gatillos_vigentes.sort_by(|a, b| b.peso_vigente_4d.total_cmp(&a.peso_vigente_4d));

    let mut clusteres_set = BTreeSet::new();
    for alias in &reg.prospecto.aliases {
        clusteres_set.insert(alias.clone());
    }

    let vecinos_circulo_cercano: Vec<VecinoCirculoCercano> = vecinos_prop
        .into_iter()
        .map(|v| {
            clusteres_set.insert(v.contexto_compartido.clone());
            VecinoCirculoCercano {
                prospecto_id: v.prospecto_id,
                nombre_normalizado: v.nombre_normalizado,
                tipo_vinculo: v.tipo_vinculo,
                fuerza_vinculo: v.fuerza_ruta,
                contexto_compartido: v.contexto_compartido,
                salto: v.salto,
                relevancia_propia_4d: v.relevancia_propia_4d,
            }
        })
        .collect();

    Ok(SubgrafoProspecto {
        prospecto: reg.prospecto.clone(),
        isla: reg.isla,
        score_compuesto: (0.65 * score_temporal_4d + 0.35 * boost_circulo_cercano).clamp(0.0, 1.5),
        similitud_vectorial: 1.0,
        score_temporal_4d,
        boost_circulo_cercano,
        gatillos_vigentes,
        vecinos_circulo_cercano,
        clusteres_afines: clusteres_set.into_iter().collect(),
    })
}

fn isla_desde_vertical(vertical: &VerticalComercial) -> IslaVertical {
    use aurora_domain::{SubverticalInmobiliaria, SubverticalSeguros};
    match vertical {
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa) => {
            IslaVertical::InmobiliariaPreventa
        }
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial) => {
            IslaVertical::InmobiliariaResidencial
        }
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial) => {
            IslaVertical::InmobiliariaComercialIndustrial
        }
        VerticalComercial::Seguros(SubverticalSeguros::Vida) => IslaVertical::SegurosVida,
        VerticalComercial::Seguros(SubverticalSeguros::Gmm) => IslaVertical::SegurosGmm,
        VerticalComercial::Seguros(SubverticalSeguros::HombreClave) => {
            IslaVertical::SegurosHombreClave
        }
        VerticalComercial::Seguros(SubverticalSeguros::Educativo) => IslaVertical::SegurosEducativo,
        VerticalComercial::Seguros(SubverticalSeguros::Patrimonial) => {
            IslaVertical::SegurosPatrimonial
        }
    }
}

fn cargar_cola_gatekeeper(ruta: &Path) -> DaemonResult<BTreeMap<Uuid, RegistroOrdenInterno>> {
    if !ruta.is_file() {
        return Ok(BTreeMap::new());
    }
    let file = File::open(ruta).map_err(|e| {
        DaemonError::Interno(format!(
            "error abriendo cola persistente '{}': {e}",
            ruta.display()
        ))
    })?;
    let reader = BufReader::new(file);
    let regs: Vec<RegistroOrdenInterno> = serde_json::from_reader(reader).map_err(|e| {
        DaemonError::Interno(format!(
            "error deserializando cola Gatekeeper '{}': {e}",
            ruta.display()
        ))
    })?;
    let mut mapa = BTreeMap::new();
    for reg in regs {
        mapa.insert(reg.plan.orden_produccion.orden_id, reg);
    }
    debug!(total = mapa.len(), "Cola Gatekeeper restaurada desde NVMe");
    Ok(mapa)
}

fn persistir_cola_gatekeeper(
    ruta: &Path,
    cola: &BTreeMap<Uuid, RegistroOrdenInterno>,
) -> DaemonResult<()> {
    let registros: Vec<&RegistroOrdenInterno> = cola.values().collect();
    let json = serde_json::to_vec_pretty(&registros).map_err(|e| {
        DaemonError::Interno(format!("error serializando cola Gatekeeper: {e}"))
    })?;
    let tmp = ruta.with_extension("json.tmp");
    fs::write(&tmp, &json).map_err(|e| {
        DaemonError::Interno(format!(
            "error escribiendo cola Gatekeeper temporal '{}': {e}",
            tmp.display()
        ))
    })?;
    fs::rename(&tmp, ruta).map_err(|e| {
        DaemonError::Interno(format!(
            "error publicando cola Gatekeeper '{}': {e}",
            ruta.display()
        ))
    })?;
    Ok(())
}

fn cargar_ordenes_forge(ruta: &Path) -> DaemonResult<Vec<RegistroOrdenForjaDto>> {
    if !ruta.is_file() {
        return Ok(Vec::new());
    }
    let file = File::open(ruta).map_err(|e| {
        DaemonError::Interno(format!(
            "error abriendo historial de aurora-forge '{}': {e}",
            ruta.display()
        ))
    })?;
    let reader = BufReader::new(file);
    let regs: Vec<RegistroOrdenForjaDto> = serde_json::from_reader(reader).map_err(|e| {
        DaemonError::Interno(format!(
            "error deserializando historial de aurora-forge '{}': {e}",
            ruta.display()
        ))
    })?;
    Ok(regs)
}

fn persistir_ordenes_forge(ruta: &Path, ordenes: &[RegistroOrdenForjaDto]) -> DaemonResult<()> {
    if let Some(padre) = ruta.parent() {
        if !padre.as_os_str().is_empty() {
            fs::create_dir_all(padre).map_err(|e| {
                DaemonError::Interno(format!(
                    "error creando directorio de historial forge '{}': {e}",
                    padre.display()
                ))
            })?;
        }
    }
    let json = serde_json::to_vec_pretty(ordenes).map_err(|e| {
        DaemonError::Interno(format!("error serializando historial de aurora-forge: {e}"))
    })?;
    let tmp = ruta.with_extension("json.tmp");
    fs::write(&tmp, &json).map_err(|e| {
        DaemonError::Interno(format!(
            "error escribiendo historial temporal de aurora-forge '{}': {e}",
            tmp.display()
        ))
    })?;
    fs::rename(&tmp, ruta).map_err(|e| {
        DaemonError::Interno(format!(
            "error publicando historial de aurora-forge '{}': {e}",
            ruta.display()
        ))
    })?;
    Ok(())
}
