use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aurora_cortex::{BobInferenceBackend, RolCognitivo};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{CouncilError, CouncilResult};
use crate::quota::MiembroConsejo;
use crate::router::TipoTareaConsejo;

/// Timeout por defecto para invocaciones de CLIs headless del consejo (90 segundos).
pub const DEFAULT_COUNCIL_CLI_TIMEOUT_MS: u64 = 90_000;

/// Patrones textuales de rate-limit o agotamiento de ventana en CLIs externos.
const PATRONES_RATE_LIMIT: &[&str] = &[
    "429",
    "rate limit",
    "rate_limit",
    "too many requests",
    "quota exceeded",
    "usage limit",
    "limit reached",
    "5-hour",
    "try again later",
    "overloaded",
    "capacity",
    "resource_exhausted",
];

/// Solicitud ya sanitizada lista para despacharse a un driver del consejo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolicitudConsejo {
    /// Identificador único de trazabilidad.
    pub id: Uuid,
    /// Categoría de tarea delegada.
    pub tarea: TipoTareaConsejo,
    /// Instrucciones del rol de sistema.
    pub prompt_sistema: String,
    /// Carga útil de usuario (sanitizada con `PiiFirewall` si el destino no es `BobLocal`).
    pub prompt_usuario: String,
    /// Directorio de trabajo opcional (usado por `aurora-forge` dentro de un worktree aislado).
    pub directorio_trabajo: Option<PathBuf>,
    /// Tiempo máximo de espera en milisegundos.
    pub timeout_ms: u64,
}

impl SolicitudConsejo {
    /// Combina instrucciones de sistema y usuario para CLIs headless de un solo prompt.
    #[must_use]
    pub fn prompt_combinado(&self) -> String {
        if self.prompt_sistema.trim().is_empty() {
            self.prompt_usuario.clone()
        } else {
            format!(
                "[INSTRUCCIONES DE SISTEMA]\n{}\n\n[SOLICITUD]\n{}",
                self.prompt_sistema.trim(),
                self.prompt_usuario.trim()
            )
        }
    }
}

/// Respuesta cruda devuelta por un driver del consejo antes de rehidratar PII.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespuestaConsejo {
    /// Miembro del consejo que produjo la respuesta.
    pub miembro: MiembroConsejo,
    /// Texto de salida producido por el modelo/CLI.
    pub contenido: String,
    /// Latencia real medida en milisegundos.
    pub latencia_ms: u64,
    /// Identificador opcional del motor/modelo reportado.
    pub modelo_reportado: Option<String>,
}

/// Contrato único para adaptadores de agentes del consejo y respaldo local BOB.
#[async_trait]
pub trait CouncilAgentDriver: Send + Sync {
    /// Identifica qué miembro del consejo implementa este driver.
    fn miembro(&self) -> MiembroConsejo;

    /// Verifica si el binario o backend subyacente está listo para recibir trabajo.
    fn disponible(&self) -> bool;

    /// Ejecuta la solicitud bajo timeout acotado y sin invocación de shell.
    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo>;
}

/// Driver headless real para Claude Code CLI (`claude -p <prompt> --output-format text`).
#[derive(Debug, Clone)]
pub struct ClaudeCodeCliDriver {
    binario: PathBuf,
    timeout_por_defecto_ms: u64,
}

impl ClaudeCodeCliDriver {
    /// Crea el driver apuntando a `claude` en `$PATH` o a una ruta explícita.
    #[must_use]
    pub fn new(binario: impl Into<PathBuf>, timeout_por_defecto_ms: u64) -> Self {
        Self {
            binario: binario.into(),
            timeout_por_defecto_ms: timeout_por_defecto_ms.max(1_000),
        }
    }
}

impl Default for ClaudeCodeCliDriver {
    fn default() -> Self {
        Self::new("claude", DEFAULT_COUNCIL_CLI_TIMEOUT_MS)
    }
}

#[async_trait]
impl CouncilAgentDriver for ClaudeCodeCliDriver {
    fn miembro(&self) -> MiembroConsejo {
        MiembroConsejo::ClaudePro
    }

