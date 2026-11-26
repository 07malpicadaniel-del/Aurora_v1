use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aurora_council::{
    ConfigCuotasConsejo, MiembroConsejo, PoliticaCuotaProveedor, DEFAULT_COUNCIL_CLI_TIMEOUT_MS,
};
use aurora_domain::VerticalComercial;
use aurora_factory::{ConfigHiggsfield, DEFAULT_HIGGSFIELD_API_URL};
use aurora_osint::{ConfigCdpFetcher, HttpCdpBrowserFetcher};
use aurora_pulse::{
    ConfigMetaReels, ConfigSmtp, ConfigTikTok, ConfigYouTubeShorts, ModoPublicacionTikTok,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{DaemonError, DaemonResult};

/// Política de fallback entre adaptadores reales (BOB, Piper, Chromium, FFmpeg, CDP, SMTP)
/// y respaldo determinista en CI/desarrollo sin hardware o dependencias auxiliares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoEjecucion {
    /// Prioriza motores reales del sistema y degrada limpiamente a respaldo solo si un binario/servicio falta.
    #[default]
    RealConFallback,
    /// Exige ejecución 100 % real en el sistema (falla explícitamente si falta BOB/Piper/Chromium/FFmpeg/SMTP).
    EstrictoReal,
    /// Opera íntegramente con adaptadores deterministas en memoria (CI y pruebas automatizadas).
    SimuladoOffline,
}

impl ModoEjecucion {
    /// Etiqueta legible para UI y logs estructurados.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::RealConFallback => "real_con_fallback",
            Self::EstrictoReal => "estricto_real",
            Self::SimuladoOffline => "simulado_offline",
        }
    }

    fn desde_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "real_con_fallback" | "hybrid" | "hibrido" => Some(Self::RealConFallback),
            "estricto_real" | "strict" | "estricto" | "prod" => Some(Self::EstrictoReal),
            "simulado_offline" | "offline" | "mock" => Some(Self::SimuladoOffline),
            _ => None,
        }
    }
}

/// Configuración de la sección `[council]` en `aurora.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigCouncilDaemon {
    /// Binario o ruta para Claude Code CLI (`claude`).
    pub claude_bin: PathBuf,
    /// Binario o ruta para Codex CLI (`codex`).
    pub codex_bin: PathBuf,
    /// Binario o ruta para Gemini CLI (`gemini`).
    pub gemini_bin: PathBuf,
    /// Binario o ruta para OpenCode CLI local (`opencode`).
    pub opencode_bin: PathBuf,
    /// Tope móvil 5h para Claude Pro.
    pub claude_max_5h: u32,
    /// Tope diario para Claude Pro.
    pub claude_max_diario: u32,
    /// Tope móvil 5h para Codex Plus.
    pub codex_max_5h: u32,
    /// Tope diario para Codex Plus.
    pub codex_max_diario: u32,
    /// Tope móvil 5h para Gemini Estudiante.
    pub gemini_max_5h: u32,
    /// Tope diario para Gemini Estudiante.
    pub gemini_max_diario: u32,
    /// Tope móvil 5h para OpenCode Local.
    pub opencode_max_5h: u32,
    /// Tope diario para OpenCode Local.
    pub opencode_max_diario: u32,
    /// Segundos de cooldown automático tras un rate-limit externo (HTTP 429 / usage limit).
    pub cooldown_rate_limit_segundos: u64,
    /// Timeout máximo por invocación CLI en milisegundos.
    pub timeout_ms: u64,
    /// Ruta explícita al archivo JSON persistente de cuotas (`<data_dir>/quota_state.json` si se omite).
    pub quota_state_path: Option<PathBuf>,
}

impl Default for ConfigCouncilDaemon {
    fn default() -> Self {
        let claude = PoliticaCuotaProveedor::por_defecto_para(MiembroConsejo::ClaudePro);
        let codex = PoliticaCuotaProveedor::por_defecto_para(MiembroConsejo::CodexPlus);
        let gemini = PoliticaCuotaProveedor::por_defecto_para(MiembroConsejo::GeminiEstudiante);
        let opencode = PoliticaCuotaProveedor::por_defecto_para(MiembroConsejo::OpenCodeLocal);
        Self {
            claude_bin: PathBuf::from("claude"),
            codex_bin: PathBuf::from("codex"),
            gemini_bin: PathBuf::from("gemini"),
            opencode_bin: PathBuf::from("opencode"),
            claude_max_5h: claude.max_tareas_ventana_5h,
            claude_max_diario: claude.max_tareas_diarias,
            codex_max_5h: codex.max_tareas_ventana_5h,
            codex_max_diario: codex.max_tareas_diarias,
            gemini_max_5h: gemini.max_tareas_ventana_5h,
            gemini_max_diario: gemini.max_tareas_diarias,
            opencode_max_5h: opencode.max_tareas_ventana_5h,
            opencode_max_diario: opencode.max_tareas_diarias,
            cooldown_rate_limit_segundos: claude.cooldown_rate_limit_segundos,
            timeout_ms: DEFAULT_COUNCIL_CLI_TIMEOUT_MS,
            quota_state_path: None,
        }
    }
}

/// Plaza geográfica y vertical objetivo para cacería OSINT autónoma (Carril Verde AFK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlazaOsintProgramada {
    /// Ciudad objetivo en México (ej. `"Monterrey"`).
    pub ciudad: String,
    /// Entidad federativa (ej. `"Nuevo León"`).
    pub estado: String,
    /// Zona comercial o industrial opcional.
    #[serde(default)]
    pub zona: Option<String>,
    /// Vertical comercial objetivo.
    pub vertical: VerticalComercial,
    /// Palabras clave públicas de intención.
    #[serde(default)]
    pub palabras_clave: Vec<String>,
    /// Máximo de registros públicos por corrida.
    #[serde(default)]
    pub limite_resultados: Option<usize>,
}

/// Configuración de la sección `[scheduler]` para operación AFK 24/7 (Carril Verde).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigSchedulerAfk {
    /// Activa cacerías OSINT programadas sobre `plazas_osint` (`false` por defecto para cero tráfico accidental).
    pub hunt_programado_habilitado: bool,
    /// Intervalo en minutos entre cacerías OSINT programadas (`360` por defecto).
    pub hunt_intervalo_minutos: u64,
    /// Plazas y verticales a monitorear cuando `hunt_programado_habilitado = true`.
    pub plazas_osint: Vec<PlazaOsintProgramada>,
    /// Activa revisión periódica de cola de publicación orgánica (Carril Verde sin PII).
    pub publicacion_organica_habilitada: bool,
    /// Intervalo en minutos entre revisiones de la cola de publicación orgánica (`120` por defecto).
    pub organico_intervalo_minutos: u64,
}

impl Default for ConfigSchedulerAfk {
    fn default() -> Self {
        Self {
            hunt_programado_habilitado: false,
            hunt_intervalo_minutos: 360,
            plazas_osint: Vec::new(),
            publicacion_organica_habilitada: false,
            organico_intervalo_minutos: 120,
        }
    }
}

/// Configuración de la sección `[factory]` en `aurora.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFactoryDaemon {
    /// Directorio NVMe de la biblioteca B-Roll e índice Higgsfield (`<data_dir>/assets/broll` por defecto).
    pub broll_dir: Option<PathBuf>,
    /// Clave API de Higgsfield (`AURORA_HIGGSFIELD_API_KEY`); si se omite usa cola manual local.
    pub higgsfield_api_key: Option<String>,
    /// Endpoint HTTP de Higgsfield (`AURORA_HIGGSFIELD_API_URL`).
    pub higgsfield_api_url: Option<String>,
}

