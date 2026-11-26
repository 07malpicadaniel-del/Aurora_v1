use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aurora_council::{OrquestadorConsejo, PeticionDelegacion, TipoTareaConsejo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{ForgeError, ForgeResult};

/// Lista dura e inmutable de patrones/rutas protegidas que ningún agente puede tocar.
pub const RUTAS_PROTEGIDAS: &[&str] = &[
    "validation.rs",
    "gatekeeper",
    "lfpdppp",
    "Cargo.lock",
    ".git",
    "capabilities",
];

/// Programas permitidos en [`ComandoValidacion::PersonalizadoPermitido`] (cero shells ni Python/Node).
const PROGRAMAS_VALIDACION_PERMITIDOS: &[&str] = &[
    "cargo", "pnpm", "git", "rustc", "rustfmt", "test", "true", "diff", "cmp", "grep", "rg",
];

/// Categoría de tarea autónoma de ingeniería permitida en la Forja (Carril Ámbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoOrdenIngenieria {
    /// Reparación de selectores o adaptadores OSINT públicos rotos.
    SelfHealingScraper,
    /// Nuevo conector OSINT sobre fuentes públicas con cumplimiento LFPDPPP.
    NuevoConectorOsint,
    /// Micro-calculadora o herramienta comercial auxiliar para propuestas.
    MicroCalculadoraVenta,
}

impl TipoOrdenIngenieria {
    /// Etiqueta estable para logs y mensajes de commit `forge/<id>`.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::SelfHealingScraper => "self_healing_scraper",
            Self::NuevoConectorOsint => "nuevo_conector_osint",
            Self::MicroCalculadoraVenta => "micro_calculadora_venta",
        }
    }
}

/// Compuerta determinista obligatoria antes de aceptar cualquier rama `forge/<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComandoValidacion {
    /// Ejecuta `cargo check`, `cargo clippy --all-targets -- -D warnings` y `cargo test`.
    CargoCheckClippyTest,
    /// Ejecuta `pnpm build` para validar compilación frontend.
    PnpmBuild,
    /// Ejecuta directamente un binario de validación de la lista blanca (sin shell).
    PersonalizadoPermitido {
        /// Programa ejecutable en lista blanca (ej. `"cargo"`, `"git"`, `"test"`).
        programa: String,
        /// Argumentos literales sin expansión de shell.
        argumentos: Vec<String>,
    },
}

/// Ciclo de vida de una [`OrdenIngenieria`] (Carril Ámbar: jamás auto-merge a `main`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstadoOrdenIngenieria {
    /// En espera de turno para abrir worktree aislado.
    EnCola,
    /// El agente/executor está aplicando cambios dentro del worktree temporal.
    Ejecutando,
    /// Compuerta determinista verificando compilación, clippy y pruebas.
    Validando,
    /// Rama `forge/<id>` commiteada y verificada, esperando revisión y merge humano.
    PendienteRevisionHumana,
    /// Orden abortada por bloqueo de seguridad, timeout o fallo en compuerta.
    Fallida,
}

/// Orden formal de ingeniería confinada a `rutas_permitidas` dentro de un worktree temporal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdenIngenieria {
    /// Identificador único usado como sufijo de la rama `forge/<id>`.
    pub id: Uuid,
    /// Tipo de intervención de ingeniería.
    pub tipo: TipoOrdenIngenieria,
    /// Especificación técnica de los cambios solicitados.
    pub descripcion_tecnica: String,
    /// Lista blanca explícita de rutas relativas (archivos o directorios) modificables.
    pub rutas_permitidas: Vec<String>,
    /// Compuerta determinista requerida antes de commitear en `forge/<id>`.
    pub comando_validacion: ComandoValidacion,
    /// Estado actual dentro del flujo de la Forja.
    pub estado: EstadoOrdenIngenieria,
    /// Marca temporal UTC de registro de la orden.
    pub creada_en: DateTime<Utc>,
}

