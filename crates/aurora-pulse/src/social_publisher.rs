use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aurora_council::PiiFirewall;
use aurora_domain::{SubverticalInmobiliaria, SubverticalSeguros, VerticalComercial};
use aurora_factory::{sha256_bytes, validar_firma_mp4_mov, validar_ruta_sin_traversal};
use aurora_memory::IslaVertical;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{PulseError, PulseResult};
use crate::rate_limiter::{ControladorTasaPlataforma, PlataformaPulso};
use crate::telemetry::{EventoTelemetria, RecolectorTelemetria, TipoEventoTelemetria};

/// Endpoint oficial resumible de YouTube Data API v3.
pub const DEFAULT_YOUTUBE_UPLOAD_URL: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos";

/// Base oficial de Meta Graph API para publicación de Instagram Reels.
pub const DEFAULT_META_GRAPH_BASE_URL: &str = "https://graph.facebook.com/v21.0";

/// Solicitud de publicación de contenido orgánico educativo (Carril Verde, cero PII).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolicitudPublicacionOrganica {
    /// Campaña orgánica asociada.
    pub campana_id: Uuid,
    /// Vertical comercial educativa (Inmobiliaria o Seguros).
    pub vertical: VerticalComercial,
    /// Título corto y libre de PII para YouTube Shorts / Meta Reels.
    pub titulo: String,
    /// Descripción educativa pública (cero PII).
    pub descripcion: String,
    /// Etiquetas temáticas sin `#` duplicado.
    #[serde(default)]
    pub hashtags: Vec<String>,
    /// Ruta en NVMe del Reel `.mp4` manufacturado y sellado.
    pub ruta_mp4: PathBuf,
    /// Huella SHA-256 hexadecimal esperada del archivo `.mp4` (64 caracteres).
    pub sha256_mp4: String,
    /// Fecha UTC programada opcional (si es `None`, publica inmediatamente).
    #[serde(default)]
    pub programado_para: Option<DateTime<Utc>>,
    /// Declaración obligatoria de que la pieza pertenece al Carril Verde sin PII.
    pub libre_de_pii: bool,
}

/// Comprobante verificable emitido tras publicar un Reel orgánico.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprobantePublicacionOrganica {
    /// Identificador único local de la publicación orgánica.
    pub publicacion_id: Uuid,
    /// Campaña orgánica asociada.
    pub campana_id: Uuid,
    /// Plataforma donde se publicó (`YouTubeShorts` o `Instagram`).
    pub plataforma: PlataformaPulso,
    /// Identificador de video/media devuelto por la API oficial o mock.
    pub id_remoto: String,
    /// URL pública resultante del Reel/Short.
    pub url_publica: String,
    /// Huella SHA-256 verificada del archivo `.mp4` publicado.
    pub sha256_mp4: String,
    /// Fecha UTC efectiva de publicación.
    pub publicado_en: DateTime<Utc>,
    /// Indica si la publicación se completó con el adaptador offline determinista.
    pub simulado: bool,
}

/// Estado observable de un publicador orgánico registrado en el daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstadoPublicadorOrganico {
    /// Plataforma atendida.
    pub plataforma: PlataformaPulso,
    /// `true` si cuenta con credenciales OAuth/Graph reales configuradas.
    pub modo_real_configurado: bool,
    /// Resumen operativo legible.
    pub detalle: String,
}

/// Contrato asíncrono para publicación orgánica de Reels verticales (Carril Verde Inbound).
#[async_trait]
pub trait OrganicSocialPublisher: Send + Sync {
    /// Plataforma gestionada por este adaptador (`YouTubeShorts`, `Instagram`, etc.).
    fn plataforma(&self) -> PlataformaPulso;

    /// Indica si este adaptador apunta a una API oficial real con credenciales activas.
    fn modo_real_configurado(&self) -> bool {
        false
    }

    /// Detalle técnico breve para diagnóstico y `/api/v1/pulse/organico`.
    fn detalle_configuracion(&self) -> String;

    /// Publica el Reel `.mp4` validado en la plataforma correspondiente.
    async fn publicar_reel_organico(
        &self,
        solicitud: &SolicitudPublicacionOrganica,
    ) -> PulseResult<ComprobantePublicacionOrganica>;
}

