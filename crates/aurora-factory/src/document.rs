use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aurora_cortex::{BriefCreativo, DiagnosticoComercial, EscenaReel};
use aurora_domain::{Prospecto, VerticalComercial};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{FactoryError, FactoryResult};

/// Anchura estándar de Reel vertical (9:16).
pub const REEL_ANCHO_PX: u32 = 1080;
/// Altura estándar de Reel vertical (9:16).
pub const REEL_ALTO_PX: u32 = 1920;

/// Escapa caracteres peligrosos de HTML5 para impedir inyección XSS o ruptura de maquetado
/// desde textos extraídos por OSINT o generados por LLM.
#[must_use]
pub fn sanitizar_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            '\0' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Genera la plantilla HTML5/CSS3 ejecutiva del Dossier PDF personalizada por vertical comercial.
#[must_use]
pub fn generar_html_dossier(
    prospecto: &Prospecto,
    diagnostico: &DiagnosticoComercial,
    brief: &BriefCreativo,
) -> String {
    let (es_inmobiliaria, etiqueta_vertical, acento_hex, fondo_hero) =
        match diagnostico.vertical_optima {
            VerticalComercial::Inmobiliaria(_) => (
                true,
                "PORTAFOLIO INMOBILIARIO ESTRATÉGICO",
                "#c59b27",
                "linear-gradient(135deg, #0f172a 0%, #1e293b 100%)",
            ),
            VerticalComercial::Seguros(_) => (
                false,
                "ARQUITECTURA DE BLINDAJE PATRIMONIAL Y SEGUROS",
                "#10b981",
                "linear-gradient(135deg, #06281e 0%, #0f3d2e 100%)",
            ),
        };

    let nombre = sanitizar_html(&prospecto.nombre_normalizado);
    let ciudad = sanitizar_html(&prospecto.ubicacion.ciudad);
    let estado = sanitizar_html(&prospecto.ubicacion.estado);
    let zona = sanitizar_html(
        prospecto
            .ubicacion
            .zona
            .as_deref()
            .unwrap_or("Corredor Ejecutivo"),
    );
    let vertical_txt = sanitizar_html(diagnostico.vertical_optima.etiqueta());
    let producto = sanitizar_html(&diagnostico.producto_gancho);
    let dolor = sanitizar_html(&diagnostico.dolor_principal);
    let angulo = sanitizar_html(&diagnostico.angulo_entrada);
    let justificacion = sanitizar_html(&diagnostico.justificacion_topologica);
    let gancho_3s = sanitizar_html(&brief.gancho_3s);
    let tono = sanitizar_html(&brief.tono_narrativo);
    let fuente = sanitizar_html(&prospecto.lfpdppp.fuente_origen);
    let url_evidencia = sanitizar_html(&prospecto.lfpdppp.url_evidencia);
    let perfil_txt = sanitizar_html(prospecto.perfil_psicologico.como_str());
    let probabilidad_pct = (diagnostico.probabilidad_cierre * 100.0).round() as u32;

    let mut gatillos_html = String::new();
    for g in &prospecto.gatillos {
        let tipo = sanitizar_html(g.tipo.como_str());
        let desc = sanitizar_html(&g.descripcion);
        let urg = (g.peso_urgencia * 100.0).round() as u32;
        let _ = writeln!(
            gatillos_html,
            "<li><strong>[{tipo} | Urgencia {urg}%]</strong> {desc}</li>"
        );
    }

    let mut secciones_html = String::new();
    for (idx, seccion) in brief.secciones_dossier_pdf.iter().enumerate() {
        let sec = sanitizar_html(seccion);
        let _ = writeln!(
            secciones_html,
            "<div class=\"sec-card\"><span class=\"sec-num\">0{}</span><h3>{sec}</h3></div>",
            idx + 1
        );
    }

    let enfoque_sector = if es_inmobiliaria {
        "Evaluación patrimonial de activos inmobiliarios, plusvalía regional, eficiencia de capital de trabajo y ocupación logística/residencial en México."
    } else {
        "Análisis de continuidad directiva, deducibilidad fiscal aplicable (Art. 27 LISR), preservación familiar y blindaje fiduciario en México."
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="es-MX">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Dossier Aurora v2 — {nombre}</title>
<style>
  @page {{ size: A4; margin: 18mm; }}
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ font-family: 'Inter', 'Liberation Sans', sans-serif; color: #0f172a; line-height: 1.5; background: #ffffff; }}
  .hero {{ background: {fondo_hero}; color: #f8fafc; padding: 28px 32px; border-radius: 10px; border-left: 6px solid {acento_hex}; margin-bottom: 22px; }}
  .badge {{ display: inline-block; font-size: 11px; letter-spacing: 0.12em; text-transform: uppercase; color: {acento_hex}; font-weight: 700; margin-bottom: 8px; }}
  h1 {{ font-size: 26px; font-weight: 700; margin-bottom: 6px; }}
  .subtitle {{ font-size: 13px; color: #cbd5e1; }}
  .grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 14px; margin-bottom: 20px; }}
  .card {{ border: 1px solid #e2e8f0; border-radius: 8px; padding: 14px 16px; background: #f8fafc; }}
  .card h2 {{ font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; color: #475569; margin-bottom: 6px; }}
  .metric {{ font-size: 20px; font-weight: 700; color: #0f172a; }}
  .section {{ margin-bottom: 18px; }}
  .section h2 {{ font-size: 15px; border-bottom: 2px solid {acento_hex}; padding-bottom: 4px; margin-bottom: 10px; color: #0f172a; }}
  .hook {{ background: #fefce8; border: 1px solid #fde047; border-left: 4px solid {acento_hex}; padding: 12px 16px; border-radius: 6px; margin-bottom: 16px; font-weight: 600; }}
  ul {{ padding-left: 18px; font-size: 13px; }}
  li {{ margin-bottom: 6px; }}
  .sec-grid {{ display: grid; grid-template-columns: 1fr; gap: 8px; }}
  .sec-card {{ display: flex; align-items: center; gap: 12px; border: 1px solid #cbd5e1; border-radius: 6px; padding: 10px 12px; }}
  .sec-num {{ font-weight: 700; color: {acento_hex}; font-size: 16px; }}
  .sec-card h3 {{ font-size: 13px; font-weight: 600; }}
  footer {{ margin-top: 24px; padding-top: 10px; border-top: 1px solid #cbd5e1; font-size: 10px; color: #64748b; }}
</style>
</head>
<body>
  <section class="hero">
    <div class="badge">{etiqueta_vertical}</div>
    <h1>Propuesta Ejecutiva para {nombre}</h1>
    <p class="subtitle">{ciudad}, {estado} · {zona} · Perfil {perfil_txt} · Vertical {vertical_txt}</p>
  </section>

  <div class="hook">Gancho Estratégico: &ldquo;{gancho_3s}&rdquo;</div>

  <div class="grid">
    <div class="card">
      <h2>Solución Recomendada</h2>
      <div class="metric">{producto}</div>
    </div>
    <div class="card">
      <h2>Afinidad Topológica 4D</h2>
      <div class="metric">{probabilidad_pct}% ajuste de cierre</div>
    </div>
  </div>

  <section class="section">
    <h2>Diagnóstico y Ángulo de Valor</h2>
    <p style="font-size:13px;margin-bottom:8px;"><strong>Contexto Sectorial:</strong> {enfoque_sector}</p>
    <p style="font-size:13px;margin-bottom:8px;"><strong>Dolor Identificado:</strong> {dolor}</p>
    <p style="font-size:13px;margin-bottom:8px;"><strong>Ángulo de Entrada ({tono}):</strong> {angulo}</p>
    <p style="font-size:13px;"><strong>Justificación Topológica:</strong> {justificacion}</p>
  </section>

  <section class="section">
    <h2>Señales y Gatillos Públicos Detectados</h2>
    <ul>{gatillos_html}</ul>
  </section>

  <section class="section">
    <h2>Estructura del Dossier</h2>
    <div class="sec-grid">{secciones_html}</div>
  </section>

  <footer>
    Cumplimiento LFPDPPP: Información compilada exclusivamente a partir de fuente pública ({fuente} — {url_evidencia}). Generado por Aurora v2 bajo supervisión Gatekeeper.
  </footer>
</body>
</html>"#
    )
}

/// Genera una diapositiva vertical `1080x1920` en HTML5/CSS3 para un fotograma clave del Reel.
#[must_use]
pub fn generar_html_escena(
    escena: &EscenaReel,
    brief: &BriefCreativo,
    diagnostico: &DiagnosticoComercial,
    es_escena_gancho: bool,
) -> String {
    let (acento, fondo) = match diagnostico.vertical_optima {
        VerticalComercial::Inmobiliaria(_) => (
            "#f59e0b",
            "radial-gradient(circle at top right, #1e293b 0%, #090d16 70%)",
        ),
        VerticalComercial::Seguros(_) => (
            "#10b981",
            "radial-gradient(circle at top right, #064e3b 0%, #051612 70%)",
        ),
    };

    let overlay = sanitizar_html(&escena.texto_overlay);
    let desc_visual = sanitizar_html(&escena.descripcion_visual);
    let gancho = sanitizar_html(&brief.gancho_3s);
    let producto = sanitizar_html(&diagnostico.producto_gancho);
    let badge_superior = if es_escena_gancho || escena.indice == 1 {
        "GANCHO INICIAL 3S"
    } else {
        "AURORA INSIGHT"
    };
    let subtitulo_gancho = if es_escena_gancho || escena.indice == 1 {
        format!(r#"<div class="hook-box">{gancho}</div>"#)
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="es-MX">
<head>
<meta charset="utf-8" />
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  html, body {{ width: {REEL_ANCHO_PX}px; height: {REEL_ALTO_PX}px; overflow: hidden; font-family: 'Inter', 'Liberation Sans', sans-serif; color: #f8fafc; background: {fondo}; }}
  .canvas {{ width: 100%; height: 100%; display: flex; flex-direction: column; justify-content: space-between; padding: 120px 84px 210px 84px; }}
  .top-pill {{ display: inline-block; align-self: flex-start; padding: 14px 28px; border-radius: 999px; border: 2px solid {acento}; color: {acento}; font-size: 28px; font-weight: 800; letter-spacing: 0.14em; }}
  .center {{ display: flex; flex-direction: column; gap: 36px; }}
  h1 {{ font-size: 86px; line-height: 1.08; font-weight: 800; text-shadow: 0 6px 24px rgba(0,0,0,0.55); }}
  .hook-box {{ background: rgba(15, 23, 42, 0.85); border-left: 10px solid {acento}; padding: 32px; border-radius: 18px; font-size: 40px; line-height: 1.3; color: #e2e8f0; }}
  .visual-cue {{ font-size: 32px; color: #94a3b8; line-height: 1.4; border-top: 1px solid rgba(148,163,184,0.28); padding-top: 24px; }}
  .footer {{ display: flex; justify-content: space-between; align-items: center; font-size: 28px; color: #cbd5e1; }}
  .scene-meta {{ color: {acento}; font-weight: 700; }}
</style>
</head>
<body>
  <main class="canvas">
    <div class="top-pill">{badge_superior}</div>
    <section class="center">
      <h1>{overlay}</h1>
      {subtitulo_gancho}
      <p class="visual-cue">{desc_visual}</p>
    </section>
    <footer class="footer">
      <span>{producto}</span>
      <span class="scene-meta">Escena 0{indice} · {dur}s</span>
    </footer>
  </main>
</body>
</html>"#,
        indice = escena.indice,
        dur = escena.duracion_segundos,
    )
}

/// Genera un overlay tipográfico vertical `1080x1920` con fondo 100% transparente
/// (`background: transparent`) listo para superponerse sobre clips B-Roll reales en FFmpeg/NVENC.
///
/// Respeta estrictamente las zonas seguras verticales de Instagram Reels / TikTok / YouTube Shorts
/// (márgenes superior >= 240 px, inferior >= 460 px, derecho >= 160 px) y aplica sanitización HTML.
#[must_use]
pub fn generar_html_overlay_transparente(
    escena: &EscenaReel,
    indice: usize,
    total: usize,
    vertical: &VerticalComercial,
) -> String {
    let overlay = sanitizar_html(&escena.texto_overlay);
    let detalle = sanitizar_html(&escena.descripcion_visual);
    let etiqueta_vertical = sanitizar_html(&vertical.etiqueta().replace('/', " · ").to_uppercase());
    let total_efectivo = total.max(1);
    let numero_escena = indice + 1;
    let insignia = if indice == 0 {
        "GANCHO 3S".to_string()
    } else {
        format!("CLAVE {numero_escena:02}/{total_efectivo:02}")
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="es-MX">
<head>
<meta charset="utf-8" />
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  html, body {{
    width: {REEL_ANCHO_PX}px;
    height: {REEL_ALTO_PX}px;
    overflow: hidden;
    background: transparent !important;
    font-family: 'Inter', 'Liberation Sans', sans-serif;
    color: #ffffff;
  }}
  .safe-zone {{
    width: 100%;
    height: 100%;
    padding: 240px 160px 460px 72px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
  }}
  .header-row {{
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 14px;
  }}
  .pill {{
    display: inline-block;
    padding: 12px 24px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.72);
    border: 2px solid #ffffff;
    color: #ffffff;
    font-size: 24px;
    font-weight: 800;
    letter-spacing: 0.16em;
    text-transform: uppercase;
  }}
  .vertical-tag {{
    font-size: 20px;
    font-weight: 600;
    letter-spacing: 0.12em;
    color: rgba(255, 255, 255, 0.88);
    text-shadow: 0 2px 12px rgba(0, 0, 0, 0.92);
  }}
  .headline-card {{
    background: linear-gradient(180deg, rgba(0, 0, 0, 0.62) 0%, rgba(0, 0, 0, 0.78) 100%);
    border-left: 8px solid #ffffff;
    padding: 34px 36px;
    border-radius: 12px;
  }}
  h1 {{
    font-size: 74px;
    line-height: 1.08;
    font-weight: 900;
    color: #ffffff;
    text-shadow: 0 4px 22px rgba(0, 0, 0, 0.95);
    margin-bottom: 18px;
  }}
  .subline {{
    font-size: 30px;
    line-height: 1.32;
    font-weight: 500;
    color: #e5e5e5;
    text-shadow: 0 2px 14px rgba(0, 0, 0, 0.92);
  }}
</style>
</head>
<body>
  <main class="safe-zone">
    <header class="header-row">
      <span class="pill">{insignia}</span>
      <span class="vertical-tag">{etiqueta_vertical}</span>
    </header>
    <section class="headline-card">
      <h1>{overlay}</h1>
      <p class="subline">{detalle}</p>
    </section>
  </main>
</body>
</html>"#
    )
}

/// Contrato asíncrono para renderizado HTML/CSS a PDF y keyframes PNG en CPU/RAM.
#[async_trait]
pub trait DocumentEngine: Send + Sync {
    /// Convierte un documento HTML autocontenido en PDF en `salida_pdf` y devuelve el tiempo en ms.
    async fn renderizar_pdf(&self, html: &str, salida_pdf: &Path) -> FactoryResult<u64>;

    /// Renderiza cada escena HTML en un archivo PNG `1080x1920` dentro de `directorio_frames`.
    async fn renderizar_frames_png(
        &self,
        html_escenas: &[String],
        directorio_frames: &Path,
    ) -> FactoryResult<Vec<PathBuf>>;

    /// Renderiza cada overlay HTML con fondo transparente en PNG RGBA `1080x1920` dentro de `directorio_overlays`.
    async fn renderizar_overlays_transparentes(
        &self,
        html_overlays: &[String],
        directorio_overlays: &Path,
    ) -> FactoryResult<Vec<PathBuf>>;
}

/// Configuración de Chromium/Chrome Headless operando exclusivamente en CPU/RAM (`--disable-gpu`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigHeadlessBrowser {
    /// Binario Chromium/Chrome instalado en Arch Linux.
    pub binario_navegador: PathBuf,
    /// Timeout máximo por invocación en milisegundos.
    pub timeout_ms: u64,
    /// Ancho del viewport para renders verticales PNG (`1080`).
    pub ancho_viewport: u32,
    /// Alto del viewport para renders verticales PNG (`1920`).
    pub alto_viewport: u32,
}

impl Default for ConfigHeadlessBrowser {
    fn default() -> Self {
        Self {
            binario_navegador: PathBuf::from("chromium"),
            timeout_ms: 30_000,
            ancho_viewport: REEL_ANCHO_PX,
            alto_viewport: REEL_ALTO_PX,
        }
    }
}

/// Motor real de maquetación documental sobre Chromium headless en CPU/RAM.
#[derive(Debug, Clone)]
pub struct HeadlessBrowserDocumentEngine {
    config: ConfigHeadlessBrowser,
}

impl HeadlessBrowserDocumentEngine {
    /// Construye un motor Chromium headless en CPU (`--disable-gpu`).
    #[must_use]
    pub const fn new(config: ConfigHeadlessBrowser) -> Self {
        Self { config }
    }

    /// Devuelve la configuración activa del navegador headless.
    #[must_use]
    pub const fn config(&self) -> &ConfigHeadlessBrowser {
        &self.config
    }

    /// Genera argumentos CLI de exportación PDF garantizando `--headless=new` y `--disable-gpu`.
    #[must_use]
    pub fn construir_args_pdf(&self, uri_html: &str, salida_pdf: &Path) -> Vec<String> {
        vec![
            "--headless=new".to_string(),
            "--no-sandbox".to_string(),
            "--disable-gpu".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--no-pdf-header-footer".to_string(),
            format!("--print-to-pdf={}", salida_pdf.display()),
            uri_html.to_string(),
        ]
    }

    /// Genera argumentos CLI de captura PNG `1080x1920` garantizando `--disable-gpu`.
    #[must_use]
    pub fn construir_args_screenshot(&self, uri_html: &str, salida_png: &Path) -> Vec<String> {
        vec![
            "--headless=new".to_string(),
            "--no-sandbox".to_string(),
            "--disable-gpu".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--hide-scrollbars".to_string(),
            format!(
                "--window-size={},{}",
                self.config.ancho_viewport, self.config.alto_viewport
            ),
            format!("--screenshot={}", salida_png.display()),
            uri_html.to_string(),
        ]
    }

    /// Genera argumentos CLI de captura PNG RGBA transparente (`--default-background-color=00000000`)
    /// operando 100% en CPU/RAM (`--disable-gpu`).
    #[must_use]
    pub fn construir_args_screenshot_transparente(
        &self,
        uri_html: &str,
        salida_png: &Path,
    ) -> Vec<String> {
        vec![
            "--headless=new".to_string(),
            "--no-sandbox".to_string(),
            "--disable-gpu".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--hide-scrollbars".to_string(),
            "--default-background-color=00000000".to_string(),
            format!(
                "--window-size={},{}",
                self.config.ancho_viewport, self.config.alto_viewport
            ),
            format!("--screenshot={}", salida_png.display()),
            uri_html.to_string(),
        ]
    }

    async fn ejecutar_chromium(&self, args: &[String]) -> FactoryResult<()> {
        let timeout_ms = self.config.timeout_ms.max(500);
        let mut cmd = Command::new(&self.config.binario_navegador);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let salida = tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output())
            .await
            .map_err(|_| FactoryError::SubprocesoTimeout {
                comando: self.config.binario_navegador.display().to_string(),
                timeout_ms,
            })?
            .map_err(|e| {
                FactoryError::RenderDocumentoFallido(format!(
                    "no se pudo ejecutar '{}': {e}",
                    self.config.binario_navegador.display()
                ))
            })?;

        if !salida.status.success() {
            let stderr = String::from_utf8_lossy(&salida.stderr);
            return Err(FactoryError::RenderDocumentoFallido(format!(
                "chromium finalizó con código {:?}: {}",
                salida.status.code(),
                stderr.trim()
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl DocumentEngine for HeadlessBrowserDocumentEngine {
    #[tracing::instrument(skip(self, html), fields(salida = %salida_pdf.display()))]
    async fn renderizar_pdf(&self, html: &str, salida_pdf: &Path) -> FactoryResult<u64> {
        if html.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el HTML del dossier no puede estar vacío".to_string(),
            ));
        }

        if let Some(parent) = salida_pdf.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        let inicio = Instant::now();
        let dir_tmp = std::env::temp_dir().join(format!("aurora_doc_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir_tmp)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: dir_tmp.clone(),
                source,
            })?;
        let ruta_html = dir_tmp.join("dossier.html");

        let resultado: FactoryResult<()> = async {
            tokio::fs::write(&ruta_html, html.as_bytes())
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: ruta_html.clone(),
                    source,
                })?;
            let uri = format!("file://{}", ruta_html.display());
            let args = self.construir_args_pdf(&uri, salida_pdf);
            self.ejecutar_chromium(&args).await?;
            Ok(())
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&dir_tmp).await;
        resultado?;

        let elapsed = inicio.elapsed().as_millis() as u64;
        info!(elapsed_ms = elapsed, "Dossier PDF renderizado en CPU/RAM");
        Ok(elapsed)
    }

    #[tracing::instrument(skip(self, html_escenas), fields(escenas = html_escenas.len(), dir = %directorio_frames.display()))]
    async fn renderizar_frames_png(
        &self,
        html_escenas: &[String],
        directorio_frames: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        if html_escenas.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "se requiere al menos una escena HTML para renderizar PNG".to_string(),
            ));
        }

        tokio::fs::create_dir_all(directorio_frames)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: directorio_frames.to_path_buf(),
                source,
            })?;

        let dir_tmp = std::env::temp_dir().join(format!("aurora_frames_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir_tmp)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: dir_tmp.clone(),
                source,
            })?;

        let mut salidas = Vec::with_capacity(html_escenas.len());
        let resultado: FactoryResult<()> = async {
            for (idx, html) in html_escenas.iter().enumerate() {
                if html.trim().is_empty() {
                    return Err(FactoryError::DatosInvalidos(format!(
                        "la escena HTML #{} está vacía",
                        idx + 1
                    )));
                }
                let ruta_html = dir_tmp.join(format!("escena_{:03}.html", idx + 1));
                let ruta_png = directorio_frames.join(format!("frame_{:03}.png", idx + 1));

                tokio::fs::write(&ruta_html, html.as_bytes())
                    .await
                    .map_err(|source| FactoryError::IoArtefacto {
                        ruta: ruta_html.clone(),
                        source,
                    })?;

                let uri = format!("file://{}", ruta_html.display());
                let args = self.construir_args_screenshot(&uri, &ruta_png);
                self.ejecutar_chromium(&args).await?;
                salidas.push(ruta_png);
            }
            Ok(())
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&dir_tmp).await;
        resultado?;
        Ok(salidas)
    }

    #[tracing::instrument(skip(self, html_overlays), fields(overlays = html_overlays.len(), dir = %directorio_overlays.display()))]
    async fn renderizar_overlays_transparentes(
        &self,
        html_overlays: &[String],
        directorio_overlays: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        if html_overlays.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "se requiere al menos un overlay HTML para renderizar PNG transparente".to_string(),
            ));
        }

        tokio::fs::create_dir_all(directorio_overlays)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: directorio_overlays.to_path_buf(),
                source,
            })?;

        let dir_tmp = std::env::temp_dir().join(format!("aurora_overlays_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir_tmp)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: dir_tmp.clone(),
                source,
            })?;

        let mut salidas = Vec::with_capacity(html_overlays.len());
        let resultado: FactoryResult<()> = async {
            for (idx, html) in html_overlays.iter().enumerate() {
                if html.trim().is_empty() {
                    return Err(FactoryError::DatosInvalidos(format!(
                        "el overlay HTML #{} está vacío",
                        idx + 1
                    )));
                }
                let ruta_html = dir_tmp.join(format!("overlay_{:03}.html", idx + 1));
                let ruta_png = directorio_overlays.join(format!("overlay_{:03}.png", idx + 1));

                tokio::fs::write(&ruta_html, html.as_bytes())
                    .await
                    .map_err(|source| FactoryError::IoArtefacto {
                        ruta: ruta_html.clone(),
                        source,
                    })?;

                let uri = format!("file://{}", ruta_html.display());
                let args = self.construir_args_screenshot_transparente(&uri, &ruta_png);
                self.ejecutar_chromium(&args).await?;
                salidas.push(ruta_png);
            }
            Ok(())
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&dir_tmp).await;
        resultado?;
        Ok(salidas)
    }
}

/// Motor determinista offline para pruebas CI/CD sin Chromium instalado.
///
/// Emite un archivo `%PDF-1.4` estructuralmente válido y fotogramas `.png` `1080x1920`
/// con firma `\x89PNG\r\n\x1a\n` y CRC32 IEEE calculado en Rust puro.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicMockDocumentEngine;

#[async_trait]
impl DocumentEngine for DeterministicMockDocumentEngine {
    #[tracing::instrument(skip(self, html), fields(salida = %salida_pdf.display()))]
    async fn renderizar_pdf(&self, html: &str, salida_pdf: &Path) -> FactoryResult<u64> {
        if html.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el HTML del dossier no puede estar vacío".to_string(),
            ));
        }

        if let Some(parent) = salida_pdf.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: parent.to_path_buf(),
                    source,
                })?;
        }

        let inicio = Instant::now();
        let pdf_bytes = construir_pdf_minimo_valido(html.len());
        tokio::fs::write(salida_pdf, &pdf_bytes)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: salida_pdf.to_path_buf(),
                source,
            })?;

        let elapsed = inicio.elapsed().as_millis() as u64;
        debug!(bytes = pdf_bytes.len(), "PDF %PDF-1.4 determinista generado en NVMe");
        Ok(elapsed)
    }

    #[tracing::instrument(skip(self, html_escenas), fields(escenas = html_escenas.len(), dir = %directorio_frames.display()))]
    async fn renderizar_frames_png(
        &self,
        html_escenas: &[String],
        directorio_frames: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        if html_escenas.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "se requiere al menos una escena HTML".to_string(),
            ));
        }

        tokio::fs::create_dir_all(directorio_frames)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: directorio_frames.to_path_buf(),
                source,
            })?;

        let png_bytes = construir_png_minimo_1080x1920();
        let mut rutas = Vec::with_capacity(html_escenas.len());
        for (idx, html) in html_escenas.iter().enumerate() {
            if html.trim().is_empty() {
                return Err(FactoryError::DatosInvalidos(format!(
                    "escena HTML #{} vacía",
                    idx + 1
                )));
            }
            let ruta = directorio_frames.join(format!("frame_{:03}.png", idx + 1));
            tokio::fs::write(&ruta, &png_bytes)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: ruta.clone(),
                    source,
                })?;
            rutas.push(ruta);
        }

        Ok(rutas)
    }

    #[tracing::instrument(skip(self, html_overlays), fields(overlays = html_overlays.len(), dir = %directorio_overlays.display()))]
    async fn renderizar_overlays_transparentes(
        &self,
        html_overlays: &[String],
        directorio_overlays: &Path,
    ) -> FactoryResult<Vec<PathBuf>> {
        if html_overlays.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "se requiere al menos un overlay HTML transparente".to_string(),
            ));
        }

        tokio::fs::create_dir_all(directorio_overlays)
            .await
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: directorio_overlays.to_path_buf(),
                source,
            })?;

        let png_rgba_bytes = construir_png_rgba_transparente_1080x1920();
        let mut rutas = Vec::with_capacity(html_overlays.len());
        for (idx, html) in html_overlays.iter().enumerate() {
            if html.trim().is_empty() {
                return Err(FactoryError::DatosInvalidos(format!(
                    "overlay HTML #{} vacío",
                    idx + 1
                )));
            }
            let ruta = directorio_overlays.join(format!("overlay_{:03}.png", idx + 1));
            tokio::fs::write(&ruta, &png_rgba_bytes)
                .await
                .map_err(|source| FactoryError::IoArtefacto {
                    ruta: ruta.clone(),
                    source,
                })?;
            rutas.push(ruta);
        }

        Ok(rutas)
    }
}

