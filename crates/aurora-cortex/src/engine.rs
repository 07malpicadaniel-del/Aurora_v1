use std::time::Duration;

use async_trait::async_trait;
use aurora_domain::{
    PerfilPsicologico, SubverticalInmobiliaria, SubverticalSeguros, VerticalComercial,
};
use aurora_memory::{MotorMemoriaAurora, SubgrafoProspecto};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{CortexError, CortexResult};
use crate::grammar::{
    gbnf_ajuste_auditor, gbnf_brief_creativo, gbnf_diagnostico_comercial, gbnf_tool_call,
    parsear_respuesta_estricta, validar_sintaxis_gbnf,
};
use crate::roles::{
    construir_contexto_auditor, construir_contexto_director_creativo,
    construir_contexto_perfilador, AjusteEstrategiaAuditor, BriefCreativo, CanalContacto,
    DiagnosticoComercial, EscenaReel, MetricasCampana, RolCognitivo,
};
use crate::tools::{
    AuroraToolCall, OrdenProduccionActivo, ResultadoEjecucionTool, ToolDispatcher,
};

/// Puerto por defecto de `llama-server` en `bob_engine/cortex`.
pub const DEFAULT_BOB_URL: &str = "http://127.0.0.1:8085";

/// Contrato único de inferencia restringida por rol y gramática GBNF.
///
/// Refleja la restricción de presupuesto VRAM: un solo modelo residente en los
/// 16 GB de la RTX 5060 Ti que alterna roles cambiando prompt de sistema y GBNF.
#[async_trait]
pub trait BobInferenceBackend: Send + Sync {
    /// Ejecuta inferencia estructurada bajo un rol cognitivo y una gramática GBNF estricta.
    async fn inferir_estructurado(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        gramatica_gbnf: &str,
    ) -> CortexResult<String>;
}

/// Configuración HTTP hacia `llama-server` local (`bob_engine/cortex`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigHttpBob {
    /// URL base de `llama-server` (por defecto `http://127.0.0.1:8085`).
    pub url_base: String,
    /// Ranura única en VRAM (`-np 1` => `slot_id = 0`).
    pub slot_id: i32,
    /// Límite de tokens de salida por inferencia.
    pub n_predict: u32,
    /// Timeout por intento HTTP en milisegundos.
    pub timeout_ms: u64,
    /// Reintentos acotados ante fallos transitorios de conexión o 5xx.
    pub max_reintentos: u32,
    /// Reutilizar KV Cache del prefijo de rol en `llama-server`.
    pub cache_prompt: bool,
}

impl Default for ConfigHttpBob {
    fn default() -> Self {
        Self {
            url_base: DEFAULT_BOB_URL.to_string(),
            slot_id: 0,
            n_predict: 2048,
            timeout_ms: 60_000,
            max_reintentos: 2,
            cache_prompt: true,
        }
    }
}

#[derive(Debug, Serialize)]
struct LlamaCompletionPayload<'a> {
    prompt: String,
    n_predict: u32,
    temperature: f32,
    id_slot: i32,
    cache_prompt: bool,
    stream: bool,
    grammar: &'a str,
}

#[derive(Debug, Deserialize)]
struct LlamaCompletionResponse {
    content: Option<String>,
    choices: Option<Vec<OpenAiCompatChoice>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatChoice {
    message: Option<OpenAiCompatMessage>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatMessage {
    content: Option<String>,
}

/// Cliente HTTP asíncrono (`reqwest` + `rustls-tls`) hacia el proceso único de `llama-server`.
#[derive(Debug, Clone)]
pub struct HttpBobCortexClient {
    config: ConfigHttpBob,
    http: Client,
}

impl HttpBobCortexClient {
    /// Construye el cliente HTTP validando URL base y configurando timeouts explícitos.
    pub fn new(config: ConfigHttpBob) -> CortexResult<Self> {
        let base = config.url_base.trim();
        if base.is_empty() || (!base.starts_with("http://") && !base.starts_with("https://")) {
            return Err(CortexError::ConfiguracionCliente(format!(
                "url_base inválida para BOB: '{}'",
                config.url_base
            )));
        }

        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(100)))
            .build()
            .map_err(|e| CortexError::ConfiguracionCliente(e.to_string()))?;

        Ok(Self { config, http })
    }