/// Configuración de la sección `[forge]` en `aurora.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigForgeDaemon {
    /// Habilita o deshabilita la ejecución de órdenes autónomas en worktrees Git.
    pub habilitado: bool,
    /// Directorio donde se montan los worktrees temporales (`<data_dir>/forge_worktrees` si se omite).
    pub worktrees_dir: Option<PathBuf>,
    /// Raíz del repositorio Git objetivo (si se omite, usa el directorio de trabajo actual).
    pub repo_root: Option<PathBuf>,
    /// Timeout máximo por etapa en segundos.
    pub timeout_segundos: u64,
}

impl Default for ConfigForgeDaemon {
    fn default() -> Self {
        Self {
            habilitado: true,
            worktrees_dir: None,
            repo_root: None,
            timeout_segundos: 300,
        }
    }
}

/// Configuración unificada de `aurora-daemon` (compatible con TOML + overrides `AURORA_*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuroraDaemonConfig {
    /// Dirección IP y puerto de escucha (`127.0.0.1:8090` por defecto).
    pub bind_addr: String,
    /// Endpoint local de BOB `llama-server` (`http://127.0.0.1:8085` por defecto).
    pub bob_url: String,
    /// Ranura única asignada a Aurora en la GPU RTX 5060 Ti (`0`).
    pub bob_slot_id: i32,
    /// Directorio raíz en NVMe (`~/.local/share/aurora` por defecto en Arch Linux).
    pub data_dir: PathBuf,
    /// Compatibilidad con bandera booleana `AURORA_OFFLINE_MOCK`; cuando es `true` fuerza `SimuladoOffline`.
    pub modo_offline: bool,
    /// Modo de operación de adaptadores reales vs fallback determinista.
    pub modo_ejecucion: ModoEjecucion,
    /// Directorio explícito del WAL/snapshot 4D (si se omite, `<data_dir>/wal`).
    pub wal_dir: Option<PathBuf>,
    /// Directorio explícito de artefactos PDF/WAV/MP4/SRT (si se omite, `<data_dir>/artefactos`).
    pub artefactos_dir: Option<PathBuf>,
    /// Directorio explícito de datasets DPO JSONL (si se omite, `<data_dir>/dpo`).
    pub dpo_dir: Option<PathBuf>,
    /// Nombre o ruta al binario `piper` en CPU (`piper` o `~/.local/bin/piper`).
    pub piper_bin: PathBuf,
    /// Ruta al modelo `.onnx` en español mexicano (ej. `es_MX-claude-high.onnx`).
    pub piper_model: PathBuf,
    /// Speaker ID opcional para modelos Piper multi-voz.
    pub piper_speaker_id: Option<u32>,
    /// Nombre o ruta al navegador headless (`chromium`, `google-chrome-stable`, etc.).
    pub chromium_bin: PathBuf,
    /// Endpoint Chrome DevTools Protocol para hidratación JS de SPAs (`http://127.0.0.1:9222`).
    pub cdp_url: String,
    /// Binario `ffmpeg` del sistema (`ffmpeg` por defecto).
    pub ffmpeg_bin: PathBuf,
    /// Prioriza codificación hardware `h264_nvenc` (RTX 5060 Ti) con respaldo `libx264` CPU.
    pub usar_nvenc: bool,
    /// Token oficial de la API pública DENUE / INEGI (`AURORA_INEGI_TOKEN`); opcional.
    pub inegi_token: Option<String>,
    /// Habilita el adaptador web público mexicano (`WebPublicaMxAdapter`) sin API key.
    pub osint_web_publica_habilitada: bool,
    /// URLs públicas opcionales personalizadas para `WebPublicaMxAdapter`.
    pub osint_web_publica_urls: Vec<String>,
    /// Activa el scheduler circadiano autónomo DPO en segundo plano.
    pub dpo_scheduler_habilitado: bool,
    /// Hora UTC preferida para anclar ciclos DPO (`0..=23`, por defecto `9` = `03:00 CST`).
    pub dpo_hora_utc: u8,
    /// Intervalo entre ciclos DPO en minutos (por defecto `360` = cada 6 horas).
    pub dpo_intervalo_minutos: u64,
    /// Token opcional `Authorization: Bearer <token>` (`AURORA_MOBILE_TOKEN`) para proteger `/api/v1/*`
    /// cuando `bind_addr` escucha en interfaz Tailscale (`100.x.y.z:8090`).
    pub mobile_token: Option<String>,
    /// Token OAuth YouTube Data API v3 (`AURORA_YOUTUBE_ACCESS_TOKEN`).
    pub youtube_access_token: Option<String>,
    /// Usuario IG Business para Meta Graph API (`AURORA_META_IG_USER_ID`).
    pub meta_ig_user_id: Option<String>,
    /// Token de acceso Meta Graph API (`AURORA_META_ACCESS_TOKEN`).
    pub meta_access_token: Option<String>,
    /// URL pública HTTPS base para entrega de `video_url` a Meta Graph (`AURORA_META_MEDIA_BASE_URL`).
    pub meta_media_base_url: Option<String>,
    /// Token OAuth 2.0 de TikTok Content Posting API v2 (`AURORA_TIKTOK_ACCESS_TOKEN`).
    pub tiktok_access_token: Option<String>,
    /// Modo de publicación TikTok: `"inbox"` (borrador seguro) o `"direct_post"` (`AURORA_TIKTOK_MODE`).
    pub tiktok_modo_publicacion: Option<String>,
    /// Configuración SMTP (`lettre` + `tokio1-rustls-tls`) y cumplimiento LFPDPPP `List-Unsubscribe`.
    pub smtp: ConfigSmtp,
    /// Configuración del Consejo Multi-Agente (`aurora-council`) y límites de cuotas NVMe.
    pub council: ConfigCouncilDaemon,
    /// Configuración de la fábrica multimodal y biblioteca B-Roll (`[factory]`).
    pub factory: ConfigFactoryDaemon,
    /// Configuración del sandbox Git aislado (`aurora-forge`, Carril Ámbar).
    pub forge: ConfigForgeDaemon,
    /// Configuración del orquestador circadiano AFK 24/7 (`[scheduler]`).
    pub scheduler: ConfigSchedulerAfk,
}

impl Default for AuroraDaemonConfig {
    fn default() -> Self {
        let base_xdg = directorio_datos_xdg_por_defecto();
        Self {
            bind_addr: "127.0.0.1:8090".to_string(),
            bob_url: aurora_cortex::DEFAULT_BOB_URL.to_string(),
            bob_slot_id: 0,
            data_dir: base_xdg.clone(),
            modo_offline: false,
            modo_ejecucion: ModoEjecucion::RealConFallback,
            wal_dir: None,
            artefactos_dir: None,
            dpo_dir: None,
            piper_bin: PathBuf::from("piper"),
            piper_model: base_xdg.join("models/tts/es_MX-ald-medium.onnx"),
            piper_speaker_id: None,
            chromium_bin: PathBuf::from("chromium"),
            cdp_url: "http://127.0.0.1:9222".to_string(),
            ffmpeg_bin: PathBuf::from("ffmpeg"),
            usar_nvenc: true,
            inegi_token: None,
            osint_web_publica_habilitada: true,
            osint_web_publica_urls: Vec::new(),
            dpo_scheduler_habilitado: true,
            dpo_hora_utc: 9,
            dpo_intervalo_minutos: 360,
            mobile_token: None,
            youtube_access_token: None,
            meta_ig_user_id: None,
            meta_access_token: None,
            meta_media_base_url: None,
            tiktok_access_token: None,
            tiktok_modo_publicacion: None,
            smtp: ConfigSmtp::default(),
            council: ConfigCouncilDaemon::default(),
            factory: ConfigFactoryDaemon::default(),
            forge: ConfigForgeDaemon::default(),
            scheduler: ConfigSchedulerAfk::default(),
        }
    }
}

impl AuroraDaemonConfig {
    /// Carga configuración desde archivo TOML (`AURORA_CONFIG_PATH`, `./aurora.toml` o
    /// `~/.config/aurora/aurora.toml`) y aplica variables de entorno `AURORA_*`.
    pub fn cargar_desde_toml_y_entorno() -> DaemonResult<Self> {
        Self::cargar_con_lector_entorno(None, |clave| std::env::var(clave).ok())
    }