/// Valida integridad SHA-256 en NVMe y cumplimiento estricto de cero PII (Carril Verde).
pub async fn validar_solicitud_organica(
    solicitud: &SolicitudPublicacionOrganica,
) -> PulseResult<Vec<u8>> {
    if !solicitud.libre_de_pii {
        return Err(PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: "bloqueo LFPDPPP Carril Verde: libre_de_pii debe ser true para contenido orgánico público".to_string(),
        });
    }

    if solicitud.titulo.trim().is_empty() || solicitud.descripcion.trim().is_empty() {
        return Err(PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: "titulo y descripcion son obligatorios para publicación orgánica".to_string(),
        });
    }

    let hash_esperado = solicitud.sha256_mp4.trim().to_ascii_lowercase();
    if hash_esperado.len() != 64 || !hash_esperado.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: "sha256_mp4 debe ser un digest SHA-256 hexadecimal de 64 caracteres".to_string(),
        });
    }

    // Filtro estricto anti-PII vía PiiFirewall antes de salir a redes públicas.
    let texto_publico = format!(
        "{}\n{}\n{}",
        solicitud.titulo,
        solicitud.descripcion,
        solicitud.hashtags.join(" ")
    );
    PiiFirewall::verificar_texto_sin_pii_generica(&texto_publico).map_err(|err| {
        PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: format!("contenido orgánico bloqueado por PiiFirewall: {err}"),
        }
    })?;

    validar_ruta_sin_traversal(&solicitud.ruta_mp4)?;
    let bytes = tokio::fs::read(&solicitud.ruta_mp4).await.map_err(|e| {
        PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: format!(
                "no se pudo leer archivo MP4 '{}': {e}",
                solicitud.ruta_mp4.display()
            ),
        }
    })?;
    validar_firma_mp4_mov(&bytes)?;

    let hash_real = sha256_bytes(&bytes);
    if hash_real != hash_esperado {
        return Err(PulseError::DespachoFallido {
            canal: "PublicacionOrganica".to_string(),
            tracking_id: None,
            detalle: format!(
                "integridad SHA-256 fallida para '{}': esperado {hash_esperado}, calculado {hash_real}",
                solicitud.ruta_mp4.display()
            ),
        });
    }

    Ok(bytes)
}

/// Configuración oficial para [`YouTubeShortsApiPublisher`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigYouTubeShorts {
    /// Token OAuth 2.0 con scope `youtube.upload` (`AURORA_YOUTUBE_ACCESS_TOKEN`).
    pub access_token: Option<String>,
    /// URL del endpoint resumible de YouTube Data API v3.
    pub upload_url: String,
    /// ID de categoría YouTube (`"22"` People & Blogs / `"27"` Education).
    pub category_id: String,
    /// Estado de privacidad (`"public"`, `"unlisted"`, `"private"`).
    pub privacy_status: String,
    /// Timeout HTTP en milisegundos.
    pub timeout_ms: u64,
}

impl Default for ConfigYouTubeShorts {
    fn default() -> Self {
        Self {
            access_token: None,
            upload_url: DEFAULT_YOUTUBE_UPLOAD_URL.to_string(),
            category_id: "27".to_string(),
            privacy_status: "public".to_string(),
            timeout_ms: 60_000,
        }
    }
}

impl ConfigYouTubeShorts {
    /// Carga configuración desde variables de entorno (`AURORA_YOUTUBE_ACCESS_TOKEN`).
    #[must_use]
    pub fn desde_entorno() -> Self {
        let mut cfg = Self::default();
        if let Ok(tok) = std::env::var("AURORA_YOUTUBE_ACCESS_TOKEN") {
            let limpio = tok.trim().to_string();
            if !limpio.is_empty() {
                cfg.access_token = Some(limpio);
            }
        }
        cfg
    }
}

/// Adaptador oficial de **YouTube Data API v3** (flujo resumible de 2 pasos vía `reqwest` + `rustls-tls`).
pub struct YouTubeShortsApiPublisher {
    config: ConfigYouTubeShorts,
    http: Client,
}