    /// Devuelve la configuración activa del cliente BOB.
    #[must_use]
    pub const fn config(&self) -> &ConfigHttpBob {
        &self.config
    }

    fn construir_prompt_completo(rol: RolCognitivo, contexto_usuario: &str) -> String {
        format!(
            "<|im_start|>system\n{}\n<|im_end|>\n<|im_start|>user\n{}\n<|im_end|>\n<|im_start|>assistant\n",
            rol.system_prompt(),
            contexto_usuario.trim()
        )
    }

    fn endpoint_completion(&self) -> String {
        let base = self.config.url_base.trim_end_matches('/');
        if base.ends_with("/completion") {
            base.to_string()
        } else {
            format!("{base}/completion")
        }
    }
}

#[async_trait]
impl BobInferenceBackend for HttpBobCortexClient {
    #[tracing::instrument(skip(self, contexto_usuario, gramatica_gbnf), fields(rol = rol.etiqueta(), slot_id = self.config.slot_id))]
    async fn inferir_estructurado(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        gramatica_gbnf: &str,
    ) -> CortexResult<String> {
        validar_sintaxis_gbnf(gramatica_gbnf)?;

        let url = self.endpoint_completion();
        let prompt = Self::construir_prompt_completo(rol, contexto_usuario);
        let payload = LlamaCompletionPayload {
            prompt,
            n_predict: self.config.n_predict,
            temperature: rol.temperatura_sugerida(),
            id_slot: self.config.slot_id,
            cache_prompt: self.config.cache_prompt,
            stream: false,
            grammar: gramatica_gbnf,
        };

        let total_intentos = self.config.max_reintentos.saturating_add(1);
        for intento in 1..=total_intentos {
            debug!(intento, total_intentos, url = %url, "Enviando inferencia GBNF a llama-server");

            match self.http.post(&url).json(&payload).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let cuerpo = resp.text().await.map_err(|e| CortexError::ConexionBob {
                            url: url.clone(),
                            source: e,
                        })?;
                        let parsed: LlamaCompletionResponse =
                            serde_json::from_str(&cuerpo).map_err(|e| {
                                CortexError::RespuestaHttpInvalida {
                                    url: url.clone(),
                                    codigo: status.as_u16(),
                                    detalle: format!("JSON inesperado de llama-server: {e}"),
                                }
                            })?;

                        let contenido = extraer_contenido_respuesta(parsed).ok_or_else(|| {
                            CortexError::RespuestaHttpInvalida {
                                url: url.clone(),
                                codigo: status.as_u16(),
                                detalle: "llama-server respondió sin campo 'content'".to_string(),
                            }
                        })?;

                        return Ok(contenido);
                    }

                    let codigo = status.as_u16();
                    let detalle = resp.text().await.unwrap_or_default();
                    let es_reintentable = status.is_server_error() || codigo == 429;
                    if !es_reintentable || intento == total_intentos {
                        return Err(CortexError::RespuestaHttpInvalida {
                            url,
                            codigo,
                            detalle,
                        });
                    }
                    warn!(intento, codigo, "Respuesta HTTP reintentable desde BOB");
                }
                Err(err) => {
                    let es_timeout = err.is_timeout();
                    if intento == total_intentos {
                        if es_timeout {
                            return Err(CortexError::TimeoutInferencia {
                                intentos: intento,
                                timeout_ms: self.config.timeout_ms,
                            });
                        }
                        return Err(CortexError::ConexionBob { url, source: err });
                    }
                    warn!(intento, error = %err, "Fallo transitorio comunicando con BOB");
                }
            }

            let espera_ms = 50_u64.saturating_mul(1_u64 << (intento - 1).min(5));
            tokio::time::sleep(Duration::from_millis(espera_ms)).await;
        }

        Err(CortexError::TimeoutInferencia {
            intentos: total_intentos,
            timeout_ms: self.config.timeout_ms,
        })
    }
}