impl OrdenIngenieria {
    /// Crea una nueva orden en estado [`EstadoOrdenIngenieria::EnCola`].
    #[must_use]
    pub fn nueva(
        tipo: TipoOrdenIngenieria,
        descripcion_tecnica: impl Into<String>,
        rutas_permitidas: Vec<String>,
        comando_validacion: ComandoValidacion,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            tipo,
            descripcion_tecnica: descripcion_tecnica.into(),
            rutas_permitidas,
            comando_validacion,
            estado: EstadoOrdenIngenieria::EnCola,
            creada_en: Utc::now(),
        }
    }

    /// Nombre canónico de la rama aislada asociada a esta orden (`forge/<id>`).
    #[must_use]
    pub fn nombre_rama_forge(&self) -> String {
        format!("forge/{}", self.id)
    }
}

/// Reporte auditable producido cuando la rama `forge/<id>` supera todas las compuertas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReporteForja {
    /// Identificador de la orden ejecutada.
    pub orden_id: Uuid,
    /// Categoría de orden.
    pub tipo: TipoOrdenIngenieria,
    /// Rama Git conservada para revisión humana (`forge/<id>`).
    pub rama_forge: String,
    /// Hash SHA-1 del commit creado exclusivamente sobre `rama_forge`.
    pub commit_hash: String,
    /// Estado final alcanzado (`PendienteRevisionHumana`).
    pub estado: EstadoOrdenIngenieria,
    /// Lista auditada de archivos modificados/creados (todos dentro de `rutas_permitidas`).
    pub archivos_modificados: Vec<String>,
    /// Resumen devuelto por el executor de código.
    pub resumen_executor: String,
    /// Salida resumida de la compuerta de validación.
    pub salida_validacion: String,
    /// Marca temporal UTC de finalización.
    pub ejecutado_en: DateTime<Utc>,
}

/// Contrato de ejecución de cambios de código confinado al directorio del worktree temporal.
#[async_trait]
pub trait ForgeCodeExecutor: Send + Sync {
    /// Aplica modificaciones dentro de `directorio_worktree` y retorna un resumen breve.
    async fn aplicar_cambios(
        &self,
        orden: &OrdenIngenieria,
        directorio_worktree: &Path,
    ) -> ForgeResult<String>;
}

#[derive(Debug, Deserialize)]
struct ParcheArchivoForja {
    ruta: String,
    contenido: String,
}

#[derive(Debug, Deserialize)]
struct PayloadParchesForja {
    archivos: Vec<ParcheArchivoForja>,
}

/// Executor real conectado a [`OrquestadorConsejo`] para delegar tareas de código dentro del worktree.
pub struct CouncilForgeExecutor {
    orquestador: Arc<OrquestadorConsejo>,
    timeout_ms: u64,
}

impl CouncilForgeExecutor {
    /// Construye el executor enlazado al consejo multi-agente.
    #[must_use]
    pub const fn new(orquestador: Arc<OrquestadorConsejo>, timeout_ms: u64) -> Self {
        Self {
            orquestador,
            timeout_ms,
        }
    }
}