impl YouTubeShortsApiPublisher {
    /// Construye el cliente HTTP `rustls-tls` para YouTube Shorts.
    pub fn new(config: ConfigYouTubeShorts) -> PulseResult<Self> {
        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1_000)))
            .build()
            .map_err(|e| PulseError::DespachoFallido {
                canal: PlataformaPulso::YouTubeShorts.etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error creando cliente HTTP YouTube: {e}"),
            })?;
        Ok(Self { config, http })
    }
}

#[derive(Debug, Deserialize)]
struct YouTubeUploadResponse {
    id: Option<String>,
}

#[async_trait]
impl OrganicSocialPublisher for YouTubeShortsApiPublisher {
    fn plataforma(&self) -> PlataformaPulso {
        PlataformaPulso::YouTubeShorts
    }

    fn modo_real_configurado(&self) -> bool {
        self.config
            .access_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    }

    fn detalle_configuracion(&self) -> String {
        if self.modo_real_configurado() {
            format!("YouTube Data API v3 lista ({})", self.config.upload_url)
        } else {
            "AURORA_YOUTUBE_ACCESS_TOKEN no configurado (usa respaldo offline)".to_string()
        }
    }

    #[tracing::instrument(skip(self, solicitud), fields(campana_id = %solicitud.campana_id))]
    async fn publicar_reel_organico(
        &self,
        solicitud: &SolicitudPublicacionOrganica,
    ) -> PulseResult<ComprobantePublicacionOrganica> {
        let bytes = validar_solicitud_organica(solicitud).await?;
        let token = self
            .config
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "falta AURORA_YOUTUBE_ACCESS_TOKEN para publicar en YouTube Shorts"
                    .to_string(),
            })?;

        let mut tags: Vec<String> = solicitud
            .hashtags
            .iter()
            .map(|h| h.trim().trim_start_matches('#').to_string())
            .filter(|h| !h.is_empty())
            .collect();
        if !tags.iter().any(|t| t.eq_ignore_ascii_case("shorts")) {
            tags.push("Shorts".to_string());
        }

        let mut status_obj = serde_json::json!({
            "privacyStatus": if solicitud.programado_para.is_some() {
                "private"
            } else {
                self.config.privacy_status.as_str()
            },
            "selfDeclaredMadeForKids": false,
        });
        if let Some(fecha) = solicitud.programado_para {
            if let Some(obj) = status_obj.as_object_mut() {
                obj.insert(
                    "publishAt".to_string(),
                    serde_json::Value::String(fecha.to_rfc3339()),
                );
            }
        }

        let descripcion_final = format!(
            "{}\n\n{}",
            solicitud.descripcion.trim(),
            tags.iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        );

        let metadata = serde_json::json!({
            "snippet": {
                "title": solicitud.titulo.trim(),
                "description": descripcion_final.trim(),
                "tags": tags,
                "categoryId": self.config.category_id,
            },
            "status": status_obj,
        });

        let init_url = format!(
            "{}?uploadType=resumable&part=snippet,status",
            self.config.upload_url.trim_end_matches('?')
        );

        let init_resp = self
            .http
            .post(&init_url)
            .bearer_auth(token)
            .header("X-Upload-Content-Type", "video/mp4")
            .header("X-Upload-Content-Length", bytes.len().to_string())
            .json(&metadata)
            .send()
            .await
            .map_err(|e| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error iniciando carga resumible en YouTube: {e}"),
            })?;

        if init_resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: self.plataforma().etiqueta().to_string(),
                motivo: "HTTP 429 en inicio de carga resumible YouTube Data API v3".to_string(),
                esperar_ms: 60_000,
            });
        }

        if !init_resp.status().is_success() {
            let code = init_resp.status().as_u16();
            let cuerpo = init_resp.text().await.unwrap_or_default();
            return Err(PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("YouTube API rechazó inicio resumible (HTTP {code}): {cuerpo}"),
            });
        }

        let upload_location = init_resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "YouTube API no devolvió cabecera Location para carga resumible"
                    .to_string(),
            })?
            .to_string();

        let put_resp = self
            .http
            .put(&upload_location)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "video/mp4")
            .body(bytes)
            .send()
            .await
            .map_err(|e| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error subiendo binario MP4 a YouTube: {e}"),
            })?;

        if put_resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: self.plataforma().etiqueta().to_string(),
                motivo: "HTTP 429 durante subida MP4 YouTube Data API v3".to_string(),
                esperar_ms: 60_000,
            });
        }

        if !put_resp.status().is_success() {
            let code = put_resp.status().as_u16();
            let cuerpo = put_resp.text().await.unwrap_or_default();
            return Err(PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("YouTube API falló al finalizar subida (HTTP {code}): {cuerpo}"),
            });
        }

        let parsed: YouTubeUploadResponse = put_resp.json().await.map_err(|e| {
            PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("respuesta JSON inválida de YouTube API: {e}"),
            }
        })?;

        let id_remoto = parsed.id.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "YouTube API respondió 200 sin video id".to_string(),
            }
        })?;

        Ok(ComprobantePublicacionOrganica {
            publicacion_id: Uuid::new_v4(),
            campana_id: solicitud.campana_id,
            plataforma: PlataformaPulso::YouTubeShorts,
            url_publica: format!("https://youtube.com/shorts/{id_remoto}"),
            id_remoto,
            sha256_mp4: solicitud.sha256_mp4.to_ascii_lowercase(),
            publicado_en: Utc::now(),
            simulado: false,
        })
    }
}

