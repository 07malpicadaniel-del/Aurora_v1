use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use aurora_forge::{
    listar_ramas_forge, ComandoValidacion, ConfigSandboxForja, EstadoOrdenIngenieria,
    ForgeCodeExecutor, ForgeError, ForgeResult, OrdenIngenieria, TipoOrdenIngenieria,
    WorktreeSandbox,
};
use uuid::Uuid;

fn ejecutar_git(repo: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git").current_dir(repo).args(args).output()?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} falló: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn crear_repo_git_temporal(prefijo: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raiz = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
    let repo = raiz.join("repo");
    fs::create_dir_all(repo.join("src"))?;

    ejecutar_git(&repo, &["init", "-b", "main"])?;
    ejecutar_git(&repo, &["config", "user.name", "Aurora Test"])?;
    ejecutar_git(&repo, &["config", "user.email", "test@aurora.local"])?;
    ejecutar_git(&repo, &["config", "commit.gpgsign", "false"])?;

    fs::write(
        repo.join("src/scraper_demo.rs"),
        "pub fn selector_css() -> &'static str { \".card-antigua\" }\n",
    )?;
    fs::write(
        repo.join("src/validation.rs"),
        "pub const LFPDPPP_BLOQUEO: bool = true;\n",
    )?;
    fs::write(
        repo.join("src/otro_modulo.rs"),
        "pub const VERSION_BASE: u32 = 1;\n",
    )?;

    ejecutar_git(&repo, &["add", "-A"])?;
    ejecutar_git(&repo, &["commit", "-m", "commit inicial en main"])?;

    Ok(raiz)
}

struct MockExecutorArchivo {
    ruta_relativa: String,
    nuevo_contenido: String,
}

#[async_trait]
impl ForgeCodeExecutor for MockExecutorArchivo {
    async fn aplicar_cambios(
        &self,
        _orden: &OrdenIngenieria,
        directorio_worktree: &Path,
    ) -> ForgeResult<String> {
        let destino = directorio_worktree.join(&self.ruta_relativa);
        if let Some(padre) = destino.parent() {
            fs::create_dir_all(padre)?;
        }
        fs::write(&destino, &self.nuevo_contenido)?;
        Ok(format!("Modificado {}", self.ruta_relativa))
    }
}

#[tokio::test]
async fn forge_caso_exitoso_crea_rama_pendiente_revision_sin_tocar_main(
) -> Result<(), Box<dyn std::error::Error>> {
    let raiz = crear_repo_git_temporal("aurora_forge_ok")?;
    let repo = raiz.join("repo");
    let worktrees_dir = raiz.join("worktrees");

    let head_main_antes = ejecutar_git(&repo, &["rev-parse", "main"])?;
    let contenido_main_antes = fs::read_to_string(repo.join("src/scraper_demo.rs"))?;

    let sandbox = WorktreeSandbox::new(ConfigSandboxForja {
        repo_root: repo.clone(),
        worktrees_dir: worktrees_dir.clone(),
        timeout_segundos: 30,
    });

    let mut orden = OrdenIngenieria::nueva(
        TipoOrdenIngenieria::SelfHealingScraper,
        "Actualizar selector CSS roto del portal público",
        vec!["src/scraper_demo.rs".to_string()],
        ComandoValidacion::PersonalizadoPermitido {
            programa: "grep".to_string(),
            argumentos: vec![
                "-q".to_string(),
                "card-nueva-v2".to_string(),
                "src/scraper_demo.rs".to_string(),
            ],
        },
    );

    let executor = MockExecutorArchivo {
        ruta_relativa: "src/scraper_demo.rs".to_string(),
        nuevo_contenido: "pub fn selector_css() -> &'static str { \".card-nueva-v2\" }\n"
            .to_string(),
    };

    let reporte = sandbox.ejecutar_orden(&mut orden, &executor).await?;

    assert_eq!(orden.estado, EstadoOrdenIngenieria::PendienteRevisionHumana);
    assert_eq!(reporte.estado, EstadoOrdenIngenieria::PendienteRevisionHumana);
    assert_eq!(reporte.rama_forge, format!("forge/{}", orden.id));
    assert_eq!(reporte.archivos_modificados, vec!["src/scraper_demo.rs"]);
    assert!(!reporte.commit_hash.is_empty());

    // Directorio temporal del worktree fue limpiado, pero la rama forge/<id> persiste.
    assert!(!worktrees_dir.join(format!("forge-{}", orden.id)).exists());
    let ramas = listar_ramas_forge(&repo).await?;
    assert!(ramas.contains(&reporte.rama_forge));

    // Verificación absoluta: `main` no cambió ni un solo commit ni un byte en disco.
    let head_main_despues = ejecutar_git(&repo, &["rev-parse", "main"])?;
    let contenido_main_despues = fs::read_to_string(repo.join("src/scraper_demo.rs"))?;
    assert_eq!(head_main_antes, head_main_despues);
    assert_eq!(contenido_main_antes, contenido_main_despues);
    assert_ne!(head_main_despues, reporte.commit_hash);

    // El commit en la rama forge/<id> sí contiene el parche verificado.
    let contenido_en_forge =
        ejecutar_git(&repo, &["show", &format!("{}:src/scraper_demo.rs", reporte.rama_forge)])?;
    assert!(contenido_en_forge.contains("card-nueva-v2"));

    let _ = fs::remove_dir_all(&raiz);
    Ok(())
}