    fn disponible(&self) -> bool {
        binario_disponible(&self.binario)
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        let prompt = solicitud.prompt_combinado();
        let args = vec![
            "-p".to_string(),
            prompt,
            "--output-format".to_string(),
            "text".to_string(),
        ];
        ejecutar_cli_sin_shell(
            self.miembro(),
            &self.binario,
            &args,
            solicitud.directorio_trabajo.as_deref(),
            solicitud.timeout_ms.max(1_000).min(self.timeout_por_defecto_ms.max(solicitud.timeout_ms)),
        )
        .await
    }
}

/// Driver headless real para OpenAI Codex CLI (`codex exec <prompt>`).
#[derive(Debug, Clone)]
pub struct CodexCliDriver {
    binario: PathBuf,
    timeout_por_defecto_ms: u64,
}

impl CodexCliDriver {
    /// Crea el driver apuntando a `codex` en `$PATH` o a una ruta explícita.
    #[must_use]
    pub fn new(binario: impl Into<PathBuf>, timeout_por_defecto_ms: u64) -> Self {
        Self {
            binario: binario.into(),
            timeout_por_defecto_ms: timeout_por_defecto_ms.max(1_000),
        }
    }
}

impl Default for CodexCliDriver {
    fn default() -> Self {
        Self::new("codex", DEFAULT_COUNCIL_CLI_TIMEOUT_MS)
    }
}

#[async_trait]
impl CouncilAgentDriver for CodexCliDriver {
    fn miembro(&self) -> MiembroConsejo {
        MiembroConsejo::CodexPlus
    }

    fn disponible(&self) -> bool {
        binario_disponible(&self.binario)
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        let prompt = solicitud.prompt_combinado();
        let args = vec!["exec".to_string(), prompt];
        ejecutar_cli_sin_shell(
            self.miembro(),
            &self.binario,
            &args,
            solicitud.directorio_trabajo.as_deref(),
            solicitud.timeout_ms.max(1_000).min(self.timeout_por_defecto_ms.max(solicitud.timeout_ms)),
        )
        .await
    }
}

/// Driver headless real para Gemini CLI (`gemini -p <prompt>`).
#[derive(Debug, Clone)]
pub struct GeminiCliDriver {
    binario: PathBuf,
    timeout_por_defecto_ms: u64,
}

impl GeminiCliDriver {
    /// Crea el driver apuntando a `gemini` en `$PATH` o a una ruta explícita.
    #[must_use]
    pub fn new(binario: impl Into<PathBuf>, timeout_por_defecto_ms: u64) -> Self {
        Self {
            binario: binario.into(),
            timeout_por_defecto_ms: timeout_por_defecto_ms.max(1_000),
        }
    }
}

impl Default for GeminiCliDriver {
    fn default() -> Self {
        Self::new("gemini", DEFAULT_COUNCIL_CLI_TIMEOUT_MS)
    }
}

#[async_trait]
impl CouncilAgentDriver for GeminiCliDriver {
    fn miembro(&self) -> MiembroConsejo {
        MiembroConsejo::GeminiEstudiante
    }

    fn disponible(&self) -> bool {
        binario_disponible(&self.binario)
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        let prompt = solicitud.prompt_combinado();
        let args = vec!["-p".to_string(), prompt];
        ejecutar_cli_sin_shell(
            self.miembro(),
            &self.binario,
            &args,
            solicitud.directorio_trabajo.as_deref(),
            solicitud.timeout_ms.max(1_000).min(self.timeout_por_defecto_ms.max(solicitud.timeout_ms)),
        )
        .await
    }
}

/// Driver headless real para OpenCode CLI local (`opencode run <prompt>`).
#[derive(Debug, Clone)]
pub struct OpenCodeCliDriver {
    binario: PathBuf,
    timeout_por_defecto_ms: u64,
}

impl OpenCodeCliDriver {
    /// Crea el driver apuntando a `opencode` en `$PATH` o a una ruta explícita.
    #[must_use]
    pub fn new(binario: impl Into<PathBuf>, timeout_por_defecto_ms: u64) -> Self {
        Self {
            binario: binario.into(),
            timeout_por_defecto_ms: timeout_por_defecto_ms.max(1_000),
        }
    }
}

