use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use aurora_domain::Prospecto;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{CouncilError, CouncilResult};
use crate::sanitizer::PiiFirewall;

static RE_SK_KEY: OnceLock<Option<Regex>> = OnceLock::new();
static RE_AIZA_KEY: OnceLock<Option<Regex>> = OnceLock::new();
static RE_BEARER_SECRET: OnceLock<Option<Regex>> = OnceLock::new();

/// Lista negra absoluta de nombres de archivo, extensiones y directorios que NUNCA pueden
/// entrar al entorno aislado de un CLI externo.
pub const LISTA_NEGRA_ARCHIVOS_PROHIBIDOS: &[&str] = &[
    "wal",
    "wal.log",
    "state.bin",
    "catalogacion_compra.json",
    "aurora.toml",
    ".env",
    "secrets.env",
    "gatekeeper_queue.json",
    "quota_state.json",
    "misiones_subdirector.json",
    "forge_ordenes.json",
    "data",
    ".git",
    "target",
    "node_modules",
];

/// Extensiones criptográficas o binarias sensibles prohibidas en staging.
pub const EXTENSIONES_PROHIBIDAS: &[&str] = &["pem", "key", "p12", "pfx", "kdbx", "sqlite", "db"];

/// Configuración de lista blanca y lista negra para el empaquetador de contexto aislado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigContextoAislado {
    /// Raíz del repositorio/proyecto Aurora desde donde se leen archivos permitidos.
    pub proyecto_root: PathBuf,
    /// Directorio padre donde se crean los directorios efímeros `mission-<uuid>/`.
    pub sandbox_root: PathBuf,
    /// Rutas relativas permitidas (archivos o subdirectorios de documentación/arquitectura).
    pub rutas_permitidas: Vec<String>,
    /// Patrones adicionales bloqueados además de [`LISTA_NEGRA_ARCHIVOS_PROHIBIDOS`].
    #[serde(default)]
    pub patrones_prohibidos: Vec<String>,
    /// Tamaño máximo por archivo copiado al staging (evita saturar prompts/disco).
    pub max_bytes_por_archivo: usize,
    /// Tope de archivos copiados por misión.
    pub max_archivos_copiados: usize,
}

impl ConfigContextoAislado {
    /// Construye la configuración estándar con lista blanca estricta del proyecto Aurora v2.
    #[must_use]
    pub fn por_defecto(proyecto_root: impl Into<PathBuf>, sandbox_root: impl Into<PathBuf>) -> Self {
        Self {
            proyecto_root: proyecto_root.into(),
            sandbox_root: sandbox_root.into(),
            rutas_permitidas: vec![
                "AURORA_AGENT_CONTEXT.md".to_string(),
                "docs".to_string(),
                "skills".to_string(),
                "Cargo.toml".to_string(),
                "crates/aurora-domain/Cargo.toml".to_string(),
                "crates/aurora-cortex/Cargo.toml".to_string(),
                "crates/aurora-council/Cargo.toml".to_string(),
                "crates/aurora-factory/Cargo.toml".to_string(),
                "crates/aurora-pulse/Cargo.toml".to_string(),
                "crates/aurora-daemon/Cargo.toml".to_string(),
            ],
            patrones_prohibidos: LISTA_NEGRA_ARCHIVOS_PROHIBIDOS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            max_bytes_por_archivo: 256 * 1024,
            max_archivos_copiados: 80,
        }
    }
}

/// Directorio efímero aislado `<sandbox_root>/mission-<uuid>/` con limpieza garantizada en `Drop`.
#[derive(Debug)]
pub struct EntornoMisionAislado {
    /// Identificador único de la misión.
    pub mision_id: Uuid,
    /// Ruta absoluta del directorio aislado donde se confina el `current_dir` de cada CLI.
    pub directorio: PathBuf,
    /// Ruta al brief sanitizado `MISSION_BRIEF.md`.
    pub ruta_brief: PathBuf,
    /// Lista de archivos relativos copiados desde la lista blanca del proyecto.
    pub archivos_copiados: Vec<String>,
    limpiado: bool,
}

impl EntornoMisionAislado {
    /// Elimina explícitamente el directorio efímero de staging en NVMe.
    pub fn limpiar(mut self) -> CouncilResult<()> {
        self.limpiar_interno()?;
        self.limpiado = true;
        Ok(())
    }