/// Configuración oficial para [`MetaReelsGraphPublisher`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigMetaReels {
    /// Identificador de cuenta profesional IG (`AURORA_META_IG_USER_ID`).
    pub ig_user_id: Option<String>,
    /// Token Graph API de largo plazo (`AURORA_META_ACCESS_TOKEN`).
    pub access_token: Option<String>,
    /// URL base de Meta Graph API (`https://graph.facebook.com/v21.0`).
    pub graph_base_url: String,
    /// URL HTTPS pública base desde donde Meta puede leer el MP4 (`AURORA_META_MEDIA_BASE_URL`).
    pub media_public_base_url: Option<String>,
    /// Timeout HTTP en milisegundos.
    pub timeout_ms: u64,
}

impl Default for ConfigMetaReels {
    fn default() -> Self {
        Self {
            ig_user_id: None,
            access_token: None,
            graph_base_url: DEFAULT_META_GRAPH_BASE_URL.to_string(),
            media_public_base_url: None,
            timeout_ms: 45_000,
        }
    }
}

impl ConfigMetaReels {
    /// Carga configuración desde variables de entorno (`AURORA_META_*`).
    #[must_use]
    pub fn desde_entorno() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("AURORA_META_IG_USER_ID") {
            if !v.trim().is_empty() {
                cfg.ig_user_id = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("AURORA_META_ACCESS_TOKEN") {
            if !v.trim().is_empty() {
                cfg.access_token = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("AURORA_META_MEDIA_BASE_URL") {
            if !v.trim().is_empty() {
                cfg.media_public_base_url = Some(v.trim().trim_end_matches('/').to_string());
            }
        }
        cfg
    }
}

/// Adaptador oficial de **Meta Graph API** (2 pasos: contenedor `media_type=REELS` + `media_publish`).
pub struct MetaReelsGraphPublisher {
    config: ConfigMetaReels,
    http: Client,
}

impl MetaReelsGraphPublisher {
    /// Construye el cliente HTTP `rustls-tls` para Instagram Reels vía Meta Graph API.
    pub fn new(config: ConfigMetaReels) -> PulseResult<Self> {
        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1_000)))
            .build()
            .map_err(|e| PulseError::DespachoFallido {
                canal: PlataformaPulso::Instagram.etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error creando cliente HTTP Meta Graph: {e}"),
            })?;
        Ok(Self { config, http })
    }
}

#[derive(Debug, Deserialize)]
struct MetaIdResponse {
    id: Option<String>,
}

#[async_trait]
impl OrganicSocialPublisher for MetaReelsGraphPublisher {
    fn plataforma(&self) -> PlataformaPulso {
        PlataformaPulso::Instagram
    }

    fn modo_real_configurado(&self) -> bool {
        self.config
            .ig_user_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            && self
                .config
                .access_token
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }

    fn detalle_configuracion(&self) -> String {
        if self.modo_real_configurado() {
            format!("Meta Graph API lista ({})", self.config.graph_base_url)
        } else {
            "AURORA_META_IG_USER_ID / AURORA_META_ACCESS_TOKEN no configurados (usa respaldo offline)"
                .to_string()
        }
    }