/// Normaliza salidas de `llama-server` (Qwen ChatML y Gemma `<end_of_turn>`),
/// eliminando tokens de fin y bloques `<think>...</think>` si van seguidos del JSON útil.
fn limpiar_salida_qwen(raw: &str) -> String {
    let sin_tokens = raw
        .replace("<|im_end|>", "")
        .replace("<|endoftext|>", "")
        .replace("<end_of_turn>", "")
        .replace("<eos>", "");
    if let Some(pos_cierre) = sin_tokens.rfind("</think>") {
        let antes = sin_tokens[..pos_cierre].trim_start();
        let resto = sin_tokens[pos_cierre + "</think>".len()..].trim();
        if (antes.starts_with("<think>") || !antes.starts_with('{')) && !resto.is_empty() {
            return resto.to_string();
        }
    }
    sin_tokens.trim().to_string()
}

fn extraer_contenido_respuesta(resp: LlamaCompletionResponse) -> Option<String> {
    let crudo = extraer_contenido_crudo(resp)?;
    let limpio = limpiar_salida_qwen(&crudo);
    if limpio.is_empty() {
        None
    } else {
        Some(limpio)
    }
}

#[cfg(test)]
mod qwen_tests {
    use super::*;

    #[test]
    fn limpia_bloques_think_y_tokens_chatml_de_qwen() {
        let salida = "<think>Analizando perfil del corredor Veracruz.</think>\n{\"nivel_encaje_0_100\":91}<|im_end|>";
        assert_eq!(
            limpiar_salida_qwen(salida),
            "{\"nivel_encaje_0_100\":91}"
        );

        let sin_apertura = "pensamiento prefill\n</think>\n{\"ok\":true}<|endoftext|>";
        assert_eq!(limpiar_salida_qwen(sin_apertura), "{\"ok\":true}");

        let json_puro = "{\"texto\":\"literal </think> interno\"}";
        assert_eq!(limpiar_salida_qwen(json_puro), json_puro);

        let gemma_fin = "{\"ok\":true}<end_of_turn><eos>";
        assert_eq!(limpiar_salida_qwen(gemma_fin), "{\"ok\":true}");
        assert_eq!(ConfigHttpBob::default().timeout_ms, 60_000);
    }
}