    /// Variante determinista (ideal para tests unitarios e integración) sin mutar variables globales.
    pub fn cargar_con_lector_entorno<F>(ruta_toml: Option<&Path>, leer_env: F) -> DaemonResult<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();

        let ruta_efectiva = ruta_toml.map(PathBuf::from).or_else(|| {
            if let Some(p) = leer_env("AURORA_CONFIG_PATH").filter(|s| !s.trim().is_empty()) {
                return Some(expandir_tilde_path(Path::new(p.trim())));
            }
            let local = PathBuf::from("aurora.toml");
            if local.is_file() {
                return Some(local);
            }
            let home_cfg = directorio_config_xdg_por_defecto().join("aurora.toml");
            if home_cfg.is_file() {
                return Some(home_cfg);
            }
            None
        });

        if let Some(ref ruta) = ruta_efectiva {
            let contenido = fs::read_to_string(ruta).map_err(|e| {
                DaemonError::Configuracion(format!(
                    "no se pudo leer configuración TOML '{}': {e}",
                    ruta.display()
                ))
            })?;
            cfg = toml::from_str::<Self>(&contenido).map_err(|e| {
                DaemonError::Configuracion(format!(
                    "TOML inválido en '{}': {e}",
                    ruta.display()
                ))
            })?;
        }