    #[tracing::instrument(skip(self, solicitud), fields(campana_id = %solicitud.campana_id))]
    async fn publicar_reel_organico(
        &self,
        solicitud: &SolicitudPublicacionOrganica,
    ) -> PulseResult<ComprobantePublicacionOrganica> {
        let _bytes = validar_solicitud_organica(solicitud).await?;
        let ig_user_id = self
            .config
            .ig_user_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "falta AURORA_META_IG_USER_ID para Meta Graph API".to_string(),
            })?;
        let token = self
            .config
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "falta AURORA_META_ACCESS_TOKEN para Meta Graph API".to_string(),
            })?;

        let nombre_archivo = solicitud
            .ruta_mp4
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("reel_vertical.mp4");
        let media_base = self
            .config
            .media_public_base_url
            .as_deref()
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "falta AURORA_META_MEDIA_BASE_URL (URL HTTPS pública para ingestión de video_url en Meta Graph)".to_string(),
            })?;
        let video_url = format!("{media_base}/{nombre_archivo}");

        let hashtags = solicitud
            .hashtags
            .iter()
            .map(|h| format!("#{}", h.trim().trim_start_matches('#')))
            .collect::<Vec<_>>()
            .join(" ");
        let caption = format!(
            "{}\n\n{}\n\n{}",
            solicitud.titulo.trim(),
            solicitud.descripcion.trim(),
            hashtags
        )
        .trim()
        .to_string();

        // Paso 1: Crear contenedor media_type=REELS
        let base = self.config.graph_base_url.trim_end_matches('/');
        let create_url = format!("{base}/{ig_user_id}/media");
        let create_resp = self
            .http
            .post(&create_url)
            .form(&[
                ("media_type", "REELS"),
                ("video_url", video_url.as_str()),
                ("caption", caption.as_str()),
                ("access_token", token),
            ])
            .send()
            .await
            .map_err(|e| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error creando contenedor REELS en Meta Graph: {e}"),
            })?;

        if create_resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: self.plataforma().etiqueta().to_string(),
                motivo: "HTTP 429 creando contenedor Meta Graph REELS".to_string(),
                esperar_ms: 60_000,
            });
        }

        if !create_resp.status().is_success() {
            let code = create_resp.status().as_u16();
            let cuerpo = create_resp.text().await.unwrap_or_default();
            return Err(PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("Meta Graph rechazó contenedor REELS (HTTP {code}): {cuerpo}"),
            });
        }

        let created: MetaIdResponse = create_resp.json().await.map_err(|e| {
            PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("JSON inválido al crear contenedor Meta Graph: {e}"),
            }
        })?;
        let creation_id = created.id.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: "Meta Graph no devolvió creation_id".to_string(),
            }
        })?;

        // Paso 2: Publicar contenedor (media_publish)
        let publish_url = format!("{base}/{ig_user_id}/media_publish");
        let publish_resp = self
            .http
            .post(&publish_url)
            .form(&[
                ("creation_id", creation_id.as_str()),
                ("access_token", token),
            ])
            .send()
            .await
            .map_err(|e| PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("error ejecutando media_publish en Meta Graph: {e}"),
            })?;

        if publish_resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: self.plataforma().etiqueta().to_string(),
                motivo: "HTTP 429 en media_publish de Meta Graph".to_string(),
                esperar_ms: 60_000,
            });
        }

        if !publish_resp.status().is_success() {
            let code = publish_resp.status().as_u16();
            let cuerpo = publish_resp.text().await.unwrap_or_default();
            return Err(PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("Meta Graph falló en media_publish (HTTP {code}): {cuerpo}"),
            });
        }

        let published: MetaIdResponse = publish_resp.json().await.map_err(|e| {
            PulseError::DespachoFallido {
                canal: self.plataforma().etiqueta().to_string(),
                tracking_id: None,
                detalle: format!("JSON inválido en media_publish de Meta Graph: {e}"),
            }
        })?;
        let id_remoto = published
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(creation_id);

        Ok(ComprobantePublicacionOrganica {
            publicacion_id: Uuid::new_v4(),
            campana_id: solicitud.campana_id,
            plataforma: PlataformaPulso::Instagram,
            url_publica: format!("https://www.instagram.com/reel/{id_remoto}/"),
            id_remoto,
            sha256_mp4: solicitud.sha256_mp4.to_ascii_lowercase(),
            publicado_en: Utc::now(),
            simulado: false,
        })
    }
}