fn extraer_contenido_crudo(resp: LlamaCompletionResponse) -> Option<String> {
    if let Some(content) = resp.content {
        if !content.trim().is_empty() {
            return Some(content);
        }
    }
    if let Some(choices) = resp.choices {
        for choice in choices {
            if let Some(msg) = choice.message {
                if let Some(content) = msg.content {
                    if !content.trim().is_empty() {
                        return Some(content);
                    }
                }
            }
            if let Some(text) = choice.text {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// Backend determinista offline para pruebas CI/CD y validación de contratos sin VRAM.
///
/// Inspecciona el contexto estructurado en español para generar JSON válido conforme
/// a las gramáticas GBNF y diferenciado según [`PerfilPsicologico`] y [`RolCognitivo`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicMockBob;

#[async_trait]
impl BobInferenceBackend for DeterministicMockBob {
    async fn inferir_estructurado(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        gramatica_gbnf: &str,
    ) -> CortexResult<String> {
        validar_sintaxis_gbnf(gramatica_gbnf)?;

        match rol {
            RolCognitivo::Perfilador => {
                let diag = construir_diagnostico_determinista(contexto_usuario);
                serde_json::to_string(&diag).map_err(|e| CortexError::ViolacionEsquema {
                    rol: rol.etiqueta().to_string(),
                    detalle: e.to_string(),
                })
            }
            RolCognitivo::DirectorCreativo => {
                let brief = construir_brief_determinista(contexto_usuario);
                serde_json::to_string(&brief).map_err(|e| CortexError::ViolacionEsquema {
                    rol: rol.etiqueta().to_string(),
                    detalle: e.to_string(),
                })
            }
            RolCognitivo::Auditor => {
                let ajuste = construir_ajuste_determinista(contexto_usuario);
                serde_json::to_string(&ajuste).map_err(|e| CortexError::ViolacionEsquema {
                    rol: rol.etiqueta().to_string(),
                    detalle: e.to_string(),
                })
            }
        }
    }
}

fn extraer_campo<'a>(contexto: &'a str, clave: &str) -> Option<&'a str> {
    let prefijo = format!("{clave}:");
    for linea in contexto.lines() {
        let trimmed = linea.trim();
        if let Some(resto) = trimmed.strip_prefix(&prefijo) {
            return Some(resto.trim());
        }
    }
    None
}

fn extraer_uuid(contexto: &str, clave: &str) -> Uuid {
    extraer_campo(contexto, clave)
        .and_then(|v| Uuid::parse_str(v).ok())
        .unwrap_or_else(Uuid::new_v4)
}

fn detectar_perfil_psicologico(contexto: &str) -> PerfilPsicologico {
    match extraer_campo(contexto, "perfil_psicologico") {
        Some("DinamicoVisual") => PerfilPsicologico::DinamicoVisual,
        Some("ConservadorPatrimonial") => PerfilPsicologico::ConservadorPatrimonial,
        _ => PerfilPsicologico::AnaliticoFinanciero,
    }
}

fn construir_diagnostico_determinista(contexto: &str) -> DiagnosticoComercial {
    let prospecto_id = extraer_uuid(contexto, "prospecto_id");
    let ubicacion = extraer_campo(contexto, "ubicacion").unwrap_or("México");
    let lower = contexto.to_lowercase();
    let es_seguros = extraer_campo(contexto, "isla_actual")
        .is_some_and(|isla| isla.starts_with("seguros"))
        || lower.contains("hombre clave")
        || lower.contains("gmm")
        || lower.contains("póliza")
        || lower.contains("poliza");

    let (vertical_optima, producto_gancho, dolor_principal) = if es_seguros {
        (
            VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
            "Seguro Hombre Clave y Blindaje Directivo Deducible (Art. 27 LISR)".to_string(),
            format!(
                "Exposición fiscal y vulnerabilidad operativa ante ausencia de socio estratégico en {ubicacion}"
            ),
        )
    } else if lower.contains("bodega")
        || lower.contains("logístic")
        || lower.contains("logistic")
        || lower.contains("industrial")
        || lower.contains("nave")
        || lower.contains("parque")
    {
        (
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
            "Nave Industrial AAA con Esquema Sale & Leaseback y ROI 14.8%".to_string(),
            format!(
                "Inmovilización de capital de trabajo y cuello de botella logístico en {ubicacion}"
            ),
        )
    } else {
        (
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa),
            "Preventa Patrimonial Plus con Plusvalía Proyectada de 18% Anual".to_string(),
            format!("Pérdida de poder adquisitivo frente a inflación inmobiliaria en {ubicacion}"),
        )
    };

    let perfil = detectar_perfil_psicologico(contexto);
    let angulo_entrada = match perfil {
        PerfilPsicologico::AnaliticoFinanciero => {
            "Modelo financiero comparativo con TIR neta, flujo libre descontado y escudo fiscal"
                .to_string()
        }
        PerfilPsicologico::DinamicoVisual => {
            "Demo visual rápida con recorrido en video vertical y caso de implementación ágil"
                .to_string()
        }
        PerfilPsicologico::ConservadorPatrimonial => {
            "Dictamen de certidumbre jurídica, fideicomiso de garantía y preservación de capital"
                .to_string()
        }
    };

    let boost_detectado = contexto.contains("boost_circulo=0.000");
    let probabilidad_cierre = if boost_detectado { 0.74 } else { 0.86 };

    let justificacion_topologica = format!(
        "Gatillo vigente detectado en {ubicacion} ({}) con alineación de perfil {} en telaraña 4D",
        if es_seguros {
            "cobertura corporativa"
        } else {
            "expansión inmobiliaria"
        },
        perfil.como_str()
    );

    DiagnosticoComercial {
        prospecto_id,
        vertical_optima,
        producto_gancho,
        dolor_principal,
        angulo_entrada,
        probabilidad_cierre,
        justificacion_topologica,
    }
}

fn construir_brief_determinista(contexto: &str) -> BriefCreativo {
    let prospecto_id = extraer_uuid(contexto, "prospecto_id");
    let perfil = detectar_perfil_psicologico(contexto);
    let ciudad = extraer_campo(contexto, "ubicacion").unwrap_or("México");
    let producto =
        extraer_campo(contexto, "producto_gancho").unwrap_or("Portafolio Estratégico Aurora");
    let dolor = extraer_campo(contexto, "dolor_principal").unwrap_or("optimización de capital");

    match perfil {
        PerfilPsicologico::AnaliticoFinanciero => BriefCreativo {
            prospecto_id,
            perfil_psicologico: perfil,
            gancho_3s: "ROI proyectado del 14.8% anual y deducibilidad fiscal inmediata Art. 27 LISR".to_string(),
            tono_narrativo: "Analítico, cuantitativo y orientado a métricas verificables de TIR, flujo libre y rendimiento neto".to_string(),
            guion_tts: format!(
                "Analizando la oportunidad en {ciudad}, estructuramos {producto} para resolver {dolor} con TIR anual proyectada de 14.8% y eficiencia fiscal documentada."
            ),
            escenas_reel: vec![
                EscenaReel {
                    indice: 1,
                    duracion_segundos: 4,
                    descripcion_visual: "Gráfica comparativa limpia de TIR neta vs CETES e inflación".to_string(),
                    texto_overlay: "ROI 14.8% Anual | TIR Real".to_string(),
                },
                EscenaReel {
                    indice: 2,
                    duracion_segundos: 6,
                    descripcion_visual: "Desglose numérico de escudo fiscal y ahorro acumulado en MXN".to_string(),
                    texto_overlay: "Deducibilidad Art. 27 LISR".to_string(),
                },
                EscenaReel {
                    indice: 3,
                    duracion_segundos: 5,
                    descripcion_visual: "Tabla resumen de escenarios optimista, base y conservador".to_string(),
                    texto_overlay: "Solicita corrida financiera".to_string(),
                },
            ],
            secciones_dossier_pdf: vec![
                "Resumen Ejecutivo y Corrida Financiera (TIR / VPN)".to_string(),
                "Análisis de Deducibilidad Fiscal y Flujo Neto en MXN".to_string(),
                "Comparativo Topológico de Mercado y Ruta de Cierre".to_string(),
            ],
            canal_sugerido: CanalContacto::CorreoEjecutivo,
        },
        PerfilPsicologico::DinamicoVisual => BriefCreativo {
            prospecto_id,
            perfil_psicologico: perfil,
            gancho_3s: "Cortes rápidos en 30 segundos: visualiza tu expansión llave en mano hoy mismo".to_string(),
            tono_narrativo: "Dinámico, visual, ritmo ágil con cortes rápidos de alto impacto cinematográfico".to_string(),
            guion_tts: format!(
                "En {ciudad}, acelera tu expansión sin fricción: mira cómo {producto} resuelve {dolor} en tiempo récord."
            ),
            escenas_reel: vec![
                EscenaReel {
                    indice: 1,
                    duracion_segundos: 2,
                    descripcion_visual: "Corte rápido aéreo con dron sobre corredor industrial iluminado".to_string(),
                    texto_overlay: "Expansión en 30s".to_string(),
                },
                EscenaReel {
                    indice: 2,
                    duracion_segundos: 2,
                    descripcion_visual: "Transición rápida whip-pan hacia render 3D interactivo de nave AAA".to_string(),
                    texto_overlay: "Corte rápido: Llave en mano".to_string(),
                },
                EscenaReel {
                    indice: 3,
                    duracion_segundos: 3,
                    descripcion_visual: "Montaje acelerado de operación logística activa y señalética moderna".to_string(),
                    texto_overlay: "Operación inmediata".to_string(),
                },
                EscenaReel {
                    indice: 4,
                    duracion_segundos: 2,
                    descripcion_visual: "Cierre visual limpio con tarjeta ejecutiva animada".to_string(),
                    texto_overlay: "Agenda recorrido VIP".to_string(),
                },
            ],
            secciones_dossier_pdf: vec![
                "Panorama Visual de Alto Impacto y Ubicación Estratégica".to_string(),
                "Galería de Renders y Especificaciones Clave".to_string(),
                "Activación Rápida en 3 Pasos".to_string(),
            ],
            canal_sugerido: CanalContacto::WhatsApp,
        },
        PerfilPsicologico::ConservadorPatrimonial => BriefCreativo {
            prospecto_id,
            perfil_psicologico: perfil,
            gancho_3s: "Blinda tu patrimonio corporativo con certeza jurídica y respaldo fiduciario".to_string(),
            tono_narrativo: "Sobrio, institucional y enfocado en preservación patrimonial y estabilidad legal".to_string(),
            guion_tts: format!(
                "Para proteger el crecimiento en {ciudad}, diseñamos {producto} garantizando estabilidad patrimonial ante {dolor}."
            ),
            escenas_reel: vec![
                EscenaReel {
                    indice: 1,
                    duracion_segundos: 5,
                    descripcion_visual: "Toma arquitectónica estable con iluminación sobria institucional".to_string(),
                    texto_overlay: "Certeza Jurídica Total".to_string(),
                },
                EscenaReel {
                    indice: 2,
                    duracion_segundos: 5,
                    descripcion_visual: "Esquema claro de blindaje fiduciario y continuidad patrimonial".to_string(),
                    texto_overlay: "Respaldo Fiduciario".to_string(),
                },
            ],
            secciones_dossier_pdf: vec![
                "Dictamen de Certeza Jurídica y Estructura Fiduciaria".to_string(),
                "Matriz de Preservación Patrimonial y Riesgos".to_string(),
                "Protocolo Ejecutivo de Implementación Gradual".to_string(),
            ],
            canal_sugerido: CanalContacto::LinkedIn,
        },
    }
}

fn construir_ajuste_determinista(contexto: &str) -> AjusteEstrategiaAuditor {
    let campana_id = extraer_uuid(contexto, "campana_id");
    let tasa_apertura: f32 = extraer_campo(contexto, "tasa_apertura")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.25);
    let tasa_respuesta: f32 = extraer_campo(contexto, "tasa_respuesta")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.08);

    let requiere_pausa = tasa_respuesta < 0.04 || tasa_apertura < 0.12;
    AjusteEstrategiaAuditor {
        campana_id,
        factor_ajuste_urgencia: if requiere_pausa { 0.75 } else { 1.15 },
        nuevo_gancho_sugerido: if requiere_pausa {
            "Benchmark sectorial con ahorro neto comprobado en primeros 90 días".to_string()
        } else {
            "Caso de referencia en Círculo Cercano con impacto patrimonial inmediato".to_string()
        },
        diagnostico_rendimiento: format!(
            "Apertura observada {:.1}% y respuesta {:.1}% procesadas por Auditor GBNF",
            tasa_apertura * 100.0,
            tasa_respuesta * 100.0
        ),
        requiere_pausa_vertical: requiere_pausa,
    }
}