#[async_trait]
impl ForgeCodeExecutor for CouncilForgeExecutor {
    async fn aplicar_cambios(
        &self,
        orden: &OrdenIngenieria,
        directorio_worktree: &Path,
    ) -> ForgeResult<String> {
        let tarea = match orden.tipo {
            TipoOrdenIngenieria::SelfHealingScraper | TipoOrdenIngenieria::NuevoConectorOsint => {
                TipoTareaConsejo::ReparacionScraper
            }
            TipoOrdenIngenieria::MicroCalculadoraVenta => TipoTareaConsejo::MicroHerramientaVenta,
        };

        let contexto = format!(
            "Orden de ingeniería {} ({}).\nDescripción técnica: {}\nRutas permitidas: {}\nReglas inquebrantables: cero unwrap()/expect(), prohibido tocar validation.rs, gatekeeper, lfpdppp, Cargo.lock, .git o capabilities. Si respondes con JSON de parches usa el formato {{\"archivos\":[{{\"ruta\":\"...\",\"contenido\":\"...\"}}]}}.",
            orden.id,
            orden.tipo.etiqueta(),
            orden.descripcion_tecnica.trim(),
            orden.rutas_permitidas.join(", ")
        );

        let resultado = self
            .orquestador
            .delegar(PeticionDelegacion {
                tarea,
                contexto,
                prospecto: None,
                forzar_local: false,
                directorio_trabajo: Some(directorio_worktree.to_path_buf()),
                timeout_ms: Some(self.timeout_ms),
            })
            .await?;

        // Si el agente no editó directamente archivos en el worktree pero devolvió un JSON de parches,
        // los materializamos dentro del worktree (la auditoría posterior de git status los verificará).
        if let Some(payload) = extraer_parches_json(&resultado.contenido) {
            for parche in payload.archivos {
                let ruta_norm = normalizar_ruta_relativa(&parche.ruta);
                if let Some(regla) = es_ruta_protegida(&ruta_norm) {
                    return Err(ForgeError::ArchivoProtegido {
                        ruta: ruta_norm,
                        regla: regla.to_string(),
                    });
                }
                let destino = directorio_worktree.join(&ruta_norm);
                if let Some(padre) = destino.parent() {
                    fs::create_dir_all(padre)?;
                }
                fs::write(&destino, parche.contenido)?;
            }
        }

        Ok(format!(
            "Delegado a {} en {} ms (fallback={})",
            resultado.miembro_respondedor.etiqueta(),
            resultado.latencia_ms,
            resultado.hubo_fallback
        ))
    }
}

/// Configuración del sandbox de worktrees Git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSandboxForja {
    /// Raíz del repositorio Git objetivo (jamás se escribe sobre `main`).
    pub repo_root: PathBuf,
    /// Directorio temporal donde se montan los worktrees `forge-<id>`.
    pub worktrees_dir: PathBuf,
    /// Límite de tiempo por etapa en segundos.
    pub timeout_segundos: u64,
}

/// Sandbox aislado sobre `git worktree` con lista dura de archivos protegidos y compuertas.
pub struct WorktreeSandbox {
    config: ConfigSandboxForja,
}

impl WorktreeSandbox {
    /// Crea un sandbox apuntando a un repositorio Git y directorio temporal de worktrees.
    #[must_use]
    pub fn new(mut config: ConfigSandboxForja) -> Self {
        config.timeout_segundos = config.timeout_segundos.max(5);
        Self { config }
    }

    /// Devuelve la configuración activa del sandbox.
    #[must_use]
    pub const fn config(&self) -> &ConfigSandboxForja {
        &self.config
    }

