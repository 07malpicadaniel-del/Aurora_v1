use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aurora_domain::{validar_activo, EstadoAprobacion};
use chrono::{Duration as ChronoDuration, Utc};
use lettre::message::header::{ContentType, Header, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::dispatcher::{
    ChannelTransport, ComprobanteDespacho, EstadoCampanaProspecto, OrdenDespacho,
};
use crate::error::{PulseError, PulseResult};
use crate::rate_limiter::{ControladorTasaPlataforma, PlataformaPulso};

#[derive(Debug, Clone)]
struct ListUnsubscribeHeader(String);

impl Header for ListUnsubscribeHeader {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("List-Unsubscribe")
    }

    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.trim().to_string()))
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), self.0.clone())
    }
}

#[derive(Debug, Clone)]
struct ListUnsubscribePostHeader(String);

impl Header for ListUnsubscribePostHeader {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("List-Unsubscribe-Post")
    }

    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.trim().to_string()))
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), self.0.clone())
    }
}

/// Configuración para el adaptador real SMTP (`lettre` sobre `tokio1-rustls-tls`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigSmtp {
    /// Habilita el envío SMTP real. Si es `false`, los envíos reales fallan explícitamente.
    pub habilitado: bool,
    /// Host del servidor SMTP (ej. `smtp.gmail.com`, `smtp.resend.com`, `127.0.0.1`).
    pub host: String,
    /// Puerto SMTP (`587` STARTTLS, `465` TLS implícito o `1025` local).
    pub puerto: u16,
    /// Usuario SMTP opcional.
    pub usuario: Option<String>,
    /// Contraseña o App Password opcional.
    pub password: Option<String>,
    /// Nombre visible del remitente.
    pub remitente_nombre: String,
    /// Dirección de correo del remitente.
    pub remitente_correo: String,
    /// Usa STARTTLS obligatorio (`rustls`).
    pub usar_tls_starttls: bool,
    /// Usa TLS implícito (`rustls`, típico en puerto 465).
    pub usar_tls_implicito: bool,
    /// Endpoint público o local para ejercicio inmediato de baja `opt-out` LFPDPPP.
    pub url_base_opt_out: String,
    /// Adjunta automáticamente el `dossier_pdf` manufacturado.
    pub adjuntar_dossier_pdf: bool,
    /// Tamaño máximo (MiB) para adjuntar el `reel_vertical_mp4` directo; si lo supera, enlaza URL.
    pub adjuntar_reel_si_menor_a_mb: u64,
    /// Timeout por operación SMTP en milisegundos.
    pub timeout_ms: u64,
}

impl Default for ConfigSmtp {
    fn default() -> Self {
        Self {
            habilitado: false,
            host: "127.0.0.1".to_string(),
            puerto: 587,
            usuario: None,
            password: None,
            remitente_nombre: "Aurora Inteligencia Comercial".to_string(),
            remitente_correo: "aurora@localhost.localdomain".to_string(),
            usar_tls_starttls: true,
            usar_tls_implicito: false,
            url_base_opt_out: "http://127.0.0.1:8090/api/v1/lfpdppp/opt-out".to_string(),
            adjuntar_dossier_pdf: true,
            adjuntar_reel_si_menor_a_mb: 12,
            timeout_ms: 15_000,
        }
    }
}

/// Transporte real de correo ejecutivo usando `lettre` + `tokio1-rustls-tls` (sin OpenSSL).
#[derive(Debug, Clone)]
pub struct SmtpEmailTransport {
    config: ConfigSmtp,
    remitente_mailbox: Mailbox,
    rate_limiter: Option<Arc<ControladorTasaPlataforma>>,
}

impl SmtpEmailTransport {
    /// Crea y valida el transporte SMTP comprobando remitente RFC y endpoint LFPDPPP.
    pub fn new(config: ConfigSmtp) -> PulseResult<Self> {
        let addr_remitente = config.remitente_correo.trim().parse().map_err(|e| {
            PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: None,
                detalle: format!(
                    "remitente SMTP inválido '{}': {e}",
                    config.remitente_correo
                ),
            }
        })?;

        let remitente_nombre = config.remitente_nombre.trim();
        let remitente_mailbox = Mailbox::new(
            if remitente_nombre.is_empty() {
                None
            } else {
                Some(remitente_nombre.to_string())
            },
            addr_remitente,
        );