        cfg.aplicar_overrides_entorno(&leer_env)?;
        cfg.normalizar_y_validar()?;
        Ok(cfg)
    }

    /// Devuelve el modo efectivo respetando tanto `modo_offline` legacy como `modo_ejecucion`.
    #[must_use]
    pub const fn modo_efectivo(&self) -> ModoEjecucion {
        if self.modo_offline {
            ModoEjecucion::SimuladoOffline
        } else {
            self.modo_ejecucion
        }
    }

    /// Directorio raíz expandiendo `~/` al `$HOME` actual.
    #[must_use]
    pub fn data_dir_resuelto(&self) -> PathBuf {
        expandir_tilde_path(&self.data_dir)
    }

    /// Directorio efectivo del WAL 4D (`wal_dir` explícito o `<data_dir>/wal`).
    #[must_use]
    pub fn wal_dir_efectivo(&self) -> PathBuf {
        self.wal_dir
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("wal"))
    }

    /// Directorio efectivo de artefactos multimedia (`artefactos_dir` o `<data_dir>/artefactos`).
    #[must_use]
    pub fn artefactos_dir_efectivo(&self) -> PathBuf {
        self.artefactos_dir
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("artefactos"))
    }

    /// Directorio efectivo de la biblioteca B-Roll (`[factory].broll_dir` o `<data_dir>/assets/broll`).
    #[must_use]
    pub fn broll_dir_efectivo(&self) -> PathBuf {
        self.factory
            .broll_dir
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("assets/broll"))
    }

    /// Directorio efectivo de datasets DPO (`dpo_dir` o `<data_dir>/dpo`).
    #[must_use]
    pub fn dpo_dir_efectivo(&self) -> PathBuf {
        self.dpo_dir
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("dpo"))
    }

    /// Ruta del archivo de respaldo persistente de la cola Gatekeeper.
    #[must_use]
    pub fn queue_path_efectivo(&self) -> PathBuf {
        self.data_dir_resuelto().join("gatekeeper_queue.json")
    }

    /// Ruta efectiva del archivo JSON de estado de cuotas del Consejo (`quota_state.json`).
    #[must_use]
    pub fn council_quota_state_efectivo(&self) -> PathBuf {
        self.council
            .quota_state_path
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("quota_state.json"))
    }

    /// Construye la configuración de cuotas de [`aurora_council::QuotaGovernor`] desde `[council]`.
    #[must_use]
    pub fn construir_config_cuotas_consejo(&self) -> ConfigCuotasConsejo {
        let cd = self.council.cooldown_rate_limit_segundos;
        ConfigCuotasConsejo::con_ruta(self.council_quota_state_efectivo())
            .con_politica(
                MiembroConsejo::ClaudePro,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: self.council.claude_max_5h,
                    max_tareas_diarias: self.council.claude_max_diario,
                    cooldown_rate_limit_segundos: cd,
                    ilimitado: false,
                },
            )
            .con_politica(
                MiembroConsejo::CodexPlus,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: self.council.codex_max_5h,
                    max_tareas_diarias: self.council.codex_max_diario,
                    cooldown_rate_limit_segundos: cd,
                    ilimitado: false,
                },
            )
            .con_politica(
                MiembroConsejo::GeminiEstudiante,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: self.council.gemini_max_5h,
                    max_tareas_diarias: self.council.gemini_max_diario,
                    cooldown_rate_limit_segundos: cd,
                    ilimitado: false,
                },
            )
            .con_politica(
                MiembroConsejo::OpenCodeLocal,
                PoliticaCuotaProveedor {
                    max_tareas_ventana_5h: self.council.opencode_max_5h,
                    max_tareas_diarias: self.council.opencode_max_diario,
                    cooldown_rate_limit_segundos: cd,
                    ilimitado: false,
                },
            )
    }

    /// Directorio temporal efectivo donde `aurora-forge` monta los `git worktree`.
    #[must_use]
    pub fn forge_worktrees_dir_efectivo(&self) -> PathBuf {
        self.forge
            .worktrees_dir
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| self.data_dir_resuelto().join("forge_worktrees"))
    }

    /// Raíz Git objetivo para `aurora-forge`.
    #[must_use]
    pub fn forge_repo_root_efectivo(&self) -> PathBuf {
        self.forge
            .repo_root
            .as_ref()
            .map(|p| expandir_tilde_path(p))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| self.data_dir_resuelto())
            })
    }

    /// Ruta del archivo de historial persistente de órdenes de `aurora-forge`.
    #[must_use]
    pub fn forge_ordenes_path_efectivo(&self) -> PathBuf {
        self.data_dir_resuelto().join("forge_ordenes.json")
    }

    /// Ruta del archivo JSON persistente de catalogación de probabilidad de compra en `<wal_dir>`.
    #[must_use]
    pub fn catalogacion_path_efectivo(&self) -> PathBuf {
        self.wal_dir_efectivo().join("catalogacion_compra.json")
    }

    /// Construye la configuración de [`aurora_factory::ApiHiggsfieldClient`] desde `[factory]`.
    #[must_use]
    pub fn config_higgsfield(&self) -> ConfigHiggsfield {
        ConfigHiggsfield {
            api_key: self.factory.higgsfield_api_key.clone(),
            api_url: self
                .factory
                .higgsfield_api_url
                .clone()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_HIGGSFIELD_API_URL.to_string()),
            ..ConfigHiggsfield::default()
        }
    }

    /// Indica si `bind_addr` escucha fuera de loopback local (ej. interfaz Tailscale `100.x.y.z:8090`).
    #[must_use]
    pub fn es_bind_remoto_o_tailscale(&self) -> bool {
        let host = self.bind_addr.trim().to_ascii_lowercase();
        !(host.starts_with("127.") || host.starts_with("localhost:") || host.starts_with("[::1]:"))
    }

    /// Devuelve el token Bearer efectivo cuando la autenticación móvil/Tailscale está activa.
    #[must_use]
    pub fn mobile_token_activo(&self) -> Option<&str> {
        self.mobile_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }

    /// Construye la configuración de [`aurora_pulse::YouTubeShortsApiPublisher`].
    #[must_use]
    pub fn config_youtube_shorts(&self) -> ConfigYouTubeShorts {
        ConfigYouTubeShorts {
            access_token: self.youtube_access_token.clone(),
            ..ConfigYouTubeShorts::default()
        }
    }

    /// Construye la configuración de [`aurora_pulse::MetaReelsGraphPublisher`].
    #[must_use]
    pub fn config_meta_reels(&self) -> ConfigMetaReels {
        ConfigMetaReels {
            ig_user_id: self.meta_ig_user_id.clone(),
            access_token: self.meta_access_token.clone(),
            media_public_base_url: self.meta_media_base_url.clone(),
            ..ConfigMetaReels::default()
        }
    }

    /// Construye la configuración de [`aurora_pulse::TikTokApiPublisher`] (Content Posting API v2).
    #[must_use]
    pub fn config_tiktok(&self) -> ConfigTikTok {
        let modo = self
            .tiktok_modo_publicacion
            .as_deref()
            .map(ModoPublicacionTikTok::desde_str)
            .unwrap_or_default();
        ConfigTikTok {
            access_token: self.tiktok_access_token.clone(),
            modo_publicacion: modo,
            ..ConfigTikTok::default()
        }
    }

    /// Ruta resuelta del modelo Piper `.onnx`.
    #[must_use]
    pub fn piper_model_resuelto(&self) -> PathBuf {
        expandir_tilde_path(&self.piper_model)
    }

    /// Resuelve el binario Chromium/Chrome disponible en Arch Linux.
    #[must_use]
    pub fn chromium_bin_efectivo(&self) -> PathBuf {
        let cfg = expandir_tilde_path(&self.chromium_bin);
        let candidatos = candidatos_chromium(&cfg);
        resolver_binario_en_sistema(&cfg, &candidatos).unwrap_or(cfg)
    }

    /// Resuelve el binario Piper disponible en sistema (`piper`, `piper-tts`, `~/.local/bin/piper`, `/opt/piper/piper`).
    #[must_use]
    pub fn piper_bin_efectivo(&self) -> PathBuf {
        let cfg = expandir_tilde_path(&self.piper_bin);
        let candidatos = candidatos_piper(&cfg);
        resolver_binario_en_sistema(&cfg, &candidatos).unwrap_or(cfg)
    }

    /// Resuelve el binario `ffmpeg` disponible en el sistema.
    #[must_use]
    pub fn ffmpeg_bin_efectivo(&self) -> PathBuf {
        let cfg = expandir_tilde_path(&self.ffmpeg_bin);
        resolver_binario_en_sistema(&cfg, &[]).unwrap_or(cfg)
    }

    fn aplicar_overrides_entorno<F>(&mut self, leer_env: &F) -> DaemonResult<()>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(v) = leer_env("AURORA_BIND_ADDR").filter(|s| !s.trim().is_empty()) {
            self.bind_addr = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_BOB_URL").filter(|s| !s.trim().is_empty()) {
            self.bob_url = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_BOB_SLOT_ID").filter(|s| !s.trim().is_empty()) {
            self.bob_slot_id = v.trim().parse::<i32>().map_err(|e| {
                DaemonError::Configuracion(format!("AURORA_BOB_SLOT_ID inválido ('{v}'): {e}"))
            })?;
        }
        if let Some(v) = leer_env("AURORA_DATA_DIR").filter(|s| !s.trim().is_empty()) {
            self.data_dir = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_WAL_DIR").filter(|s| !s.trim().is_empty()) {
            self.wal_dir = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_ARTEFACTOS_DIR").filter(|s| !s.trim().is_empty()) {
            self.artefactos_dir = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_BROLL_DIR").filter(|s| !s.trim().is_empty()) {
            self.factory.broll_dir = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_HIGGSFIELD_API_KEY") {
            let limpio = v.trim().to_string();
            self.factory.higgsfield_api_key = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_HIGGSFIELD_API_URL") {
            let limpio = v.trim().to_string();
            self.factory.higgsfield_api_url = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_DPO_DIR").filter(|s| !s.trim().is_empty()) {
            self.dpo_dir = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_PIPER_BIN").filter(|s| !s.trim().is_empty()) {
            self.piper_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_PIPER_MODEL").filter(|s| !s.trim().is_empty()) {
            self.piper_model = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_PIPER_SPEAKER_ID").filter(|s| !s.trim().is_empty()) {
            self.piper_speaker_id = Some(v.trim().parse::<u32>().map_err(|e| {
                DaemonError::Configuracion(format!(
                    "AURORA_PIPER_SPEAKER_ID inválido ('{v}'): {e}"
                ))
            })?);
        }
        if let Some(v) = leer_env("AURORA_CHROMIUM_BIN").filter(|s| !s.trim().is_empty()) {
            self.chromium_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_CDP_URL").filter(|s| !s.trim().is_empty()) {
            self.cdp_url = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_FFMPEG_BIN").filter(|s| !s.trim().is_empty()) {
            self.ffmpeg_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_USAR_NVENC").filter(|s| !s.trim().is_empty()) {
            self.usar_nvenc = parsear_bool_env("AURORA_USAR_NVENC", &v)?;
        }
        if let Some(v) = leer_env("AURORA_INEGI_TOKEN") {
            let limpio = v.trim().to_string();
            self.inegi_token = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_OSINT_WEB_PUBLICA").filter(|s| !s.trim().is_empty()) {
            self.osint_web_publica_habilitada = parsear_bool_env("AURORA_OSINT_WEB_PUBLICA", &v)?;
        }
        if let Some(v) =
            leer_env("AURORA_DPO_SCHEDULER_HABILITADO").filter(|s| !s.trim().is_empty())
        {
            self.dpo_scheduler_habilitado =
                parsear_bool_env("AURORA_DPO_SCHEDULER_HABILITADO", &v)?;
        }
        if let Some(v) = leer_env("AURORA_DPO_HORA_UTC").filter(|s| !s.trim().is_empty()) {
            self.dpo_hora_utc = v.trim().parse::<u8>().map_err(|e| {
                DaemonError::Configuracion(format!("AURORA_DPO_HORA_UTC inválido ('{v}'): {e}"))
            })?;
        }
        if let Some(v) = leer_env("AURORA_DPO_INTERVALO_MINUTOS").filter(|s| !s.trim().is_empty()) {
            self.dpo_intervalo_minutos = v.trim().parse::<u64>().map_err(|e| {
                DaemonError::Configuracion(format!(
                    "AURORA_DPO_INTERVALO_MINUTOS inválido ('{v}'): {e}"
                ))
            })?;
        }
        if let Some(v) = leer_env("AURORA_MODO_EJECUCION").filter(|s| !s.trim().is_empty()) {
            let parsed = ModoEjecucion::desde_str(&v).ok_or_else(|| {
                DaemonError::Configuracion(format!(
                    "AURORA_MODO_EJECUCION inválido '{v}' (usa real_con_fallback | estricto_real | simulado_offline)"
                ))
            })?;
            self.modo_ejecucion = parsed;
            self.modo_offline = matches!(parsed, ModoEjecucion::SimuladoOffline);
        }
        if let Some(v) = leer_env("AURORA_OFFLINE_MOCK").filter(|s| !s.trim().is_empty()) {
            let offline = parsear_bool_env("AURORA_OFFLINE_MOCK", &v)?;
            self.modo_offline = offline;
            if offline {
                self.modo_ejecucion = ModoEjecucion::SimuladoOffline;
            }
        }

        // Overrides SMTP
        if let Some(v) = leer_env("AURORA_SMTP_HABILITADO").filter(|s| !s.trim().is_empty()) {
            self.smtp.habilitado = parsear_bool_env("AURORA_SMTP_HABILITADO", &v)?;
        }
        if let Some(v) = leer_env("AURORA_SMTP_HOST").filter(|s| !s.trim().is_empty()) {
            self.smtp.host = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_SMTP_PUERTO").filter(|s| !s.trim().is_empty()) {
            self.smtp.puerto = v.trim().parse::<u16>().map_err(|e| {
                DaemonError::Configuracion(format!("AURORA_SMTP_PUERTO inválido ('{v}'): {e}"))
            })?;
        }
        if let Some(v) = leer_env("AURORA_SMTP_USUARIO") {
            let limpio = v.trim().to_string();
            self.smtp.usuario = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_SMTP_PASSWORD") {
            self.smtp.password = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = leer_env("AURORA_SMTP_REMITENTE_NOMBRE").filter(|s| !s.trim().is_empty())
        {
            self.smtp.remitente_nombre = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_SMTP_REMITENTE_CORREO").filter(|s| !s.trim().is_empty())
        {
            self.smtp.remitente_correo = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_SMTP_OPT_OUT_URL").filter(|s| !s.trim().is_empty()) {
            self.smtp.url_base_opt_out = v.trim().to_string();
        }
        if let Some(v) = leer_env("AURORA_SMTP_STARTTLS").filter(|s| !s.trim().is_empty()) {
            self.smtp.usar_tls_starttls = parsear_bool_env("AURORA_SMTP_STARTTLS", &v)?;
        }
        if let Some(v) = leer_env("AURORA_SMTP_TLS_IMPLICITO").filter(|s| !s.trim().is_empty()) {
            self.smtp.usar_tls_implicito = parsear_bool_env("AURORA_SMTP_TLS_IMPLICITO", &v)?;
        }

        // Overrides [council]
        if let Some(v) = leer_env("AURORA_CLAUDE_BIN").filter(|s| !s.trim().is_empty()) {
            self.council.claude_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_CODEX_BIN").filter(|s| !s.trim().is_empty()) {
            self.council.codex_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_GEMINI_BIN").filter(|s| !s.trim().is_empty()) {
            self.council.gemini_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_OPENCODE_BIN").filter(|s| !s.trim().is_empty()) {
            self.council.opencode_bin = PathBuf::from(v.trim());
        }
        if let Some(v) = leer_env("AURORA_COUNCIL_QUOTA_PATH").filter(|s| !s.trim().is_empty()) {
            self.council.quota_state_path = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_COUNCIL_TIMEOUT_MS").filter(|s| !s.trim().is_empty()) {
            self.council.timeout_ms = v.trim().parse::<u64>().map_err(|e| {
                DaemonError::Configuracion(format!(
                    "AURORA_COUNCIL_TIMEOUT_MS inválido ('{v}'): {e}"
                ))
            })?;
        }

        // Overrides [forge]
        if let Some(v) = leer_env("AURORA_FORGE_HABILITADO").filter(|s| !s.trim().is_empty()) {
            self.forge.habilitado = parsear_bool_env("AURORA_FORGE_HABILITADO", &v)?;
        }
        if let Some(v) = leer_env("AURORA_FORGE_WORKTREES_DIR").filter(|s| !s.trim().is_empty()) {
            self.forge.worktrees_dir = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) = leer_env("AURORA_FORGE_REPO_ROOT").filter(|s| !s.trim().is_empty()) {
            self.forge.repo_root = Some(PathBuf::from(v.trim()));
        }
        if let Some(v) =
            leer_env("AURORA_FORGE_TIMEOUT_SEGUNDOS").filter(|s| !s.trim().is_empty())
        {
            self.forge.timeout_segundos = v.trim().parse::<u64>().map_err(|e| {
                DaemonError::Configuracion(format!(
                    "AURORA_FORGE_TIMEOUT_SEGUNDOS inválido ('{v}'): {e}"
                ))
            })?;
        }

        // Overrides portal móvil Tailscale, APIs sociales orgánicas y [scheduler]
        if let Some(v) = leer_env("AURORA_MOBILE_TOKEN") {
            let limpio = v.trim().to_string();
            self.mobile_token = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_YOUTUBE_ACCESS_TOKEN") {
            let limpio = v.trim().to_string();
            self.youtube_access_token = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_META_IG_USER_ID") {
            let limpio = v.trim().to_string();
            self.meta_ig_user_id = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_META_ACCESS_TOKEN") {
            let limpio = v.trim().to_string();
            self.meta_access_token = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_META_MEDIA_BASE_URL") {
            let limpio = v.trim().trim_end_matches('/').to_string();
            self.meta_media_base_url = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_TIKTOK_ACCESS_TOKEN") {
            let limpio = v.trim().to_string();
            self.tiktok_access_token = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) = leer_env("AURORA_TIKTOK_MODE") {
            let limpio = v.trim().to_string();
            self.tiktok_modo_publicacion = if limpio.is_empty() { None } else { Some(limpio) };
        }
        if let Some(v) =
            leer_env("AURORA_SCHEDULER_HUNT_HABILITADO").filter(|s| !s.trim().is_empty())
        {
            self.scheduler.hunt_programado_habilitado =
                parsear_bool_env("AURORA_SCHEDULER_HUNT_HABILITADO", &v)?;
        }
        if let Some(v) =
            leer_env("AURORA_SCHEDULER_ORGANICO_HABILITADO").filter(|s| !s.trim().is_empty())
        {
            self.scheduler.publicacion_organica_habilitada =
                parsear_bool_env("AURORA_SCHEDULER_ORGANICO_HABILITADO", &v)?;
        }

        Ok(())
    }

    fn normalizar_y_validar(&mut self) -> DaemonResult<()> {
        if self.bind_addr.trim().is_empty() {
            return Err(DaemonError::Configuracion(
                "bind_addr no puede estar vacío".to_string(),
            ));
        }
        let bob = self.bob_url.trim();
        if !(bob.starts_with("http://") || bob.starts_with("https://")) {
            return Err(DaemonError::Configuracion(format!(
                "bob_url '{}' debe iniciar con http:// o https://",
                self.bob_url
            )));
        }
        if self.bob_slot_id != 0 {
            return Err(DaemonError::Configuracion(format!(
                "bob_slot_id={} viola exclusividad VRAM RTX 5060 Ti (debe ser 0)",
                self.bob_slot_id
            )));
        }
        let cdp = self.cdp_url.trim();
        if !(cdp.starts_with("http://") || cdp.starts_with("https://")) {
            return Err(DaemonError::Configuracion(format!(
                "cdp_url '{}' debe iniciar con http:// o https://",
                self.cdp_url
            )));
        }
        if self.dpo_hora_utc > 23 {
            return Err(DaemonError::Configuracion(format!(
                "dpo_hora_utc={} fuera de rango válido (0..=23)",
                self.dpo_hora_utc
            )));
        }
        if self.dpo_scheduler_habilitado && self.dpo_intervalo_minutos == 0 {
            return Err(DaemonError::Configuracion(
                "dpo_intervalo_minutos debe ser >= 1 cuando dpo_scheduler_habilitado = true"
                    .to_string(),
            ));
        }
        if let Some(ref t) = self.inegi_token {
            if t.trim().is_empty() {
                self.inegi_token = None;
            }
        }
        if self.modo_offline {
            self.modo_ejecucion = ModoEjecucion::SimuladoOffline;
        }
        if self.council.timeout_ms < 1_000 {
            return Err(DaemonError::Configuracion(format!(
                "council.timeout_ms={} debe ser >= 1000 ms",
                self.council.timeout_ms
            )));
        }
        if self.forge.habilitado && self.forge.timeout_segundos < 5 {
            return Err(DaemonError::Configuracion(format!(
                "forge.timeout_segundos={} debe ser >= 5 s",
                self.forge.timeout_segundos
            )));
        }
        if self.scheduler.hunt_programado_habilitado && self.scheduler.hunt_intervalo_minutos == 0 {
            return Err(DaemonError::Configuracion(
                "scheduler.hunt_intervalo_minutos debe ser >= 1 cuando hunt_programado_habilitado = true".to_string(),
            ));
        }
        if self.scheduler.publicacion_organica_habilitada
            && self.scheduler.organico_intervalo_minutos == 0
        {
            return Err(DaemonError::Configuracion(
                "scheduler.organico_intervalo_minutos debe ser >= 1 cuando publicacion_organica_habilitada = true".to_string(),
            ));
        }
        Ok(())
    }
}