    /// Ejecuta una [`OrdenIngenieria`] completa en un `git worktree` aislado bajo `forge/<id>`.
    ///
    /// Garantiza que `main` jamás se modifica y que cualquier violación de rutas protegidas
    /// o fuera de `rutas_permitidas` aborta con [`ForgeError::ArchivoProtegido`] y limpia
    /// tanto el directorio del worktree como la rama temporal.
    #[tracing::instrument(
        skip(self, orden, executor),
        fields(orden_id = %orden.id, tipo = orden.tipo.etiqueta())
    )]
    pub async fn ejecutar_orden(
        &self,
        orden: &mut OrdenIngenieria,
        executor: &dyn ForgeCodeExecutor,
    ) -> ForgeResult<ReporteForja> {
        if orden.descripcion_tecnica.trim().is_empty() {
            orden.estado = EstadoOrdenIngenieria::Fallida;
            return Err(ForgeError::RutaNoPermitida(
                "descripcion_tecnica no puede estar vacía".to_string(),
            ));
        }

        if let Err(err) = validar_rutas_permitidas(&orden.rutas_permitidas) {
            orden.estado = EstadoOrdenIngenieria::Fallida;
            return Err(err);
        }
        if let Err(err) = validar_comando_validacion(&orden.comando_validacion) {
            orden.estado = EstadoOrdenIngenieria::Fallida;
            return Err(err);
        }

        fs::create_dir_all(&self.config.worktrees_dir)?;

        let rama = orden.nombre_rama_forge();
        let tmp_dir = self.config.worktrees_dir.join(format!("forge-{}", orden.id));
        if tmp_dir.exists() {
            let _ = fs::remove_dir_all(&tmp_dir);
        }

        orden.estado = EstadoOrdenIngenieria::Ejecutando;

        // 1. Crear rama y worktree aislado a partir de HEAD (sin tocar main).
        let args_add = vec![
            "-C".to_string(),
            self.config.repo_root.display().to_string(),
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            rama.clone(),
            tmp_dir.display().to_string(),
            "HEAD".to_string(),
        ];
        if let Err(err) = ejecutar_proceso_sin_shell(
            "git",
            &args_add,
            &self.config.repo_root,
            self.config.timeout_segundos,
            "git_worktree_add",
        )
        .await
        {
            orden.estado = EstadoOrdenIngenieria::Fallida;
            return Err(ForgeError::WorktreeFallido(err.to_string()));
        }

        info!(
            orden_id = %orden.id,
            rama = %rama,
            worktree = %tmp_dir.display(),
            "Worktree Git temporal creado para orden de ingeniería"
        );

        // 2. Ejecutar cambios del agente confinado al directorio del worktree.
        let resumen_executor = match timeout(
            Duration::from_secs(self.config.timeout_segundos),
            executor.aplicar_cambios(orden, &tmp_dir),
        )
        .await
        {
            Ok(Ok(resumen)) => resumen,
            Ok(Err(err)) => {
                self.limpiar_worktree(&tmp_dir, &rama, true).await;
                orden.estado = EstadoOrdenIngenieria::Fallida;
                return Err(err);
            }
            Err(_) => {
                self.limpiar_worktree(&tmp_dir, &rama, true).await;
                orden.estado = EstadoOrdenIngenieria::Fallida;
                return Err(ForgeError::TimeoutEjecucion {
                    etapa: "executor_codigo".to_string(),
                    timeout_segundos: self.config.timeout_segundos,
                });
            }
        };

        // 3. Auditar estrictamente `git status --porcelain` dentro del worktree.
        let archivos_modificados = match self.auditar_git_status(&tmp_dir, &orden.rutas_permitidas).await {
            Ok(archivos) => archivos,
            Err(err) => {
                warn!(
                    orden_id = %orden.id,
                    error = %err,
                    "Auditoría git status bloqueó la orden; eliminando worktree y rama temporal"
                );
                self.limpiar_worktree(&tmp_dir, &rama, true).await;
                orden.estado = EstadoOrdenIngenieria::Fallida;
                return Err(err);
            }
        };

        if archivos_modificados.is_empty() {
            self.limpiar_worktree(&tmp_dir, &rama, true).await;
            orden.estado = EstadoOrdenIngenieria::Fallida;
            return Err(ForgeError::WorktreeFallido(
                "el executor no produjo cambios auditables en el worktree".to_string(),
            ));
        }

        // 4. Ejecutar compuerta determinista de validación.
        orden.estado = EstadoOrdenIngenieria::Validando;
        let salida_validacion = match self
            .ejecutar_compuerta_validacion(&orden.comando_validacion, &tmp_dir)
            .await
        {
            Ok(salida) => salida,
            Err(err) => {
                warn!(
                    orden_id = %orden.id,
                    error = %err,
                    "Compuerta de validación fallida; abortando y limpiando worktree"
                );
                self.limpiar_worktree(&tmp_dir, &rama, true).await;
                orden.estado = EstadoOrdenIngenieria::Fallida;
                return Err(err);
            }
        };

        // 5. Commit exclusivamente en `forge/<id>` y limpieza del directorio temporal conservando la rama.
        let commit_hash = match self.commitear_en_rama_forge(&tmp_dir, orden).await {
            Ok(hash) => hash,
            Err(err) => {
                self.limpiar_worktree(&tmp_dir, &rama, true).await;
                orden.estado = EstadoOrdenIngenieria::Fallida;
                return Err(err);
            }
        };

        // Elimina el directorio temporal del worktree pero CONSERVA la rama `forge/<id>` para revisión humana.
        self.limpiar_worktree(&tmp_dir, &rama, false).await;
        orden.estado = EstadoOrdenIngenieria::PendienteRevisionHumana;

        info!(
            orden_id = %orden.id,
            rama = %rama,
            commit = %commit_hash,
            archivos = archivos_modificados.len(),
            "Rama forge lista en estado PendienteRevisionHumana (main intacta)"
        );

        Ok(ReporteForja {
            orden_id: orden.id,
            tipo: orden.tipo,
            rama_forge: rama,
            commit_hash,
            estado: EstadoOrdenIngenieria::PendienteRevisionHumana,
            archivos_modificados,
            resumen_executor,
            salida_validacion,
            ejecutado_en: Utc::now(),
        })
    }

    async fn auditar_git_status(
        &self,
        worktree_dir: &Path,
        rutas_permitidas: &[String],
    ) -> ForgeResult<Vec<String>> {
        let args = vec![
            "-C".to_string(),
            worktree_dir.display().to_string(),
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain".to_string(),
            "-uall".to_string(),
        ];

        let salida = ejecutar_proceso_sin_shell(
            "git",
            &args,
            worktree_dir,
            self.config.timeout_segundos,
            "git_status_audit",
        )
        .await
        .map_err(|e| ForgeError::WorktreeFallido(e.to_string()))?;

        let mut archivos = BTreeSet::new();
        for linea in salida.lines() {
            if linea.len() < 4 {
                continue;
            }
            let parte_ruta = linea[3..].trim();
            for ruta_cruda in parsear_rutas_porcelain(parte_ruta) {
                let ruta_norm = normalizar_ruta_relativa(&ruta_cruda);
                if ruta_norm.is_empty() {
                    continue;
                }

                if let Some(regla) = es_ruta_protegida(&ruta_norm) {
                    return Err(ForgeError::ArchivoProtegido {
                        ruta: ruta_norm,
                        regla: regla.to_string(),
                    });
                }

                if !ruta_dentro_de_permitidas(&ruta_norm, rutas_permitidas) {
                    return Err(ForgeError::ArchivoProtegido {
                        ruta: ruta_norm,
                        regla: "fuera_de_rutas_permitidas".to_string(),
                    });
                }

                archivos.insert(ruta_norm);
            }
        }

        Ok(archivos.into_iter().collect())
    }

    async fn ejecutar_compuerta_validacion(
        &self,
        comando: &ComandoValidacion,
        worktree_dir: &Path,
    ) -> ForgeResult<String> {
        match comando {
            ComandoValidacion::CargoCheckClippyTest => {
                let pasos: &[(&str, &[&str])] = &[
                    ("cargo", &["check"]),
                    ("cargo", &["clippy", "--all-targets", "--", "-D", "warnings"]),
                    ("cargo", &["test"]),
                ];
                let mut logs = Vec::new();
                for (prog, args_slice) in pasos {
                    let args: Vec<String> = args_slice.iter().map(|s| (*s).to_string()).collect();
                    let etiqueta = format!("{prog} {}", args.join(" "));
                    let out = ejecutar_proceso_sin_shell(
                        prog,
                        &args,
                        worktree_dir,
                        self.config.timeout_segundos,
                        &etiqueta,
                    )
                    .await
                    .map_err(|err| convertir_error_compuerta(err, &etiqueta))?;
                    logs.push(format!("[OK] {etiqueta}\n{}", truncar_salida(&out, 400)));
                }
                Ok(logs.join("\n"))
            }
            ComandoValidacion::PnpmBuild => {
                let args = vec!["build".to_string()];
                let out = ejecutar_proceso_sin_shell(
                    "pnpm",
                    &args,
                    worktree_dir,
                    self.config.timeout_segundos,
                    "pnpm build",
                )
                .await
                .map_err(|err| convertir_error_compuerta(err, "pnpm build"))?;
                Ok(format!("[OK] pnpm build\n{}", truncar_salida(&out, 600)))
            }
            ComandoValidacion::PersonalizadoPermitido {
                programa,
                argumentos,
            } => {
                let etiqueta = format!("{programa} {}", argumentos.join(" "));
                let out = ejecutar_proceso_sin_shell(
                    programa,
                    argumentos,
                    worktree_dir,
                    self.config.timeout_segundos,
                    &etiqueta,
                )
                .await
                .map_err(|err| convertir_error_compuerta(err, &etiqueta))?;
                Ok(format!("[OK] {etiqueta}\n{}", truncar_salida(&out, 600)))
            }
        }
    }

    async fn commitear_en_rama_forge(
        &self,
        worktree_dir: &Path,
        orden: &OrdenIngenieria,
    ) -> ForgeResult<String> {
        let args_add = vec![
            "-C".to_string(),
            worktree_dir.display().to_string(),
            "add".to_string(),
            "-A".to_string(),
        ];
        ejecutar_proceso_sin_shell(
            "git",
            &args_add,
            worktree_dir,
            self.config.timeout_segundos,
            "git_add",
        )
        .await
        .map_err(|e| ForgeError::WorktreeFallido(e.to_string()))?;

        let mensaje = format!(
            "forge({}): {} [PendienteRevisionHumana]",
            orden.id,
            orden.tipo.etiqueta()
        );
        let args_commit = vec![
            "-C".to_string(),
            worktree_dir.display().to_string(),
            "-c".to_string(),
            "user.name=Aurora Forge Sandbox".to_string(),
            "-c".to_string(),
            "user.email=forge@aurora.local".to_string(),
            "-c".to_string(),
            "commit.gpgsign=false".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            mensaje,
        ];
        ejecutar_proceso_sin_shell(
            "git",
            &args_commit,
            worktree_dir,
            self.config.timeout_segundos,
            "git_commit",
        )
        .await
        .map_err(|e| ForgeError::WorktreeFallido(e.to_string()))?;

        let args_rev = vec![
            "-C".to_string(),
            worktree_dir.display().to_string(),
            "rev-parse".to_string(),
            "HEAD".to_string(),
        ];
        let hash = ejecutar_proceso_sin_shell(
            "git",
            &args_rev,
            worktree_dir,
            self.config.timeout_segundos,
            "git_rev_parse",
        )
        .await
        .map_err(|e| ForgeError::WorktreeFallido(e.to_string()))?;

        Ok(hash.trim().to_string())
    }

    async fn limpiar_worktree(&self, worktree_dir: &Path, rama: &str, eliminar_rama: bool) {
        let args_remove = vec![
            "-C".to_string(),
            self.config.repo_root.display().to_string(),
            "worktree".to_string(),
            "remove".to_string(),
            "--force".to_string(),
            worktree_dir.display().to_string(),
        ];
        let _ = ejecutar_proceso_sin_shell(
            "git",
            &args_remove,
            &self.config.repo_root,
            30,
            "git_worktree_remove",
        )
        .await;

        if worktree_dir.exists() {
            let _ = fs::remove_dir_all(worktree_dir);
        }

        let args_prune = vec![
            "-C".to_string(),
            self.config.repo_root.display().to_string(),
            "worktree".to_string(),
            "prune".to_string(),
        ];
        let _ = ejecutar_proceso_sin_shell(
            "git",
            &args_prune,
            &self.config.repo_root,
            30,
            "git_worktree_prune",
        )
        .await;

        if eliminar_rama {
            let args_branch = vec![
                "-C".to_string(),
                self.config.repo_root.display().to_string(),
                "branch".to_string(),
                "-D".to_string(),
                rama.to_string(),
            ];
            let _ = ejecutar_proceso_sin_shell(
                "git",
                &args_branch,
                &self.config.repo_root,
                30,
                "git_branch_delete",
            )
            .await;
        }
    }
}