impl Default for OpenCodeCliDriver {
    fn default() -> Self {
        Self::new("opencode", DEFAULT_COUNCIL_CLI_TIMEOUT_MS)
    }
}

#[async_trait]
impl CouncilAgentDriver for OpenCodeCliDriver {
    fn miembro(&self) -> MiembroConsejo {
        MiembroConsejo::OpenCodeLocal
    }

    fn disponible(&self) -> bool {
        binario_disponible(&self.binario)
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        let prompt = solicitud.prompt_combinado();
        let args = vec!["run".to_string(), prompt];
        ejecutar_cli_sin_shell(
            self.miembro(),
            &self.binario,
            &args,
            solicitud.directorio_trabajo.as_deref(),
            solicitud.timeout_ms.max(1_000).min(self.timeout_por_defecto_ms.max(solicitud.timeout_ms)),
        )
        .await
    }
}

/// Respaldo soberano local conectado a [`BobInferenceBackend`] (`aurora-cortex`, `slot_id = 0`).
pub struct BobLocalFallbackDriver<B: BobInferenceBackend> {
    backend: B,
    slot_lock: Option<Arc<Mutex<()>>>,
}

impl<B: BobInferenceBackend> BobLocalFallbackDriver<B> {
    /// Crea el respaldo local BOB sin bloqueo adicional de slot.
    #[must_use]
    pub const fn new(backend: B) -> Self {
        Self {
            backend,
            slot_lock: None,
        }
    }

    /// Adjunta el mutex compartido de exclusividad VRAM (`slot_id = 0`).
    #[must_use]
    pub fn con_slot_lock(mut self, slot_lock: Arc<Mutex<()>>) -> Self {
        self.slot_lock = Some(slot_lock);
        self
    }
}

/// Gramática GBNF mínima válida para salidas textuales estructuradas del respaldo BOB local.
#[must_use]
pub fn gbnf_respuesta_consejo_bob() -> &'static str {
    r#"root ::= "{" ws "\"respuesta\"" ws ":" ws json-string "}" ws