        let url_baja = config.url_base_opt_out.trim();
        if !(url_baja.starts_with("http://") || url_baja.starts_with("https://")) {
            return Err(PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: None,
                detalle: "url_base_opt_out debe iniciar con http:// o https://".to_string(),
            });
        }

        Ok(Self {
            config,
            remitente_mailbox,
            rate_limiter: None,
        })
    }

    /// Asocia un controlador de tasa compartido para consultar/verificar cuotas si se invoca directo.
    #[must_use]
    pub fn con_rate_limiter(mut self, rate_limiter: Arc<ControladorTasaPlataforma>) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }

    /// Devuelve la configuración SMTP activa.
    #[must_use]
    pub const fn config(&self) -> &ConfigSmtp {
        &self.config
    }

    /// Construye el URL verificable de exclusión inmediata LFPDPPP para este envío.
    #[must_use]
    pub fn construir_url_opt_out(&self, envio: &OrdenDespacho) -> String {
        format!(
            "{}?prospecto_id={}&campana_id={}",
            self.config.url_base_opt_out.trim_end_matches('/'),
            envio.prospecto.id,
            envio.campana_id
        )
    }

    /// Resuelve y valida estrictamente el buzón destino desde `correo_destino` o `aliases`.
    pub fn resolver_destinatario(&self, envio: &OrdenDespacho) -> PulseResult<Mailbox> {
        let candidato = envio
            .correo_destino
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                envio.prospecto.aliases.iter().find_map(|alias| {
                    let limpio = alias
                        .trim()
                        .strip_prefix("correo:")
                        .or_else(|| alias.trim().strip_prefix("email:"))
                        .unwrap_or_else(|| alias.trim())
                        .trim();
                    if limpio.contains('@') {
                        Some(limpio)
                    } else {
                        None
                    }
                })
            })
            .ok_or_else(|| PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: Some(envio.tracking_id),
                detalle: format!(
                    "el prospecto '{}' ({}) no cuenta con correo electrónico válido",
                    envio.prospecto.nombre_normalizado, envio.prospecto.id
                ),
            })?;

        let address = candidato.parse().map_err(|e| PulseError::DespachoFallido {
            canal: "CorreoEjecutivo".to_string(),
            tracking_id: Some(envio.tracking_id),
            detalle: format!("dirección de correo destino inválida '{candidato}': {e}"),
        })?;

        Ok(Mailbox::new(
            Some(envio.prospecto.nombre_normalizado.clone()),
            address,
        ))
    }

    /// Construye de forma determinista el mensaje MIME completo con adjunto PDF, enlace al Reel
    /// y cabeceras `List-Unsubscribe` / `List-Unsubscribe-Post` (LFPDPPP).
    pub async fn construir_mensaje_mime(&self, envio: &OrdenDespacho) -> PulseResult<Message> {
        if envio.prospecto.lfpdppp.opt_out {
            return Err(PulseError::ProspectoOptOut {
                prospecto_id: envio.prospecto.id,
            });
        }

        if envio.paquete.activos_actualizados.is_empty() {
            return Err(PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: Some(envio.tracking_id),
                detalle: "el paquete no contiene activos aprobados".to_string(),
            });
        }

        for activo in &envio.paquete.activos_actualizados {
            if activo.estado != EstadoAprobacion::Aprobado {
                return Err(PulseError::NoAprobadoPorGatekeeper {
                    entidad_id: activo.id,
                    prospecto_id: envio.prospecto.id,
                    estado_actual: activo.estado,
                });
            }
            validar_activo(activo)?;
        }

        let destino_mailbox = self.resolver_destinatario(envio)?;
        let url_opt_out = self.construir_url_opt_out(envio);

        let cuerpo_texto = format!(
            "{mensaje}\n\nResumen ejecutivo: {tracking}\nReel vertical personalizado (SHA-256 {reel_sha}): disponible en su dossier.\n\n---\nCumplimiento LFPDPPP: Si desea ejercer su derecho de oposición/baja inmediata, visite: {opt_out}\n",
            mensaje = envio.paquete.mensaje_directo_texto.trim(),
            tracking = envio.url_propuesta_tracking,
            reel_sha = &envio.paquete.reel_vertical_mp4.sha256_hex[..12.min(envio.paquete.reel_vertical_mp4.sha256_hex.len())],
            opt_out = url_opt_out,
        );

        let cuerpo_html = format!(
            "<!DOCTYPE html><html lang=\"es-MX\"><body>\
            <p>{mensaje}</p>\
            <p><strong>Propuesta personalizada:</strong> <a href=\"{tracking}\">{tracking}</a></p>\
            <p>Adjuntamos el Dossier Ejecutivo en PDF y las referencias de resumen multimedia (<code>{reel_sha}</code>).</p>\
            <hr/><p style=\"font-size:12px;color:#555;\">Cumplimiento LFPDPPP: para ejercer su derecho de oposición inmediata haga clic en <a href=\"{opt_out}\">darse de baja</a>.</p>\
            </body></html>",
            mensaje = escapar_html(envio.paquete.mensaje_directo_texto.trim()),
            tracking = escapar_html(&envio.url_propuesta_tracking),
            reel_sha = escapar_html(&envio.paquete.reel_vertical_mp4.sha256_hex),
            opt_out = escapar_html(&url_opt_out),
        );

        let mut mixed = MultiPart::mixed().multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(cuerpo_texto))
                .singlepart(SinglePart::html(cuerpo_html)),
        );

        if self.config.adjuntar_dossier_pdf {
            let pdf_bytes = tokio::fs::read(&envio.paquete.dossier_pdf.ruta_nvme)
                .await
                .map_err(|e| PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: Some(envio.tracking_id),
                    detalle: format!(
                        "no fue posible leer dossier PDF '{}': {e}",
                        envio.paquete.dossier_pdf.ruta_nvme.display()
                    ),
                })?;

            let pdf_ct = ContentType::parse("application/pdf").map_err(|e| {
                PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: Some(envio.tracking_id),
                    detalle: format!("ContentType PDF inválido: {e}"),
                }
            })?;

            let nombre_pdf = format!("Dossier_Aurora_{}.pdf", envio.orden_id);
            mixed = mixed.singlepart(Attachment::new(nombre_pdf).body(pdf_bytes, pdf_ct));
        }

        let max_reel_bytes = self
            .config
            .adjuntar_reel_si_menor_a_mb
            .saturating_mul(1024)
            .saturating_mul(1024);

        if max_reel_bytes > 0
            && envio.paquete.reel_vertical_mp4.tamano_bytes <= max_reel_bytes
            && envio.paquete.reel_vertical_mp4.ruta_nvme.is_file()
        {
            if let Ok(mp4_bytes) = tokio::fs::read(&envio.paquete.reel_vertical_mp4.ruta_nvme).await {
                if let Ok(mp4_ct) = ContentType::parse("video/mp4") {
                    let nombre_mp4 = format!("Reel_Aurora_{}.mp4", envio.orden_id);
                    mixed = mixed.singlepart(Attachment::new(nombre_mp4).body(mp4_bytes, mp4_ct));
                }
            }
        }

        let asunto = format!(
            "Propuesta estratégica para {}",
            envio.prospecto.nombre_normalizado
        );

        Message::builder()
            .from(self.remitente_mailbox.clone())
            .to(destino_mailbox)
            .subject(asunto)
            .header(ListUnsubscribeHeader(format!("<{url_opt_out}>")))
            .header(ListUnsubscribePostHeader(
                "List-Unsubscribe=One-Click".to_string(),
            ))
            .multipart(mixed)
            .map_err(|e| PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: Some(envio.tracking_id),
                detalle: format!("error ensamblando mensaje MIME: {e}"),
            })
    }

    fn construir_transporte(&self) -> PulseResult<AsyncSmtpTransport<Tokio1Executor>> {
        let host = self.config.host.trim();
        if host.is_empty() {
            return Err(PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: None,
                detalle: "el host SMTP no puede estar vacío".to_string(),
            });
        }

        let timeout = Some(Duration::from_millis(self.config.timeout_ms.max(1_000)));
        let mut builder = if self.config.usar_tls_implicito {
            let tls_params =
                TlsParameters::new_rustls(host.to_string()).map_err(|e| PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: None,
                    detalle: format!("error configurando rustls TLS implícito: {e}"),
                })?;
            AsyncSmtpTransport::<Tokio1Executor>::relay(host)
                .map_err(|e| PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: None,
                    detalle: format!("host SMTP inválido '{host}': {e}"),
                })?
                .tls(Tls::Wrapper(tls_params))
        } else if self.config.usar_tls_starttls {
            let tls_params =
                TlsParameters::new_rustls(host.to_string()).map_err(|e| PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: None,
                    detalle: format!("error configurando rustls STARTTLS: {e}"),
                })?;
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
                .map_err(|e| PulseError::DespachoFallido {
                    canal: "CorreoEjecutivo".to_string(),
                    tracking_id: None,
                    detalle: format!("host SMTP STARTTLS inválido '{host}': {e}"),
                })?
                .tls(Tls::Required(tls_params))
        } else {
            warn!(host = %host, "Inicializando transporte SMTP local sin TLS");
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        };

        builder = builder.port(self.config.puerto).timeout(timeout);

        if let (Some(user), Some(pass)) = (&self.config.usuario, &self.config.password) {
            if !user.trim().is_empty() {
                builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
            }
        }

        Ok(builder.build())
    }
}