/// Lista todas las ramas `forge/*` presentes actualmente en el repositorio Git.
pub async fn listar_ramas_forge(repo_root: &Path) -> ForgeResult<Vec<String>> {
    if !repo_root.join(".git").exists() {
        return Ok(Vec::new());
    }

    let args = vec![
        "-C".to_string(),
        repo_root.display().to_string(),
        "branch".to_string(),
        "--list".to_string(),
        "forge/*".to_string(),
        "--format=%(refname:short)".to_string(),
    ];

    let salida = ejecutar_proceso_sin_shell("git", &args, repo_root, 15, "git_branch_list")
        .await
        .map_err(|e| ForgeError::WorktreeFallido(e.to_string()))?;

    let ramas = salida
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect();
    Ok(ramas)
}

/// Evalúa si una ruta toca algún patrón inmutable de [`RUTAS_PROTEGIDAS`] o traversal inseguro.
#[must_use]
pub fn es_ruta_protegida(ruta: &str) -> Option<&'static str> {
    let norm = normalizar_ruta_relativa(ruta);
    if norm.is_empty() {
        return Some("ruta_vacia");
    }
    if ruta.trim().starts_with('/') || ruta.trim().starts_with('\\') {
        return Some("ruta_absoluta");
    }

    for segmento in norm.split('/') {
        if segmento == ".." {
            return Some("path_traversal");
        }
        let seg_lower = segmento.to_ascii_lowercase();
        for &regla in RUTAS_PROTEGIDAS {
            let regla_lower = regla.to_ascii_lowercase();
            if seg_lower == regla_lower || seg_lower.contains(&regla_lower) {
                return Some(regla);
            }
        }
    }

    let norm_lower = norm.to_ascii_lowercase();
    for &regla in RUTAS_PROTEGIDAS {
        let regla_lower = regla.to_ascii_lowercase();
        if norm_lower == regla_lower
            || norm_lower.ends_with(&format!("/{regla_lower}"))
            || norm_lower.contains(&format!("/{regla_lower}/"))
        {
            return Some(regla);
        }
    }

    None
}