/// Estado detallado de un subsistema de hardware/red sondeado al arranque o bajo `/health`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EstadoSubsistema {
    /// Identificador legible del subsistema.
    pub nombre: String,
    /// `true` si el subsistema está listo para atender llamadas (real o simulado offline).
    pub disponible: bool,
    /// `true` cuando opera sobre el binario/servicio real del sistema (no mock offline).
    pub modo_real_activo: bool,
    /// Resumen técnico breve (versión, codec NVENC, ausencia de modelo `.onnx`, etc.).
    pub detalle: String,
    /// Ruta de binario en sistema o URL local sondeada.
    pub ruta_o_endpoint: String,
}

/// Reporte consolidado de los 6 subsistemas nativos de Aurora v2 en Arch Linux.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReporteSubsistemas {
    /// Inferencia cognitiva local (`llama-server` slot 0 en `127.0.0.1:8085`).
    pub bob_llm: EstadoSubsistema,
    /// Locución neuronal en CPU (`piper` + modelo `.onnx` es_MX).
    pub piper_tts: EstadoSubsistema,
    /// Maquetación PDF y frames PNG en CPU headless (`chromium`/`google-chrome-stable`).
    pub chromium_headless: EstadoSubsistema,
    /// Ensamblaje Reel 1080x1920 (`ffmpeg` con aceleración NVENC o fallback `libx264`).
    pub ffmpeg_nvenc: EstadoSubsistema,
    /// Navegador Chrome DevTools Protocol WebSocket (`http://127.0.0.1:9222`) para SPAs.
    pub cdp_browser: EstadoSubsistema,
    /// Transporte ejecutivo SMTP (`rustls-tls` + cabecera LFPDPPP `List-Unsubscribe`).
    pub smtp_transport: EstadoSubsistema,
    /// Disponibilidad sondeada de los CLIs del Consejo Multi-Agente (`claude`, `codex`, `gemini`, `opencode`).
    #[serde(default)]
    pub consejo_clis: Vec<EstadoSubsistema>,
}