#[async_trait]
impl ChannelTransport for SmtpEmailTransport {
    #[tracing::instrument(
        skip(self, envio),
        fields(
            tracking_id = %envio.tracking_id,
            prospecto_id = %envio.prospecto.id,
            smtp_host = %self.config.host
        )
    )]
    async fn enviar_paquete(&self, envio: &OrdenDespacho) -> PulseResult<ComprobanteDespacho> {
        let ahora = Utc::now();
        if let Some(ref limiter) = self.rate_limiter {
            let _ = limiter.verificar_y_reservar(PlataformaPulso::CorreoEjecutivo, ahora)?;
        }

        let mensaje = self.construir_mensaje_mime(envio).await?;

        if !self.config.habilitado {
            return Err(PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: Some(envio.tracking_id),
                detalle: "transporte SMTP real deshabilitado en configuración (`smtp.habilitado = false`)"
                    .to_string(),
            });
        }

        let mailer = self.construir_transporte()?;
        let respuesta = mailer.send(mensaje).await.map_err(|e| {
            if let Some(ref limiter) = self.rate_limiter {
                let _ = limiter.registrar_fallo(PlataformaPulso::CorreoEjecutivo, ahora, false);
            }
            PulseError::DespachoFallido {
                canal: "CorreoEjecutivo".to_string(),
                tracking_id: Some(envio.tracking_id),
                detalle: format!("error enviando correo vía SMTP '{}:{}': {e}", self.config.host, self.config.puerto),
            }
        })?;

        if let Some(ref limiter) = self.rate_limiter {
            limiter.registrar_exito(PlataformaPulso::CorreoEjecutivo);
        }

        let codigo = respuesta.code();
        debug!(code = ?codigo, "Correo ejecutivo entregado por SMTP rustls");
        info!(
            tracking_id = %envio.tracking_id,
            prospecto_id = %envio.prospecto.id,
            "Correo ejecutivo despachado con cumplimiento LFPDPPP"
        );

        Ok(ComprobanteDespacho {
            tracking_id: envio.tracking_id,
            orden_id: envio.orden_id,
            campana_id: envio.campana_id,
            prospecto_id: envio.prospecto.id,
            canal: envio.canal,
            estado_campana: EstadoCampanaProspecto::Enviado,
            despachado_en: ahora,
            proximo_seguimiento_en: ahora
                + ChronoDuration::hours(i64::from(envio.cadencia_seguimiento_horas)),
            url_propuesta_tracking: envio.url_propuesta_tracking.clone(),
            referencia_externa: format!("smtp://{}:{}#{:?}", self.config.host, self.config.puerto, codigo),
            activos_despachados: envio.paquete.activos_actualizados.clone(),
        })
    }
}