/// Valida que la lista blanca de `rutas_permitidas` no sea vacía ni incluya rutas protegidas.
pub fn validar_rutas_permitidas(rutas_permitidas: &[String]) -> ForgeResult<()> {
    if rutas_permitidas.is_empty() {
        return Err(ForgeError::RutaNoPermitida(
            "rutas_permitidas no puede estar vacía".to_string(),
        ));
    }

    for ruta in rutas_permitidas {
        let norm = normalizar_ruta_relativa(ruta);
        if norm.is_empty() {
            return Err(ForgeError::RutaNoPermitida(
                "se encontró una entrada vacía en rutas_permitidas".to_string(),
            ));
        }
        if let Some(regla) = es_ruta_protegida(ruta) {
            return Err(ForgeError::ArchivoProtegido {
                ruta: norm,
                regla: regla.to_string(),
            });
        }
    }

    Ok(())
}

/// Comprueba si `ruta` cae exactamente o dentro de alguno de los prefijos de `rutas_permitidas`.
#[must_use]
pub fn ruta_dentro_de_permitidas(ruta: &str, rutas_permitidas: &[String]) -> bool {
    let objetivo = normalizar_ruta_relativa(ruta);
    if objetivo.is_empty() {
        return false;
    }

    rutas_permitidas.iter().any(|permitida| {
        let base = normalizar_ruta_relativa(permitida);
        let base_sin_barra = base.trim_end_matches('/');
        if base_sin_barra.is_empty() {
            return false;
        }
        objetivo == base_sin_barra || objetivo.starts_with(&format!("{base_sin_barra}/"))
    })
}