/// Plan completo emitido por Aurora Cortex combinando perfilamiento, brief creativo y orden Gatekeeper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanAtaqueComercial {
    /// Identificador canónico del prospecto.
    pub prospecto_id: Uuid,
    /// Identificador de la campaña asociada.
    pub campana_id: Uuid,
    /// Diagnóstico topológico emitido por el rol `Perfilador`.
    pub diagnostico: DiagnosticoComercial,
    /// Brief multimodal adaptado al `PerfilPsicologico` por `DirectorCreativo`.
    pub brief_creativo: BriefCreativo,
    /// Orden lista para `aurora-factory`, siempre en `PendienteGatekeeper`.
    pub orden_produccion: OrdenProduccionActivo,
    /// Marca temporal UTC de generación del plan.
    pub generado_en: DateTime<Utc>,
}

/// Orquestador cognitivo central que coordina roles sobre un único modelo BOB y Gatekeeper.
#[derive(Debug, Clone)]
pub struct OrquestadorComercialBob<B: BobInferenceBackend> {
    backend: B,
    dispatcher: ToolDispatcher,
}

impl<B: BobInferenceBackend> OrquestadorComercialBob<B> {
    /// Crea un orquestador sobre el backend de inferencia indicado (`HttpBobCortexClient` o `DeterministicMockBob`).
    #[must_use]
    pub const fn new(backend: B) -> Self {
        Self {
            backend,
            dispatcher: ToolDispatcher,
        }
    }