fn construir_pdf_minimo_valido(html_len: usize) -> Vec<u8> {
    let stream_text = format!(
        "BT /F1 14 Tf 54 760 Td (Aurora v2 Dossier Ejecutivo - HTML {html_len} bytes) Tj ET"
    );
    let obj1 = "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let obj2 = "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let obj3 = "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n";
    let obj4 = "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n";
    let obj5 = format!(
        "5 0 obj\n<< /Length {} >>\nstream\n{stream_text}\nendstream\nendobj\n",
        stream_text.len()
    );

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(5);
    for obj in [obj1, obj2, obj3, obj4, &obj5] {
        offsets.push(pdf.len());
        pdf.push_str(obj);
    }

    let xref_start = pdf.len();
    let _ = writeln!(pdf, "xref");
    let _ = writeln!(pdf, "0 6");
    let _ = writeln!(pdf, "0000000000 65535 f ");
    for off in offsets {
        let _ = writeln!(pdf, "{off:010} 00000 n ");
    }
    let _ = writeln!(pdf, "trailer");
    let _ = writeln!(pdf, "<< /Size 6 /Root 1 0 R >>");
    let _ = writeln!(pdf, "startxref");
    let _ = writeln!(pdf, "{xref_start}");
    pdf.push_str("%%EOF\n");
    pdf.into_bytes()
}