fn validar_comando_validacion(comando: &ComandoValidacion) -> ForgeResult<()> {
    if let ComandoValidacion::PersonalizadoPermitido {
        programa,
        argumentos,
    } = comando
    {
        let prog = programa.trim();
        if prog.is_empty() || prog.contains('/') || prog.contains('\\') {
            return Err(ForgeError::RutaNoPermitida(format!(
                "programa de validación inválido: '{programa}'"
            )));
        }
        if !PROGRAMAS_VALIDACION_PERMITIDOS.contains(&prog) {
            return Err(ForgeError::RutaNoPermitida(format!(
                "programa '{prog}' fuera de lista blanca de validación sin shell"
            )));
        }
        for arg in argumentos {
            if arg.contains('\0') {
                return Err(ForgeError::RutaNoPermitida(
                    "argumento de validación contiene byte nulo".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn normalizar_ruta_relativa(ruta: &str) -> String {
    let mut limpia = ruta.trim().replace('\\', "/");
    while let Some(sin_prefijo) = limpia.strip_prefix("./") {
        limpia = sin_prefijo.to_string();
    }
    limpia.trim_matches('"').to_string()
}

fn parsear_rutas_porcelain(parte_ruta: &str) -> Vec<String> {
    if let Some((origen, destino)) = parte_ruta.split_once(" -> ") {
        vec![origen.trim().to_string(), destino.trim().to_string()]
    } else {
        vec![parte_ruta.trim().to_string()]
    }
}

fn extraer_parches_json(texto: &str) -> Option<PayloadParchesForja> {
    let trimmed = texto.trim();
    if let Ok(parsed) = serde_json::from_str::<PayloadParchesForja>(trimmed) {
        return Some(parsed);
    }
    if let (Some(inicio), Some(fin)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if fin > inicio {
            if let Ok(parsed) = serde_json::from_str::<PayloadParchesForja>(&trimmed[inicio..=fin])
            {
                return Some(parsed);
            }
        }
    }
    None
}

fn convertir_error_compuerta(err: ForgeError, comando: &str) -> ForgeError {
    match err {
        ForgeError::TimeoutEjecucion { .. } => err,
        ForgeError::ValidacionCompuertaFallida { .. } => err,
        otro => ForgeError::ValidacionCompuertaFallida {
            comando: comando.to_string(),
            salida: otro.to_string(),
        },
    }
}

fn truncar_salida(salida: &str, max_chars: usize) -> String {
    let limpia = salida.trim();
    if limpia.chars().count() <= max_chars {
        limpia.to_string()
    } else {
        let mut prefijo: String = limpia.chars().take(max_chars).collect();
        prefijo.push_str("...");
        prefijo
    }
}

async fn ejecutar_proceso_sin_shell(
    programa: &str,
    argumentos: &[String],
    cwd: &Path,
    timeout_segundos: u64,
    etapa: &str,
) -> ForgeResult<String> {
    debug!(
        programa,
        etapa,
        cwd = %cwd.display(),
        "Ejecutando subproceso directo en aurora-forge (sin shell)"
    );

    let child = Command::new(programa)
        .args(argumentos)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let output = match timeout(Duration::from_secs(timeout_segundos), child.wait_with_output()).await
    {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => return Err(ForgeError::Io(err)),
        Err(_) => {
            return Err(ForgeError::TimeoutEjecucion {
                etapa: etapa.to_string(),
                timeout_segundos,
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let combinado = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else if stdout.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            format!("{}\n{}", stdout.trim(), stderr.trim())
        };
        return Err(ForgeError::ValidacionCompuertaFallida {
            comando: etapa.to_string(),
            salida: truncar_salida(&combinado, 800),
        });
    }

    Ok(stdout)
}