    fn limpiar_interno(&self) -> CouncilResult<()> {
        if self.directorio.exists() {
            fs::remove_dir_all(&self.directorio)?;
            debug!(
                mision_id = %self.mision_id,
                ruta = %self.directorio.display(),
                "Entorno aislado de misión eliminado limpiamente"
            );
        }
        Ok(())
    }
}

impl Drop for EntornoMisionAislado {
    fn drop(&mut self) {
        if !self.limpiado && self.directorio.exists() {
            if let Err(err) = fs::remove_dir_all(&self.directorio) {
                warn!(
                    mision_id = %self.mision_id,
                    ruta = %self.directorio.display(),
                    error = %err,
                    "No se pudo limpiar automáticamente el directorio efímero de misión"
                );
            }
        }
    }
}

/// Resultado verificado de la auditoría de privacidad y secretos previa al vuelo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReporteAuditoriaPrivacidad {
    /// Total de archivos escaneados dentro del entorno aislado.
    pub archivos_auditados: usize,
    /// Total de bytes inspeccionados entre archivos y prompt.
    pub bytes_auditados: usize,
    /// Confirma ausencia total de PII de prospectos y secretos.
    pub libre_de_fugas: bool,
}

/// Empaquetador determinista que prepara un entorno aislado `mission-<uuid>/` con lista blanca.
pub struct ContextPackager;

impl ContextPackager {
    /// Crea `<sandbox_root>/mission-<uuid>/`, escribe `MISSION_BRIEF.md` y copia únicamente
    /// los archivos de la lista blanca que superen las reglas de la lista negra absoluta.
    pub fn preparar_entorno_mision(
        config: &ConfigContextoAislado,
        mision_id: Uuid,
        contenido_brief_sanitizado: &str,
        incluir_contexto_proyecto: bool,
    ) -> CouncilResult<EntornoMisionAislado> {
        fs::create_dir_all(&config.sandbox_root)?;
        let directorio = config.sandbox_root.join(format!("mission-{mision_id}"));
        fs::create_dir_all(&directorio)?;

        let ruta_brief = directorio.join("MISSION_BRIEF.md");
        fs::write(&ruta_brief, contenido_brief_sanitizado.as_bytes())?;

        let mut entorno = EntornoMisionAislado {
            mision_id,
            directorio: directorio.clone(),
            ruta_brief,
            archivos_copiados: vec!["MISSION_BRIEF.md".to_string()],
            limpiado: false,
        };

        if !incluir_contexto_proyecto || !config.proyecto_root.is_dir() {
            return Ok(entorno);
        }

        let mut contador = 0_usize;
        for rel_str in &config.rutas_permitidas {
            if contador >= config.max_archivos_copiados {
                break;
            }
            let rel = Path::new(rel_str.trim());
            if rel.as_os_str().is_empty() || ruta_tiene_traversal_o_prohibida(rel, config) {
                continue;
            }

            let origen = config.proyecto_root.join(rel);
            let Ok(meta) = fs::symlink_metadata(&origen) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }

            if meta.is_file() {
                copiar_archivo_seguro(
                    &origen,
                    rel,
                    &directorio,
                    config,
                    &mut entorno.archivos_copiados,
                    &mut contador,
                )?;
            } else if meta.is_dir() {
                copiar_directorio_recursivo(
                    &origen,
                    rel,
                    &directorio,
                    config,
                    &mut entorno.archivos_copiados,
                    &mut contador,
                )?;
            }
        }

        Ok(entorno)
    }
}

/// Auditor pre-vuelo obligatorio de cero-fugas (PII de prospectos + secretos/tokens).
pub struct ContextPrivacyAudit;

impl ContextPrivacyAudit {
    /// Escanea recursivamente el entorno aislado y el prompt; aborta con
    /// [`CouncilError::FalloSanitizacionPii`] ante cualquier coincidencia de PII o secreto.
    pub fn auditar_directorio_y_prompt(
        directorio: &Path,
        prompt: &str,
        prospectos_activos: &[Prospecto],
    ) -> CouncilResult<ReporteAuditoriaPrivacidad> {
        auditar_texto_completo("prompt_mision", prompt, prospectos_activos)?;

        let mut archivos_auditados = 0_usize;
        let mut bytes_auditados = prompt.len();

        if directorio.is_dir() {
            escanear_directorio_recursivo(
                directorio,
                directorio,
                prospectos_activos,
                &mut archivos_auditados,
                &mut bytes_auditados,
            )?;
        }

        Ok(ReporteAuditoriaPrivacidad {
            archivos_auditados,
            bytes_auditados,
            libre_de_fugas: true,
        })
    }