fn construir_png_minimo_1080x1920() -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    // Firma PNG estándar de 8 bytes.
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // Chunk IHDR: 1080 x 1920, 8-bit grayscale/indexed sin filtros exóticos.
    let mut ihdr = [0_u8; 13];
    ihdr[0..4].copy_from_slice(&REEL_ANCHO_PX.to_be_bytes());
    ihdr[4..8].copy_from_slice(&REEL_ALTO_PX.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // color type RGB
    ihdr[10] = 0; // compression
    ihdr[11] = 0; // filter
    ihdr[12] = 0; // interlace
    escribir_chunk_png(&mut out, *b"IHDR", &ihdr);

    // Chunk IDAT con cabecera zlib estándar mínima.
    escribir_chunk_png(&mut out, *b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    // Chunk IEND de cierre.
    escribir_chunk_png(&mut out, *b"IEND", &[]);
    out
}

/// Construye un archivo PNG RGBA (`color_type = 6`, 8-bit) de `1080x1920` completamente transparente
/// con flujo zlib/DEFLATE RFC 1951 válido y decodificable por `ffmpeg` y cualquier visor PNG.
#[must_use]
pub fn construir_png_rgba_transparente_1080x1920() -> Vec<u8> {
    let mut out = Vec::with_capacity(60_000);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // Chunk IHDR: 1080 x 1920, 8-bit RGBA (color_type = 6).
    let mut ihdr = [0_u8; 13];
    ihdr[0..4].copy_from_slice(&REEL_ANCHO_PX.to_be_bytes());
    ihdr[4..8].copy_from_slice(&REEL_ALTO_PX.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // color type RGBA (con canal alfa)
    ihdr[10] = 0; // compression deflate
    ihdr[11] = 0; // filter adaptive
    ihdr[12] = 0; // no interlace
    escribir_chunk_png(&mut out, *b"IHDR", &ihdr);

    // Cada fila de escaneo tiene 1 byte de filtro (0x00) + 1080 * 4 bytes RGBA (0x00 = transparente).
    let total_bytes = (REEL_ALTO_PX as usize) * (1 + (REEL_ANCHO_PX as usize) * 4);
    let idat = comprimir_zlib_ceros_rfc1951(total_bytes);
    escribir_chunk_png(&mut out, *b"IDAT", &idat);
    escribir_chunk_png(&mut out, *b"IEND", &[]);
    out
}

fn comprimir_zlib_ceros_rfc1951(total_ceros: usize) -> Vec<u8> {
    let mut zlib = Vec::with_capacity((total_ceros / 258) * 2 + 64);
    // Cabecera Zlib RFC 1950: CMF=0x78 (DEFLATE 32K), FLG=0x01 (FCHECK válido).
    zlib.push(0x78);
    zlib.push(0x01);

    let mut bit_buf: u64 = 0;
    let mut bit_count: u32 = 0;
    let mut push_bits = |val: u64, nbits: u32, dst: &mut Vec<u8>| {
        bit_buf |= val << bit_count;
        bit_count += nbits;
        while bit_count >= 8 {
            dst.push((bit_buf & 0xff) as u8);
            bit_buf >>= 8;
            bit_count -= 8;
        }
    };

    // Bloque final (BFINAL=1) con Huffman fijo (BTYPE=01 -> bits LSB-first: 1, 1, 0).
    push_bits(0b011, 3, &mut zlib);

    // Literal 0x00 en Huffman fijo = 0b0011_0000 MSB-first -> 0b0000_1100 LSB-first (8 bits).
    const LIT_ZERO_LSB: u64 = 0b0000_1100;
    // Código 285 (longitud 258, 0 bits extra) = 0b1100_0101 MSB-first -> 0b1010_0011 LSB-first (8 bits).
    const LEN_258_LSB: u64 = 0b1010_0011;
    // Distancia 1 (código 0, 0 bits extra) = 5 bits cero.
    const DIST_1_LSB: u64 = 0;

    if total_ceros > 0 {
        push_bits(LIT_ZERO_LSB, 8, &mut zlib);
        let mut restantes = total_ceros - 1;
        while restantes >= 258 {
            push_bits(LEN_258_LSB, 8, &mut zlib);
            push_bits(DIST_1_LSB, 5, &mut zlib);
            restantes -= 258;
        }
        for _ in 0..restantes {
            push_bits(LIT_ZERO_LSB, 8, &mut zlib);
        }
    }

    // Fin de bloque (código 256 = 7 bits cero en Huffman fijo).
    push_bits(0, 7, &mut zlib);
    if bit_count > 0 {
        zlib.push((bit_buf & 0xff) as u8);
    }

    // Adler-32 de `total_ceros` bytes cero: s1 = 1, s2 = total_ceros % 65521.
    let s1: u32 = 1;
    let s2: u32 = (total_ceros % 65_521) as u32;
    let adler32: u32 = (s2 << 16) | s1;
    zlib.extend_from_slice(&adler32.to_be_bytes());
    zlib
}

fn escribir_chunk_png(dst: &mut Vec<u8>, tipo: [u8; 4], payload: &[u8]) {
    dst.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    dst.extend_from_slice(&tipo);
    dst.extend_from_slice(payload);

    let mut crc_input = Vec::with_capacity(4 + payload.len());
    crc_input.extend_from_slice(&tipo);
    crc_input.extend_from_slice(payload);
    let crc = crc32_ieee(&crc_input);
    dst.extend_from_slice(&crc.to_be_bytes());
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