    /// Devuelve una referencia al backend de inferencia.
    #[must_use]
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Ejecuta el ciclo `Perfilador -> DirectorCreativo -> ToolDispatcher` para generar un plan listo para Gatekeeper.
    #[tracing::instrument(skip(self, subgrafo, memoria), fields(prospecto_id = %subgrafo.prospecto.id, campana_id = %campana_id))]
    pub async fn generar_plan_ataque(
        &self,
        subgrafo: &SubgrafoProspecto,
        campana_id: Uuid,
        memoria: &MotorMemoriaAurora,
    ) -> CortexResult<PlanAtaqueComercial> {
        let prospecto_id = subgrafo.prospecto.id;

        // Barrera LFPDPPP previa a cualquier consumo de VRAM en BOB.
        ToolDispatcher::verificar_habilitacion_lfpdppp(prospecto_id, memoria)?;

        // 1. Rol Perfilador (misma VRAM, prompt + GBNF de diagnóstico)
        let ctx_perfilador = construir_contexto_perfilador(subgrafo);
        let gramatica_diag = gbnf_diagnostico_comercial();
        let raw_diag = self
            .backend
            .inferir_estructurado(RolCognitivo::Perfilador, &ctx_perfilador, &gramatica_diag)
            .await?;
        let diagnostico: DiagnosticoComercial =
            parsear_respuesta_estricta(&raw_diag, RolCognitivo::Perfilador)?;
        if diagnostico.prospecto_id != prospecto_id {
            return Err(CortexError::ViolacionEsquema {
                rol: RolCognitivo::Perfilador.etiqueta().to_string(),
                detalle: format!(
                    "prospecto_id devuelto ({}) no coincide con el subgrafo ({})",
                    diagnostico.prospecto_id, prospecto_id
                ),
            });
        }

        let tool_diag = AuroraToolCall::EmitirDiagnosticoComercial {
            prospecto_id,
            vertical: diagnostico.vertical_optima,
            producto_gancho: diagnostico.producto_gancho.clone(),
            dolor_principal: diagnostico.dolor_principal.clone(),
            probabilidad_cierre: diagnostico.probabilidad_cierre,
            justificacion_topologica: diagnostico.justificacion_topologica.clone(),
        };
        let _ = self.dispatcher.ejecutar(&tool_diag, campana_id, memoria)?;

        // 2. Rol DirectorCreativo (reutiliza mismo modelo en VRAM con cambio de contexto y GBNF)
        let ctx_creativo = construir_contexto_director_creativo(subgrafo, &diagnostico);
        let gramatica_brief = gbnf_brief_creativo();
        let raw_brief = self
            .backend
            .inferir_estructurado(RolCognitivo::DirectorCreativo, &ctx_creativo, &gramatica_brief)
            .await?;
        let brief_creativo: BriefCreativo =
            parsear_respuesta_estricta(&raw_brief, RolCognitivo::DirectorCreativo)?;
        if brief_creativo.prospecto_id != prospecto_id {
            return Err(CortexError::ViolacionEsquema {
                rol: RolCognitivo::DirectorCreativo.etiqueta().to_string(),
                detalle: format!(
                    "prospecto_id devuelto ({}) no coincide con el subgrafo ({})",
                    brief_creativo.prospecto_id, prospecto_id
                ),
            });
        }

        // 3. Despacho determinista de orden multimodal en estado PendienteGatekeeper
        let tool_paquete = AuroraToolCall::SolicitarPaqueteMultimodal {
            prospecto_id,
            gancho_3s: brief_creativo.gancho_3s.clone(),
            guion_tts: brief_creativo.guion_tts.clone(),
            escenas_reel: brief_creativo.escenas_reel.clone(),
            secciones_dossier_pdf: brief_creativo.secciones_dossier_pdf.clone(),
            canal_sugerido: brief_creativo.canal_sugerido,
        };
        let resultado = self
            .dispatcher
            .ejecutar(&tool_paquete, campana_id, memoria)?;
        let orden_produccion = match resultado {
            ResultadoEjecucionTool::OrdenProduccion(orden) => *orden,
            _ => {
                return Err(CortexError::HerramientaNoAutorizada {
                    herramienta: "SolicitarPaqueteMultimodal".to_string(),
                    motivo: "el dispatcher devolvió un variante incompatible".to_string(),
                });
            }
        };
        orden_produccion.verificar_invariante_gatekeeper()?;

        info!(
            prospecto_id = %prospecto_id,
            campana_id = %campana_id,
            orden_id = %orden_produccion.orden_id,
            "PlanAtaqueComercial generado exitosamente en PendienteGatekeeper"
        );

        Ok(PlanAtaqueComercial {
            prospecto_id,
            campana_id,
            diagnostico,
            brief_creativo,
            orden_produccion,
            generado_en: Utc::now(),
        })
    }