    /// Verifica un texto aislado contra patrones de secretos/credenciales (sin exponer el secreto).
    pub fn verificar_sin_secretos(texto: &str) -> CouncilResult<()> {
        let lower = texto.to_ascii_lowercase();

        for patron in [
            "ghp_",
            "github_pat_",
            "-----begin private key-----",
            "-----begin rsa private key-----",
            "-----begin openssh private key-----",
        ] {
            if lower.contains(patron) {
                return Err(CouncilError::FalloSanitizacionPii(format!(
                    "fuga de credencial detectada por ContextPrivacyAudit (patrón '{patron}')"
                )));
            }
        }

        for clave in ["password=", "smtp_pass", "smtp_password="] {
            if let Some(pos) = lower.find(clave) {
                let resto = texto[pos + clave.len()..].trim_start();
                if !resto.is_empty()
                    && !resto.starts_with("...")
                    && !resto.starts_with("<")
                    && !resto.starts_with("[")
                {
                    return Err(CouncilError::FalloSanitizacionPii(format!(
                        "fuga de secreto detectada por ContextPrivacyAudit (clave '{clave}')"
                    )));
                }
            }
        }

        if let Some(re_sk) = regex_sk_key() {
            if re_sk.is_match(texto) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga de clave API tipo 'sk-*' detectada por ContextPrivacyAudit".to_string(),
                ));
            }
        }

        if let Some(re_aiza) = regex_aiza_key() {
            if re_aiza.is_match(texto) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga de clave API Google 'AIza*' detectada por ContextPrivacyAudit".to_string(),
                ));
            }
        }

        if let Some(re_bearer) = regex_bearer_secret() {
            for m in re_bearer.find_iter(texto) {
                let candidato = m.as_str();
                if !candidato.contains("<token>")
                    && !candidato.contains("<TOKEN>")
                    && !candidato.contains("[TOKEN")
                {
                    return Err(CouncilError::FalloSanitizacionPii(
                        "fuga de cabecera 'Bearer <token>' detectada por ContextPrivacyAudit"
                            .to_string(),
                    ));
                }
            }
        }

        Ok(())
    }
}

fn regex_sk_key() -> Option<&'static Regex> {
    RE_SK_KEY
        .get_or_init(|| Regex::new(r"\bsk-(?:proj-|ant-|live-|test-)?[A-Za-z0-9_-]{12,}\b").ok())
        .as_ref()
}

fn regex_aiza_key() -> Option<&'static Regex> {
    RE_AIZA_KEY
        .get_or_init(|| Regex::new(r"\bAIza[0-9A-Za-z_-]{16,}\b").ok())
        .as_ref()
}

fn regex_bearer_secret() -> Option<&'static Regex> {
    RE_BEARER_SECRET
        .get_or_init(|| Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{12,}\b").ok())
        .as_ref()
}

fn auditar_texto_completo(
    origen: &str,
    texto: &str,
    prospectos_activos: &[Prospecto],
) -> CouncilResult<()> {
    ContextPrivacyAudit::verificar_sin_secretos(texto).map_err(|err| {
        CouncilError::FalloSanitizacionPii(format!("bloqueo en '{origen}': {err}"))
    })?;
    PiiFirewall::verificar_texto_sin_pii_generica(texto).map_err(|err| {
        CouncilError::FalloSanitizacionPii(format!("PII genérica bloqueada en '{origen}': {err}"))
    })?;
    for prospecto in prospectos_activos {
        PiiFirewall::verificar_libre_de_pii(texto, prospecto).map_err(|err| {
            CouncilError::FalloSanitizacionPii(format!(
                "PII de prospecto activo bloqueada en '{origen}': {err}"
            ))
        })?;
    }
    Ok(())
}