ws ::= (" " | "\n")?
json-string ::= "\"" string-char* "\"" ws
string-char ::= [^"\\\x00-\x1f] | "\\" escape-char
escape-char ::= ["\\/bfnrt]
"#
}

#[async_trait]
impl<B: BobInferenceBackend> CouncilAgentDriver for BobLocalFallbackDriver<B> {
    fn miembro(&self) -> MiembroConsejo {
        MiembroConsejo::BobLocal
    }

    fn disponible(&self) -> bool {
        true
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        let inicio = Instant::now();
        let _guard = match &self.slot_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };

        let rol = match solicitud.tarea {
            TipoTareaConsejo::CopyAltoTicket
            | TipoTareaConsejo::RespuestaObjecionCompleja
            | TipoTareaConsejo::MicroHerramientaVenta => RolCognitivo::DirectorCreativo,
            TipoTareaConsejo::AuditoriaLoteDpo => RolCognitivo::Auditor,
            TipoTareaConsejo::ReparacionScraper => RolCognitivo::Perfilador,
        };

        let contexto = format!(
            "tarea_consejo: {}\n{}\n\n{}",
            solicitud.tarea.etiqueta(),
            solicitud.prompt_sistema.trim(),
            solicitud.prompt_usuario.trim()
        );

        let crudo = self
            .backend
            .inferir_estructurado(rol, &contexto, gbnf_respuesta_consejo_bob())
            .await?;

        let contenido = serde_json::from_str::<serde_json::Value>(&crudo)
            .ok()
            .and_then(|val| {
                val.get("respuesta")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or(crudo);

        Ok(RespuestaConsejo {
            miembro: MiembroConsejo::BobLocal,
            contenido,
            latencia_ms: inicio.elapsed().as_millis().min(u64::MAX as u128) as u64,
            modelo_reportado: Some("bob-local-slot-0".to_string()),
        })
    }
}

/// Comportamiento simulado para pruebas offline de [`DeterministicMockCouncilDriver`].
#[derive(Debug, Clone)]
pub enum ModoMockDriver {
    /// Responde con éxito devolviendo `plantilla_respuesta` (admite sustitución `{prompt}`).
    Exito {
        /// Respuesta fija o con placeholder `{prompt}`.
        plantilla_respuesta: String,
    },
    /// Simula un límite de tasa externo `429` con mensaje diagnóstico.
    RateLimit429(String),
    /// Simula binario no instalado en el sistema.
    NoDisponible,
    /// Simula expiración de timeout del CLI.
    Timeout,
}

/// Driver determinista offline con registro auditable de prompts recibidos para pruebas CI/CD.
#[derive(Debug, Clone)]
pub struct DeterministicMockCouncilDriver {
    miembro: MiembroConsejo,
    modo: ModoMockDriver,
    historial: Arc<std::sync::Mutex<Vec<SolicitudConsejo>>>,
}

impl DeterministicMockCouncilDriver {
    /// Crea un driver mock para un miembro y modo determinado.
    #[must_use]
    pub fn new(miembro: MiembroConsejo, modo: ModoMockDriver) -> Self {
        Self {
            miembro,
            modo,
            historial: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Atajo para un driver mock que responde exitosamente con la plantilla dada.
    #[must_use]
    pub fn exito(miembro: MiembroConsejo, plantilla_respuesta: impl Into<String>) -> Self {
        Self::new(
            miembro,
            ModoMockDriver::Exito {
                plantilla_respuesta: plantilla_respuesta.into(),
            },
        )
    }

    /// Atajo para un driver mock que simula HTTP/CLI 429 rate-limit.
    #[must_use]
    pub fn rate_limit_429(miembro: MiembroConsejo, detalle: impl Into<String>) -> Self {
        Self::new(miembro, ModoMockDriver::RateLimit429(detalle.into()))
    }

    /// Atajo para un driver mock no disponible.
    #[must_use]
    pub fn no_disponible(miembro: MiembroConsejo) -> Self {
        Self::new(miembro, ModoMockDriver::NoDisponible)
    }

    /// Devuelve copia de todas las solicitudes exactamente como llegaron a este driver.
    #[must_use]
    pub fn solicitudes_recibidas(&self) -> Vec<SolicitudConsejo> {
        match self.historial.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[async_trait]
impl CouncilAgentDriver for DeterministicMockCouncilDriver {
    fn miembro(&self) -> MiembroConsejo {
        self.miembro
    }

    fn disponible(&self) -> bool {
        !matches!(self.modo, ModoMockDriver::NoDisponible)
    }

    async fn ejecutar(&self, solicitud: &SolicitudConsejo) -> CouncilResult<RespuestaConsejo> {
        if let Ok(mut guard) = self.historial.lock() {
            guard.push(solicitud.clone());
        }

        match &self.modo {
            ModoMockDriver::Exito {
                plantilla_respuesta,
            } => {
                let contenido =
                    plantilla_respuesta.replace("{prompt}", &solicitud.prompt_usuario);
                Ok(RespuestaConsejo {
                    miembro: self.miembro,
                    contenido,
                    latencia_ms: 1,
                    modelo_reportado: Some(format!("mock-{}", self.miembro.etiqueta())),
                })
            }
            ModoMockDriver::RateLimit429(detalle) => Err(CouncilError::RateLimitExterno {
                miembro: self.miembro.etiqueta().to_string(),
                detalle: detalle.clone(),
            }),
            ModoMockDriver::NoDisponible => Err(CouncilError::CliNoDisponible {
                miembro: self.miembro.etiqueta().to_string(),
                detalle: "driver mock configurado como no disponible".to_string(),
            }),
            ModoMockDriver::Timeout => Err(CouncilError::TimeoutCli {
                miembro: self.miembro.etiqueta().to_string(),
                timeout_ms: solicitud.timeout_ms,
            }),
        }
    }
}

/// Detecta si la salida o código del CLI indica limitación de tasa o cuota externa.
#[must_use]
pub fn detectar_rate_limit_en_salida(stdout: &str, stderr: &str, codigo: Option<i32>) -> bool {
    if codigo == Some(429) {
        return true;
    }
    let combinado = format!("{stdout}\n{stderr}").to_lowercase();
    PATRONES_RATE_LIMIT.iter().any(|p| combinado.contains(p))
}

async fn ejecutar_cli_sin_shell(
    miembro: MiembroConsejo,
    binario: &Path,
    args: &[String],
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> CouncilResult<RespuestaConsejo> {
    let inicio = Instant::now();
    let mut cmd = Command::new(binario);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    debug!(
        miembro = miembro.etiqueta(),
        binario = %binario.display(),
        timeout_ms,
        "Lanzando subproceso CLI headless del consejo sin shell"
    );

    let child = cmd.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            CouncilError::CliNoDisponible {
                miembro: miembro.etiqueta().to_string(),
                detalle: format!("binario '{}' no localizado en sistema", binario.display()),
            }
        } else {
            CouncilError::SalidaInvalida {
                miembro: miembro.etiqueta().to_string(),
                detalle: format!("no se pudo iniciar '{}': {err}", binario.display()),
            }
        }
    })?;

    let resultado = timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await;
    let output = match resultado {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => {
            return Err(CouncilError::SalidaInvalida {
                miembro: miembro.etiqueta().to_string(),
                detalle: format!("fallo esperando salida del subproceso: {err}"),
            });
        }
        Err(_) => {
            warn!(
                miembro = miembro.etiqueta(),
                timeout_ms,
                "Timeout alcanzado; subproceso CLI limpiado vía kill_on_drop"
            );
            return Err(CouncilError::TimeoutCli {
                miembro: miembro.etiqueta().to_string(),
                timeout_ms,
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let codigo = output.status.code();

    if !output.status.success() {
        if detectar_rate_limit_en_salida(&stdout, &stderr, codigo) {
            return Err(CouncilError::RateLimitExterno {
                miembro: miembro.etiqueta().to_string(),
                detalle: resumir_detalle(&stderr, &stdout, codigo),
            });
        }
        return Err(CouncilError::SalidaInvalida {
            miembro: miembro.etiqueta().to_string(),
            detalle: resumir_detalle(&stderr, &stdout, codigo),
        });
    }

    if stdout.is_empty() {
        if detectar_rate_limit_en_salida(&stdout, &stderr, codigo) {
            return Err(CouncilError::RateLimitExterno {
                miembro: miembro.etiqueta().to_string(),
                detalle: resumir_detalle(&stderr, &stdout, codigo),
            });
        }
        return Err(CouncilError::SalidaInvalida {
            miembro: miembro.etiqueta().to_string(),
            detalle: "el CLI finalizó con código 0 pero sin contenido en stdout".to_string(),
        });
    }

    Ok(RespuestaConsejo {
        miembro,
        contenido: stdout,
        latencia_ms: inicio.elapsed().as_millis().min(u64::MAX as u128) as u64,
        modelo_reportado: Some(binario.display().to_string()),
    })
}

fn resumir_detalle(stderr: &str, stdout: &str, codigo: Option<i32>) -> String {
    let fuente = if !stderr.is_empty() { stderr } else { stdout };
    let truncado: String = fuente.chars().take(240).collect();
    match codigo {
        Some(c) if !truncado.is_empty() => format!("código {c}: {truncado}"),
        Some(c) => format!("código de salida {c}"),
        None if !truncado.is_empty() => truncado,
        None => "proceso terminado por señal".to_string(),
    }
}

/// Verifica si un binario configurado existe y es ejecutable en ruta explícita o `$PATH`.
#[must_use]
pub fn binario_disponible(binario: &Path) -> bool {
    let texto = binario.to_string_lossy();
    if texto.contains('/') {
        return es_ejecutable(binario);
    }

    let Some(path_env) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_env) {
        if es_ejecutable(&dir.join(binario)) {
            return true;
        }
    }
    false
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