/// Publicador determinista offline para pruebas CI y modo `SimuladoOffline` / `RealConFallback`.
#[derive(Debug)]
pub struct DeterministicMockSocialPublisher {
    plataforma: PlataformaPulso,
    forzar_429: AtomicBool,
}

impl DeterministicMockSocialPublisher {
    /// Crea un publicador offline exitoso para `plataforma`.
    #[must_use]
    pub const fn new(plataforma: PlataformaPulso) -> Self {
        Self {
            plataforma,
            forzar_429: AtomicBool::new(false),
        }
    }

    /// Activa o desactiva la simulación determinista de HTTP 429 (rate-limit remoto).
    pub fn inyectar_rate_limit_429(&self, activar: bool) {
        self.forzar_429.store(activar, Ordering::SeqCst);
    }
}

#[async_trait]
impl OrganicSocialPublisher for DeterministicMockSocialPublisher {
    fn plataforma(&self) -> PlataformaPulso {
        self.plataforma
    }

    fn modo_real_configurado(&self) -> bool {
        false
    }

    fn detalle_configuracion(&self) -> String {
        format!(
            "Publicador orgánico offline determinista activo ({})",
            self.plataforma.etiqueta()
        )
    }

    async fn publicar_reel_organico(
        &self,
        solicitud: &SolicitudPublicacionOrganica,
    ) -> PulseResult<ComprobantePublicacionOrganica> {
        let _bytes = validar_solicitud_organica(solicitud).await?;

        if self.forzar_429.load(Ordering::SeqCst) {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: self.plataforma.etiqueta().to_string(),
                motivo: "HTTP 429 simulado por DeterministicMockSocialPublisher".to_string(),
                esperar_ms: 60_000,
            });
        }

        let publicacion_id = Uuid::new_v4();
        let prefijo = match self.plataforma {
            PlataformaPulso::YouTubeShorts => "yt_short",
            PlataformaPulso::Instagram => "ig_reel",
            _ => "org_reel",
        };
        let id_remoto = format!("{prefijo}_{}", &publicacion_id.simple().to_string()[..12]);
        let url_publica = match self.plataforma {
            PlataformaPulso::YouTubeShorts => format!("https://youtube.com/shorts/{id_remoto}"),
            PlataformaPulso::Instagram => format!("https://www.instagram.com/reel/{id_remoto}/"),
            _ => format!("https://aurora.local/organico/{id_remoto}"),
        };

        Ok(ComprobantePublicacionOrganica {
            publicacion_id,
            campana_id: solicitud.campana_id,
            plataforma: self.plataforma,
            id_remoto,
            url_publica,
            sha256_mp4: solicitud.sha256_mp4.to_ascii_lowercase(),
            publicado_en: Utc::now(),
            simulado: true,
        })
    }
}

/// Orquestador de publicación orgánica (Carril Verde) gobernado por rate-limiter, circuit breaker
/// y registro automático en [`RecolectorTelemetria`].
pub struct GestorPublicacionOrganica {
    rate_limiter: Arc<ControladorTasaPlataforma>,
    publicadores: HashMap<PlataformaPulso, Arc<dyn OrganicSocialPublisher>>,
    historial: Mutex<Vec<ComprobantePublicacionOrganica>>,
}

impl GestorPublicacionOrganica {
    /// Crea un gestor orgánico enlazado al [`ControladorTasaPlataforma`] compartido.
    #[must_use]
    pub fn new(rate_limiter: Arc<ControladorTasaPlataforma>) -> Self {
        Self {
            rate_limiter,
            publicadores: HashMap::new(),
            historial: Mutex::new(Vec::new()),
        }
    }

    /// Registra un adaptador para una plataforma (`YouTubeShorts`, `Instagram`, etc.).
    pub fn registrar_publicador(&mut self, publicador: Arc<dyn OrganicSocialPublisher>) {
        self.publicadores
            .insert(publicador.plataforma(), publicador);
    }