fn escanear_directorio_recursivo(
    raiz: &Path,
    actual: &Path,
    prospectos_activos: &[Prospecto],
    archivos_auditados: &mut usize,
    bytes_auditados: &mut usize,
) -> CouncilResult<()> {
    for entrada_res in fs::read_dir(actual)? {
        let entrada = entrada_res?;
        let ruta = entrada.path();
        let meta = fs::symlink_metadata(&ruta)?;
        if meta.file_type().is_symlink() {
            return Err(CouncilError::FalloSanitizacionPii(format!(
                "enlace simbólico no permitido dentro del sandbox: '{}'",
                ruta.display()
            )));
        }

        let nombre = entrada.file_name().to_string_lossy().to_ascii_lowercase();
        if es_nombre_o_extension_prohibida(&nombre, &ruta) {
            return Err(CouncilError::FalloSanitizacionPii(format!(
                "archivo prohibido detectado en sandbox: '{}'",
                ruta.display()
            )));
        }

        if meta.is_dir() {
            escanear_directorio_recursivo(
                raiz,
                &ruta,
                prospectos_activos,
                archivos_auditados,
                bytes_auditados,
            )?;
        } else if meta.is_file() {
            let bytes = fs::read(&ruta)?;
            *archivos_auditados += 1;
            *bytes_auditados += bytes.len();
            let texto = String::from_utf8_lossy(&bytes);
            let rel = ruta
                .strip_prefix(raiz)
                .map_or_else(|_| ruta.display().to_string(), |p| p.display().to_string());
            auditar_texto_completo(&rel, &texto, prospectos_activos)?;
        }
    }
    Ok(())
}

fn es_nombre_o_extension_prohibida(nombre_lower: &str, ruta: &Path) -> bool {
    if nombre_lower.starts_with(".env") {
        return true;
    }
    if LISTA_NEGRA_ARCHIVOS_PROHIBIDOS.contains(&nombre_lower) {
        return true;
    }
    if let Some(ext) = ruta
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        if EXTENSIONES_PROHIBIDAS.contains(&ext.as_str()) {
            return true;
        }
    }
    false
}

fn ruta_tiene_traversal_o_prohibida(rel: &Path, config: &ConfigContextoAislado) -> bool {
    if rel.is_absolute() {
        return true;
    }
    for comp in rel.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return true,
            Component::Normal(seg) => {
                let s = seg.to_string_lossy().to_ascii_lowercase();
                if es_nombre_o_extension_prohibida(&s, Path::new(&s)) {
                    return true;
                }
                if config
                    .patrones_prohibidos
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(&s))
                {
                    return true;
                }
            }
            Component::CurDir => {}
        }
    }
    false
}

fn copiar_archivo_seguro(
    origen: &Path,
    rel: &Path,
    sandbox_dir: &Path,
    config: &ConfigContextoAislado,
    copiados: &mut Vec<String>,
    contador: &mut usize,
) -> CouncilResult<()> {
    if *contador >= config.max_archivos_copiados || ruta_tiene_traversal_o_prohibida(rel, config) {
        return Ok(());
    }
    let meta = fs::metadata(origen)?;
    if meta.len() as usize > config.max_bytes_por_archivo {
        return Ok(());
    }
    let destino = sandbox_dir.join(rel);
    if let Some(padre) = destino.parent() {
        fs::create_dir_all(padre)?;
    }
    let bytes = fs::read(origen)?;
    // Solo copiamos texto UTF-8 válido (documentación, código fuente, TOML, Markdown).
    if std::str::from_utf8(&bytes).is_err() {
        return Ok(());
    }
    fs::write(&destino, &bytes)?;
    copiados.push(rel.display().to_string());
    *contador += 1;
    Ok(())
}

fn copiar_directorio_recursivo(
    origen: &Path,
    rel_base: &Path,
    sandbox_dir: &Path,
    config: &ConfigContextoAislado,
    copiados: &mut Vec<String>,
    contador: &mut usize,
) -> CouncilResult<()> {
    if *contador >= config.max_archivos_copiados {
        return Ok(());
    }
    let Ok(entradas) = fs::read_dir(origen) else {
        return Ok(());
    };
    for entrada_res in entradas {
        if *contador >= config.max_archivos_copiados {
            break;
        }
        let Ok(entrada) = entrada_res else {
            continue;
        };
        let ruta = entrada.path();
        let Ok(meta) = fs::symlink_metadata(&ruta) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = rel_base.join(entrada.file_name());
        if ruta_tiene_traversal_o_prohibida(&rel, config) {
            continue;
        }
        if meta.is_dir() {
            copiar_directorio_recursivo(&ruta, &rel, sandbox_dir, config, copiados, contador)?;
        } else if meta.is_file() {
            copiar_archivo_seguro(&ruta, &rel, sandbox_dir, config, copiados, contador)?;
        }
    }
    Ok(())
}