impl ReporteSubsistemas {
    /// Resumen compacto en una sola línea para bitácora de arranque.
    #[must_use]
    pub fn resumen_log(&self) -> String {
        let mark = |st: &EstadoSubsistema| {
            if st.modo_real_activo {
                "REAL"
            } else if st.disponible {
                "MOCK"
            } else {
                "FALLBACK"
            }
        };
        let clis_reales = self
            .consejo_clis
            .iter()
            .filter(|st| st.modo_real_activo)
            .count();
        format!(
            "bob_llm={}, piper_tts={}, chromium_headless={}, ffmpeg_nvenc={}, cdp_browser={}, smtp_transport={}, consejo_clis_reales={}/{}",
            mark(&self.bob_llm),
            mark(&self.piper_tts),
            mark(&self.chromium_headless),
            mark(&self.ffmpeg_nvenc),
            mark(&self.cdp_browser),
            mark(&self.smtp_transport),
            clis_reales,
            self.consejo_clis.len(),
        )
    }

    /// Devuelve todos los nombres de subsistemas críticos que no están listos en modo real.
    #[must_use]
    pub fn faltantes_en_modo_estricto(&self) -> Vec<String> {
        let mut faltantes = Vec::new();
        for st in [
            &self.bob_llm,
            &self.piper_tts,
            &self.chromium_headless,
            &self.ffmpeg_nvenc,
        ] {
            if !st.modo_real_activo {
                faltantes.push(format!("{} ({})", st.nombre, st.detalle));
            }
        }
        faltantes
    }
}

/// Sonda asíncrona ligera de binarios en sistema, NVENC, CDP, SMTP y BOB HTTP `/health`.
pub struct SubsystemProbe;

impl SubsystemProbe {
    /// Sonda todos los subsistemas relevantes respetando `config.modo_efectivo()` y timeouts cortos.
    pub async fn ejecutar(
        config: &AuroraDaemonConfig,
        http_client: &Client,
    ) -> ReporteSubsistemas {
        let es_offline = matches!(config.modo_efectivo(), ModoEjecucion::SimuladoOffline);

        let (bob_llm, cdp_browser, ffmpeg_nvenc) = tokio::join!(
            Self::sondear_bob(config, http_client, es_offline),
            Self::sondear_cdp(config, es_offline),
            Self::sondear_ffmpeg(config, es_offline),
        );

        let piper_tts = Self::sondear_piper(config, es_offline);
        let chromium_headless = Self::sondear_chromium(config, es_offline);
        let smtp_transport = Self::sondear_smtp(config, es_offline);
        let consejo_clis = Self::sondear_consejo_clis(config, es_offline);

        ReporteSubsistemas {
            bob_llm,
            piper_tts,
            chromium_headless,
            ffmpeg_nvenc,
            cdp_browser,
            smtp_transport,
            consejo_clis,
        }
    }

    fn sondear_consejo_clis(
        config: &AuroraDaemonConfig,
        es_offline: bool,
    ) -> Vec<EstadoSubsistema> {
        let entradas: &[(&str, &Path)] = &[
            ("Consejo CLI: Claude Code", &config.council.claude_bin),
            ("Consejo CLI: Codex", &config.council.codex_bin),
            ("Consejo CLI: Gemini", &config.council.gemini_bin),
            ("Consejo CLI: OpenCode", &config.council.opencode_bin),
        ];

        entradas
            .iter()
            .map(|(nombre, bin_raw)| {
                let bin_cfg = expandir_tilde_path(bin_raw);
                if es_offline {
                    return EstadoSubsistema {
                        nombre: (*nombre).to_string(),
                        disponible: true,
                        modo_real_activo: false,
                        detalle: "Driver determinista offline activo (respaldo BOB Local)"
                            .to_string(),
                        ruta_o_endpoint: bin_cfg.display().to_string(),
                    };
                }

                if let Some(bin) = resolver_binario_en_sistema(&bin_cfg, &[]) {
                    EstadoSubsistema {
                        nombre: (*nombre).to_string(),
                        disponible: true,
                        modo_real_activo: true,
                        detalle: "Binario CLI headless detectado en sistema".to_string(),
                        ruta_o_endpoint: bin.display().to_string(),
                    }
                } else {
                    EstadoSubsistema {
                        nombre: (*nombre).to_string(),
                        disponible: false,
                        modo_real_activo: false,
                        detalle: "CLI no instalado en PATH (cascada degradará a BOB Local)"
                            .to_string(),
                        ruta_o_endpoint: bin_cfg.display().to_string(),
                    }
                }
            })
            .collect()
    }

    async fn sondear_bob(
        config: &AuroraDaemonConfig,
        http_client: &Client,
        es_offline: bool,
    ) -> EstadoSubsistema {
        if es_offline {
            return EstadoSubsistema {
                nombre: "BOB Córtex (llama-server)".to_string(),
                disponible: true,
                modo_real_activo: false,
                detalle: "Mock determinista offline activo (sin VRAM)".to_string(),
                ruta_o_endpoint: config.bob_url.clone(),
            };
        }

        let health_url = format!("{}/health", config.bob_url.trim_end_matches('/'));
        match timeout(Duration::from_millis(450), http_client.get(&health_url).send()).await {
            Ok(Ok(resp)) if resp.status().is_success() => EstadoSubsistema {
                nombre: "BOB Córtex (llama-server)".to_string(),
                disponible: true,
                modo_real_activo: true,
                detalle: format!("En línea en slot único VRAM id={}", config.bob_slot_id),
                ruta_o_endpoint: health_url,
            },
            Ok(Ok(resp)) => EstadoSubsistema {
                nombre: "BOB Córtex (llama-server)".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: format!("HTTP {} en /health; usando respaldo híbrido si aplica", resp.status().as_u16()),
                ruta_o_endpoint: health_url,
            },
            _ => EstadoSubsistema {
                nombre: "BOB Córtex (llama-server)".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "Puerto 8085 sin respuesta; pendiente de llama-server".to_string(),
                ruta_o_endpoint: health_url,
            },
        }
    }