    /// Invoca directamente una herramienta vía GBNF `gbnf_tool_call` y la despacha de forma segura.
    #[tracing::instrument(skip(self, contexto_usuario, memoria), fields(campana_id = %campana_id))]
    pub async fn ejecutar_tool_desde_modelo(
        &self,
        rol: RolCognitivo,
        contexto_usuario: &str,
        campana_id: Uuid,
        memoria: &MotorMemoriaAurora,
    ) -> CortexResult<ResultadoEjecucionTool> {
        let gramatica = gbnf_tool_call();
        let raw = self
            .backend
            .inferir_estructurado(rol, contexto_usuario, &gramatica)
            .await?;
        let llamado: AuroraToolCall = parsear_respuesta_estricta(&raw, rol)?;
        self.dispatcher.ejecutar(&llamado, campana_id, memoria)
    }

    /// Ejecuta el rol `Auditor` sobre métricas agregadas para calibrar futuras estrategias.
    #[tracing::instrument(skip(self, metricas), fields(campana_id = %metricas.campana_id))]
    pub async fn auditar_campana(
        &self,
        metricas: &MetricasCampana,
    ) -> CortexResult<AjusteEstrategiaAuditor> {
        for (nombre, valor) in [
            ("tasa_apertura", metricas.tasa_apertura),
            ("tasa_retencion_3s", metricas.tasa_retencion_3s),
            ("tasa_respuesta", metricas.tasa_respuesta),
        ] {
            if !valor.is_finite() || !(0.0..=1.0).contains(&valor) {
                return Err(CortexError::ViolacionEsquema {
                    rol: RolCognitivo::Auditor.etiqueta().to_string(),
                    detalle: format!("{nombre}={valor} fuera de rango [0.0, 1.0]"),
                });
            }
        }

        let ctx = construir_contexto_auditor(metricas);
        let gramatica = gbnf_ajuste_auditor();
        let raw = self
            .backend
            .inferir_estructurado(RolCognitivo::Auditor, &ctx, &gramatica)
            .await?;
        let ajuste: AjusteEstrategiaAuditor =
            parsear_respuesta_estricta(&raw, RolCognitivo::Auditor)?;
        if ajuste.campana_id != metricas.campana_id {
            return Err(CortexError::ViolacionEsquema {
                rol: RolCognitivo::Auditor.etiqueta().to_string(),
                detalle: format!(
                    "campana_id devuelto ({}) no coincide con la campaña auditada ({})",
                    ajuste.campana_id, metricas.campana_id
                ),
            });
        }
        Ok(ajuste)
    }
}