    /// Variante fluida para encadenar registros.
    #[must_use]
    pub fn con_publicador(mut self, publicador: Arc<dyn OrganicSocialPublisher>) -> Self {
        self.registrar_publicador(publicador);
        self
    }

    /// Devuelve el estado operativo de todos los publicadores registrados.
    #[must_use]
    pub fn estado_publicadores(&self) -> Vec<EstadoPublicadorOrganico> {
        let mut lista: Vec<EstadoPublicadorOrganico> = self
            .publicadores
            .values()
            .map(|p| EstadoPublicadorOrganico {
                plataforma: p.plataforma(),
                modo_real_configurado: p.modo_real_configurado(),
                detalle: p.detalle_configuracion(),
            })
            .collect();
        lista.sort_by_key(|e| e.plataforma.etiqueta());
        lista
    }

    /// Devuelve el historial de publicaciones orgánicas completadas.
    #[must_use]
    pub fn historial(&self) -> Vec<ComprobantePublicacionOrganica> {
        self.historial.lock().clone()
    }

    /// Valida PII/SHA-256, verifica cuotas/circuit breaker, publica y registra en telemetría.
    pub async fn publicar(
        &self,
        plataforma: PlataformaPulso,
        solicitud: &SolicitudPublicacionOrganica,
        telemetria: &RecolectorTelemetria,
    ) -> PulseResult<ComprobantePublicacionOrganica> {
        self.publicar_en(plataforma, solicitud, telemetria, Utc::now())
            .await
    }

    /// Variante determinista de [`Self::publicar`] con reloj explícito `ahora` para pruebas.
    #[tracing::instrument(
        skip(self, solicitud, telemetria),
        fields(plataforma = plataforma.etiqueta(), campana_id = %solicitud.campana_id)
    )]
    pub async fn publicar_en(
        &self,
        plataforma: PlataformaPulso,
        solicitud: &SolicitudPublicacionOrganica,
        telemetria: &RecolectorTelemetria,
        ahora: DateTime<Utc>,
    ) -> PulseResult<ComprobantePublicacionOrganica> {
        let _bytes = validar_solicitud_organica(solicitud).await?;

        let publicador = self.publicadores.get(&plataforma).ok_or_else(|| {
            PulseError::DespachoFallido {
                canal: plataforma.etiqueta().to_string(),
                tracking_id: None,
                detalle: format!(
                    "no hay publicador orgánico registrado para '{}'",
                    plataforma.etiqueta()
                ),
            }
        })?;

        let _jitter_ms = self.rate_limiter.verificar_y_reservar(plataforma, ahora)?;

        match publicador.publicar_reel_organico(solicitud).await {
            Ok(comprobante) => {
                self.rate_limiter.registrar_exito(plataforma);
                self.historial.lock().push(comprobante.clone());

                let evento = EventoTelemetria::nuevo(
                    solicitud.campana_id,
                    Uuid::nil(),
                    comprobante.publicacion_id,
                    Some(isla_desde_vertical(&solicitud.vertical)),
                    solicitud.titulo.clone(),
                    solicitud.descripcion.clone(),
                    TipoEventoTelemetria::ImpresionReel,
                );
                let _ = telemetria.registrar_evento(evento);

                info!(
                    plataforma = plataforma.etiqueta(),
                    id_remoto = %comprobante.id_remoto,
                    simulado = comprobante.simulado,
                    "Reel orgánico (Carril Verde) publicado y trazado en telemetría"
                );
                Ok(comprobante)
            }
            Err(PulseError::RateLimitPlataforma {
                plataforma: plat,
                motivo,
                esperar_ms,
            }) => {
                let fallos = self.rate_limiter.registrar_fallo(plataforma, ahora, true);
                warn!(
                    plataforma = %plat,
                    fallos,
                    motivo = %motivo,
                    "Rate limit 429 en publicación orgánica registrado en circuit breaker"
                );
                Err(PulseError::RateLimitPlataforma {
                    plataforma: plat,
                    motivo,
                    esperar_ms,
                })
            }
            Err(err) => {
                let _ = self.rate_limiter.registrar_fallo(plataforma, ahora, false);
                Err(err)
            }
        }
    }
}

const fn isla_desde_vertical(vertical: &VerticalComercial) -> IslaVertical {
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