    fn sondear_piper(config: &AuroraDaemonConfig, es_offline: bool) -> EstadoSubsistema {
        let bin_cfg = expandir_tilde_path(&config.piper_bin);
        let modelo = config.piper_model_resuelto();

        if es_offline {
            return EstadoSubsistema {
                nombre: "Piper TTS CPU".to_string(),
                disponible: true,
                modo_real_activo: false,
                detalle: "Motor WAV silencioso offline activo".to_string(),
                ruta_o_endpoint: modelo.display().to_string(),
            };
        }

        let bin_resuelto = resolver_binario_en_sistema(&bin_cfg, &candidatos_piper(&bin_cfg));
        let modelo_ok = modelo.is_file();

        match (bin_resuelto, modelo_ok) {
            (Some(bin), true) => EstadoSubsistema {
                nombre: "Piper TTS CPU".to_string(),
                disponible: true,
                modo_real_activo: true,
                detalle: format!("Binario + modelo ONNX '{}' listos", modelo.display()),
                ruta_o_endpoint: bin.display().to_string(),
            },
            (Some(bin), false) => EstadoSubsistema {
                nombre: "Piper TTS CPU".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: format!(
                    "Binario hallado ({}), falta modelo ONNX '{}'",
                    bin.display(),
                    modelo.display()
                ),
                ruta_o_endpoint: modelo.display().to_string(),
            },
            (None, _) => EstadoSubsistema {
                nombre: "Piper TTS CPU".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "Binario piper no localizado en PATH ni ~/.local/bin (respaldo WAV limpio)"
                    .to_string(),
                ruta_o_endpoint: bin_cfg.display().to_string(),
            },
        }
    }

    fn sondear_chromium(config: &AuroraDaemonConfig, es_offline: bool) -> EstadoSubsistema {
        let bin_cfg = expandir_tilde_path(&config.chromium_bin);

        if es_offline {
            return EstadoSubsistema {
                nombre: "Chromium Headless CPU".to_string(),
                disponible: true,
                modo_real_activo: false,
                detalle: "Motor PDF/PNG mock determinista activo".to_string(),
                ruta_o_endpoint: bin_cfg.display().to_string(),
            };
        }

        if let Some(bin) = resolver_binario_en_sistema(&bin_cfg, &candidatos_chromium(&bin_cfg)) {
            EstadoSubsistema {
                nombre: "Chromium Headless CPU".to_string(),
                disponible: true,
                modo_real_activo: true,
                detalle: "Navegador headless CPU listo (--headless=new --disable-gpu)".to_string(),
                ruta_o_endpoint: bin.display().to_string(),
            }
        } else {
            EstadoSubsistema {
                nombre: "Chromium Headless CPU".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "No se halló chromium ni google-chrome-stable en PATH".to_string(),
                ruta_o_endpoint: bin_cfg.display().to_string(),
            }
        }
    }

    async fn sondear_ffmpeg(config: &AuroraDaemonConfig, es_offline: bool) -> EstadoSubsistema {
        let bin_cfg = expandir_tilde_path(&config.ffmpeg_bin);

        if es_offline {
            return EstadoSubsistema {
                nombre: "FFmpeg + NVENC".to_string(),
                disponible: true,
                modo_real_activo: false,
                detalle: "Compositor MP4 determinista offline activo".to_string(),
                ruta_o_endpoint: bin_cfg.display().to_string(),
            };
        }

        let Some(bin) = resolver_binario_en_sistema(&bin_cfg, &[]) else {
            return EstadoSubsistema {
                nombre: "FFmpeg + NVENC".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "Binario ffmpeg no encontrado en PATH".to_string(),
                ruta_o_endpoint: bin_cfg.display().to_string(),
            };
        };

        let mut detalle = "FFmpeg disponible (CPU libx264)".to_string();
        if config.usar_nvenc {
            if let Ok(Ok(output)) = timeout(
                Duration::from_millis(1_200),
                Command::new(&bin)
                    .args(["-hide_banner", "-encoders"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output(),
            )
            .await
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("h264_nvenc") {
                    detalle = "FFmpeg + codec hardware h264_nvenc listo".to_string();
                } else {
                    detalle = "FFmpeg listo; h264_nvenc ausente (usará fallback libx264)".to_string();
                }
            }
        }

        EstadoSubsistema {
            nombre: "FFmpeg + NVENC".to_string(),
            disponible: true,
            modo_real_activo: true,
            detalle,
            ruta_o_endpoint: bin.display().to_string(),
        }
    }

    async fn sondear_cdp(config: &AuroraDaemonConfig, es_offline: bool) -> EstadoSubsistema {
        if es_offline {
            return EstadoSubsistema {
                nombre: "Browser CDP (:9222)".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "Desactivado en simulado_offline".to_string(),
                ruta_o_endpoint: config.cdp_url.clone(),
            };
        }

        let cfg = ConfigCdpFetcher {
            cdp_http_url: config.cdp_url.clone(),
            timeout_ms: 800,
            esperar_hidratacion_ms: 300,
        };
        let online = match HttpCdpBrowserFetcher::new(cfg) {
            Ok(fetcher) => fetcher.probe_disponible().await,
            Err(_) => false,
        };

        EstadoSubsistema {
            nombre: "Browser CDP (:9222)".to_string(),
            disponible: online,
            modo_real_activo: online,
            detalle: if online {
                "Chrome DevTools WebSocket activo para SPAs dinámicas".to_string()
            } else {
                "Endpoint CDP inactivo (cacerías JS usarán HTML directo / fallback)".to_string()
            },
            ruta_o_endpoint: config.cdp_url.clone(),
        }
    }

    fn sondear_smtp(config: &AuroraDaemonConfig, es_offline: bool) -> EstadoSubsistema {
        if es_offline {
            return EstadoSubsistema {
                nombre: "SMTP TLS (rustls)".to_string(),
                disponible: true,
                modo_real_activo: false,
                detalle: "Despacho simulado offline activo".to_string(),
                ruta_o_endpoint: format!("{}:{}", config.smtp.host, config.smtp.puerto),
            };
        }

        let endpoint = format!("{}:{}", config.smtp.host, config.smtp.puerto);
        if !config.smtp.habilitado {
            return EstadoSubsistema {
                nombre: "SMTP TLS (rustls)".to_string(),
                disponible: false,
                modo_real_activo: false,
                detalle: "smtp.habilitado = false (correo ejecutivo usará simulación segura)"
                    .to_string(),
                ruta_o_endpoint: endpoint,
            };
        }

        let config_valida = !config.smtp.host.trim().is_empty()
            && config.smtp.puerto > 0
            && config.smtp.remitente_correo.contains('@')
            && (config.smtp.url_base_opt_out.starts_with("http://")
                || config.smtp.url_base_opt_out.starts_with("https://"));

        EstadoSubsistema {
            nombre: "SMTP TLS (rustls)".to_string(),
            disponible: config_valida,
            modo_real_activo: config_valida,
            detalle: if config_valida {
                let tls = if config.smtp.usar_tls_implicito {
                    "TLS implícito"
                } else if config.smtp.usar_tls_starttls {
                    "STARTTLS rustls"
                } else {
                    "SMTP plano local"
                };
                format!("Habilitado ({tls}) con cabecera List-Unsubscribe")
            } else {
                "Configuración SMTP incompleta (revisa host, puerto, remitente y url_base_opt_out)"
                    .to_string()
            },
            ruta_o_endpoint: endpoint,
        }
    }
}

fn directorio_datos_xdg_por_defecto() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("aurora");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".local/share/aurora");
        }
    }
    PathBuf::from("data/aurora_daemon")
}