#[tokio::test]
async fn forge_bloquea_archivo_protegido_validation_rs_y_limpia_worktree(
) -> Result<(), Box<dyn std::error::Error>> {
    let raiz = crear_repo_git_temporal("aurora_forge_protected")?;
    let repo = raiz.join("repo");
    let worktrees_dir = raiz.join("worktrees");

    let head_main_antes = ejecutar_git(&repo, &["rev-parse", "main"])?;

    let sandbox = WorktreeSandbox::new(ConfigSandboxForja {
        repo_root: repo.clone(),
        worktrees_dir: worktrees_dir.clone(),
        timeout_segundos: 30,
    });

    let mut orden = OrdenIngenieria::nueva(
        TipoOrdenIngenieria::SelfHealingScraper,
        "Intento malicioso de alterar reglas LFPDPPP en validation.rs",
        vec!["src/scraper_demo.rs".to_string()],
        ComandoValidacion::PersonalizadoPermitido {
            programa: "true".to_string(),
            argumentos: Vec::new(),
        },
    );

    let executor_malicioso = MockExecutorArchivo {
        ruta_relativa: "src/validation.rs".to_string(),
        nuevo_contenido: "pub const LFPDPPP_BLOQUEO: bool = false;\n".to_string(),
    };

    let err = sandbox.ejecutar_orden(&mut orden, &executor_malicioso).await;
    assert!(
        matches!(err, Err(ForgeError::ArchivoProtegido { ref regla, .. }) if regla == "validation.rs"),
        "Se esperaba ForgeError::ArchivoProtegido para validation.rs, se obtuvo: {err:?}"
    );
    assert_eq!(orden.estado, EstadoOrdenIngenieria::Fallida);

    // Worktree eliminado y rama temporal purgada.
    assert!(!worktrees_dir.join(format!("forge-{}", orden.id)).exists());
    assert!(listar_ramas_forge(&repo).await?.is_empty());
    assert_eq!(ejecutar_git(&repo, &["rev-parse", "main"])?, head_main_antes);

    let _ = fs::remove_dir_all(&raiz);
    Ok(())
}

#[tokio::test]
async fn forge_bloquea_archivo_fuera_de_rutas_permitidas_y_limpia_worktree(
) -> Result<(), Box<dyn std::error::Error>> {
    let raiz = crear_repo_git_temporal("aurora_forge_out_of_scope")?;
    let repo = raiz.join("repo");
    let worktrees_dir = raiz.join("worktrees");

    let sandbox = WorktreeSandbox::new(ConfigSandboxForja {
        repo_root: repo.clone(),
        worktrees_dir: worktrees_dir.clone(),
        timeout_segundos: 30,
    });

    let mut orden = OrdenIngenieria::nueva(
        TipoOrdenIngenieria::NuevoConectorOsint,
        "Ajuste acotado únicamente a scraper_demo.rs",
        vec!["src/scraper_demo.rs".to_string()],
        ComandoValidacion::PersonalizadoPermitido {
            programa: "true".to_string(),
            argumentos: Vec::new(),
        },
    );

    let executor_fuera_alcance = MockExecutorArchivo {
        ruta_relativa: "src/otro_modulo.rs".to_string(),
        nuevo_contenido: "pub const VERSION_BASE: u32 = 999;\n".to_string(),
    };

    let err = sandbox
        .ejecutar_orden(&mut orden, &executor_fuera_alcance)
        .await;
    assert!(
        matches!(err, Err(ForgeError::ArchivoProtegido { ref regla, .. }) if regla == "fuera_de_rutas_permitidas"),
        "Se esperaba ForgeError::ArchivoProtegido por ruta fuera de alcance, se obtuvo: {err:?}"
    );
    assert_eq!(orden.estado, EstadoOrdenIngenieria::Fallida);
    assert!(!worktrees_dir.join(format!("forge-{}", orden.id)).exists());
    assert!(listar_ramas_forge(&repo).await?.is_empty());

    let _ = fs::remove_dir_all(&raiz);
    Ok(())
}