fn escapar_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_cortex::CanalContacto;
    use aurora_domain::{
        ActivoGenerado, CapacidadAdquisitiva, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
        TipoActivoGenerado, Ubicacion,
    };
    use aurora_factory::{sha256_bytes, MetadatosArtefacto, PaqueteManufacturado};
    use uuid::Uuid;

    fn crear_artefacto_temp(
        dir: &std::path::Path,
        nombre: &str,
        bytes: &[u8],
    ) -> Result<MetadatosArtefacto, Box<dyn std::error::Error>> {
        let ruta = dir.join(nombre);
        std::fs::write(&ruta, bytes)?;
        Ok(MetadatosArtefacto {
            ruta_nvme: ruta,
            sha256_hex: sha256_bytes(bytes),
            fnv1a_64_hex: "0000000000000001".to_string(),
            tamano_bytes: bytes.len() as u64,
            duracion_ms: None,
            tiempo_renderizado_ms: 1,
        })
    }

    fn orden_despacho_prueba(
        dir: &std::path::Path,
        correo_destino: Option<String>,
        opt_out: bool,
    ) -> Result<OrdenDespacho, Box<dyn std::error::Error>> {
        let prospecto_id = Uuid::new_v4();
        let campana_id = Uuid::new_v4();
        let orden_id = Uuid::new_v4();
        let ahora = Utc::now();

        let pdf = crear_artefacto_temp(dir, "dossier.pdf", b"%PDF-1.4\n%AuroraTest\n%%EOF")?;
        let wav = crear_artefacto_temp(dir, "nota.wav", b"RIFFtestWAVEfmt ")?;
        let mp4 = crear_artefacto_temp(dir, "reel.mp4", b"ftypisomtestmp4")?;
        let srt = crear_artefacto_temp(dir, "sub.srt", b"1\n00:00:00,000 --> 00:00:02,000\nHola\n")?;

        let activo = ActivoGenerado {
            id: Uuid::new_v4(),
            campana_id,
            prospecto_id,
            tipo: TipoActivoGenerado::DossierPdf,
            estado: EstadoAprobacion::Aprobado,
            contenido_o_ruta: pdf.ruta_nvme.display().to_string(),
            creado_en: ahora,
            actualizado_en: ahora,
            motivo_rechazo: None,
        };

        Ok(OrdenDespacho {
            tracking_id: Uuid::new_v4(),
            orden_id,
            campana_id,
            prospecto: Prospecto {
                id: prospecto_id,
                nombre_normalizado: "Valeria Garza".to_string(),
                aliases: vec!["Grupo Industrial Regio".to_string()],
                ubicacion: Ubicacion {
                    ciudad: "Monterrey".to_string(),
                    estado: "Nuevo León".to_string(),
                    zona: Some("Valle Oriente".to_string()),
                },
                capacidad_adquisitiva: CapacidadAdquisitiva {
                    ingreso_anual_estimado_mxn: 4_800_000,
                    patrimonio_estimado_mxn: Some(25_000_000),
                },
                perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
                lfpdppp: MetadatosLfpdppp {
                    fuente_origen: "Boletín Público NL".to_string(),
                    url_evidencia: "https://datos.ejemplo.mx/regio".to_string(),
                    capturado_en: ahora,
                    opt_out,
                },
                gatillos: Vec::new(),
            },
            canal: CanalContacto::CorreoEjecutivo,
            paquete: PaqueteManufacturado {
                orden_id,
                campana_id,
                prospecto_id,
                directorio_salida: dir.to_path_buf(),
                dossier_pdf: pdf,
                nota_voz_wav: wav,
                reel_vertical_mp4: mp4,
                subtitulos_srt: srt,
                mensaje_directo_texto: "Hola Valeria, comparto análisis financiero.".to_string(),
                activos_actualizados: vec![activo],
                modo_video: aurora_factory::ModoComposicionVideo::DiapositivasEstaticas,
                clips_broll_usados: Vec::new(),
                generado_en: ahora,
            },
            url_propuesta_tracking: format!("https://aurora.local/propuesta/{prospecto_id}"),
            cadencia_seguimiento_horas: 48,
            correo_destino,
        })
    }

    #[tokio::test]
    async fn smtp_transport_construye_mime_con_pdf_y_cabecera_opt_out_y_rechaza_invalidos(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("aurora_smtp_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;

        let transport = SmtpEmailTransport::new(ConfigSmtp::default())?;

        // 1. Caso válido: incluye adjunto PDF y cabeceras List-Unsubscribe LFPDPPP
        let orden_ok = orden_despacho_prueba(&dir, Some("valeria@regio.mx".to_string()), false)?;
        let msg = transport.construir_mensaje_mime(&orden_ok).await?;
        let raw = String::from_utf8_lossy(&msg.formatted()).to_string();
        assert!(raw.contains("List-Unsubscribe:"));
        assert!(raw.contains(&orden_ok.prospecto.id.to_string()));
        assert!(raw.contains("application/pdf"));
        assert!(raw.contains("Dossier_Aurora_"));

        // 2. Sin correo válido debe fallar explícitamente
        let orden_sin_correo = orden_despacho_prueba(&dir, Some("correo-invalido".to_string()), false)?;
        assert!(transport.construir_mensaje_mime(&orden_sin_correo).await.is_err());

        let orden_vacio = orden_despacho_prueba(&dir, None, false)?;
        assert!(transport.construir_mensaje_mime(&orden_vacio).await.is_err());

        // 3. Con opt_out = true debe bloquear LFPDPPP
        let orden_opt_out =
            orden_despacho_prueba(&dir, Some("valeria@regio.mx".to_string()), true)?;
        assert!(matches!(
            transport.construir_mensaje_mime(&orden_opt_out).await,
            Err(PulseError::ProspectoOptOut { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