fn directorio_config_xdg_por_defecto() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("aurora");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".config/aurora");
        }
    }
    PathBuf::from("config/aurora")
}

/// Expande un prefijo `~/` o `~` hacia `$HOME` si existe en el entorno.
#[must_use]
pub fn expandir_tilde_path(ruta: &Path) -> PathBuf {
    let raw = ruta.to_string_lossy();
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if raw == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    ruta.to_path_buf()
}

fn candidatos_chromium(cfg: &Path) -> Vec<&'static str> {
    let es_generico = cfg
        .to_str()
        .is_some_and(|s| s == "chromium" || s == "chromium-browser");
    if es_generico {
        vec![
            "chromium",
            "google-chrome-stable",
            "chromium-browser",
            "google-chrome",
        ]
    } else {
        Vec::new()
    }
}

fn candidatos_piper(cfg: &Path) -> Vec<&'static str> {
    let es_generico = cfg.to_str().is_some_and(|s| s == "piper" || s == "piper-tts");
    if es_generico {
        vec!["piper", "piper-tts"]
    } else {
        Vec::new()
    }
}

/// Busca un binario en ruta explícita, en `$PATH` o entre candidatos alternativos de Arch Linux.
#[must_use]
pub fn resolver_binario_en_sistema(configurado: &Path, candidatos_fallback: &[&str]) -> Option<PathBuf> {
    let texto = configurado.to_string_lossy();
    if texto.contains('/') {
        return es_ejecutable(configurado).then(|| configurado.to_path_buf());
    }

    if let Some(encontrado) = buscar_en_path(configurado) {
        return Some(encontrado);
    }

    for candidato in candidatos_fallback {
        if let Some(encontrado) = buscar_en_path(Path::new(candidato)) {
            return Some(encontrado);
        }
    }

    // Rutas comunes en Arch Linux para instalaciones manuales de Piper.
    if candidatos_fallback.contains(&"piper") {
        if let Ok(home) = std::env::var("HOME") {
            let local_piper = PathBuf::from(home).join(".local/bin/piper");
            if es_ejecutable(&local_piper) {
                return Some(local_piper);
            }
        }
        let opt_piper = PathBuf::from("/opt/piper/piper");
        if es_ejecutable(&opt_piper) {
            return Some(opt_piper);
        }
    }

    None
}

fn buscar_en_path(binario: &Path) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidato = dir.join(binario);
        if es_ejecutable(&candidato) {
            return Some(candidato);
        }
    }
    None
}

fn es_ejecutable(ruta: &Path) -> bool {
    let Ok(meta) = fs::metadata(ruta) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn parsear_bool_env(clave: &str, raw: &str) -> DaemonResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "si" | "sí" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(DaemonError::Configuracion(format!(
            "valor booleano inválido para {clave}: '{raw}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn config_carga_toml_y_aplica_overrides_de_entorno() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = std::env::temp_dir().join(format!("aurora_cfg_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp)?;
        let toml_path = tmp.join("aurora.toml");
        fs::write(
            &toml_path,
            r#"
bind_addr = "127.0.0.1:8095"
bob_url = "http://127.0.0.1:8085"
bob_slot_id = 0
data_dir = "~/.local/share/aurora"
modo_ejecucion = "real_con_fallback"
usar_nvenc = true

[smtp]
habilitado = true
host = "smtp.resend.com"
puerto = 587
remitente_correo = "comercial@inmobiliaria-aurora.mx"

[council]
claude_bin = "/usr/local/bin/claude"
claude_max_5h = 8
claude_max_diario = 24
timeout_ms = 45000

[factory]
broll_dir = "~/.local/share/aurora/assets/broll"

[forge]
habilitado = true
timeout_segundos = 180
"#,
        )?;

        let mut env_map = HashMap::new();
        env_map.insert("AURORA_BIND_ADDR", "127.0.0.1:8199".to_string());
        env_map.insert("AURORA_SMTP_PUERTO", "465".to_string());
        env_map.insert("AURORA_SMTP_TLS_IMPLICITO", "true".to_string());
        env_map.insert("AURORA_FORGE_TIMEOUT_SEGUNDOS", "240".to_string());

        let cfg = AuroraDaemonConfig::cargar_con_lector_entorno(Some(&toml_path), |k| {
            env_map.get(k).cloned()
        })?;

        assert_eq!(cfg.bind_addr, "127.0.0.1:8199");
        assert_eq!(cfg.bob_slot_id, 0);
        assert_eq!(cfg.modo_ejecucion, ModoEjecucion::RealConFallback);
        assert!(cfg.smtp.habilitado);
        assert_eq!(cfg.smtp.host, "smtp.resend.com");
        assert_eq!(cfg.smtp.puerto, 465);
        assert!(cfg.smtp.usar_tls_implicito);
        assert_eq!(cfg.council.claude_bin, PathBuf::from("/usr/local/bin/claude"));
        assert_eq!(cfg.council.claude_max_5h, 8);
        assert_eq!(cfg.council.claude_max_diario, 24);
        assert_eq!(cfg.council.timeout_ms, 45_000);
        assert!(cfg.forge.habilitado);
        assert_eq!(cfg.forge.timeout_segundos, 240);
        assert!(cfg.council_quota_state_efectivo().ends_with("aurora/quota_state.json"));
        assert!(cfg.broll_dir_efectivo().ends_with("aurora/assets/broll"));
        assert!(cfg.forge_worktrees_dir_efectivo().ends_with("aurora/forge_worktrees"));
        assert!(cfg.wal_dir_efectivo().ends_with("aurora/wal"));
        assert!(cfg.artefactos_dir_efectivo().ends_with("aurora/artefactos"));
        assert!(cfg.dpo_dir_efectivo().ends_with("aurora/dpo"));

        let _ = fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[test]
    fn config_rechaza_slot_distinto_de_cero_y_offline_legacy_fuerza_modo() -> Result<(), Box<dyn std::error::Error>> {
        let env_slot = HashMap::from([("AURORA_BOB_SLOT_ID", "1".to_string())]);
        let err = AuroraDaemonConfig::cargar_con_lector_entorno(None, |k| env_slot.get(k).cloned());
        assert!(err.is_err());

        let env_off = HashMap::from([("AURORA_OFFLINE_MOCK", "1".to_string())]);
        let cfg = AuroraDaemonConfig::cargar_con_lector_entorno(None, |k| env_off.get(k).cloned())?;
        assert_eq!(cfg.modo_efectivo(), ModoEjecucion::SimuladoOffline);

        let env_dpo = HashMap::from([
            ("AURORA_INEGI_TOKEN", "token-inegi-mx".to_string()),
            ("AURORA_DPO_SCHEDULER_HABILITADO", "true".to_string()),
            ("AURORA_DPO_HORA_UTC", "4".to_string()),
            ("AURORA_DPO_INTERVALO_MINUTOS", "180".to_string()),
        ]);
        let cfg_dpo =
            AuroraDaemonConfig::cargar_con_lector_entorno(None, |k| env_dpo.get(k).cloned())?;
        assert_eq!(cfg_dpo.inegi_token.as_deref(), Some("token-inegi-mx"));
        assert!(cfg_dpo.dpo_scheduler_habilitado);
        assert_eq!(cfg_dpo.dpo_hora_utc, 4);
        assert_eq!(cfg_dpo.dpo_intervalo_minutos, 180);

        let env_bad_hour = HashMap::from([("AURORA_DPO_HORA_UTC", "25".to_string())]);
        assert!(AuroraDaemonConfig::cargar_con_lector_entorno(None, |k| {
            env_bad_hour.get(k).cloned()
        })
        .is_err());

        let env_bad_interval = HashMap::from([("AURORA_DPO_INTERVALO_MINUTOS", "0".to_string())]);
        assert!(AuroraDaemonConfig::cargar_con_lector_entorno(None, |k| {
            env_bad_interval.get(k).cloned()
        })
        .is_err());
        Ok(())
    }
}
