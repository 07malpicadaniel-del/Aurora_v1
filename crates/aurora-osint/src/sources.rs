use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aurora_domain::{Ubicacion, VerticalComercial};
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info_span, warn, Instrument};

use crate::error::{OsintError, OsintResult};

const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (X11; Linux x86_64) AuroraOSINT/0.1 (+https://github.com/07malpicadaniel-del/Aurora_v1)",
    "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0 AuroraResearch/0.1",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
];

/// Puerto desacoplado para fuentes que exijan renderizado JavaScript vía CDP en fases posteriores
/// (compatible con `bob_engine/router/src/browser_cdp.rs`).
#[async_trait]
pub trait DynamicBrowserFetcher: Send + Sync {
    /// Obtiene el HTML renderizado tras hidratación de scripts para una URL pública.
    async fn fetch_rendered_html(&self, url: &str) -> OsintResult<String>;
}

/// Especificación de búsqueda para una corrida del enjambre OSINT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsultaObjetivo {
    /// Vertical comercial objetivo (Inmobiliaria o Seguros).
    pub vertical: VerticalComercial,
    /// Territorio mexicano objetivo.
    pub ubicacion: Ubicacion,
    /// Palabras clave de búsqueda (cargos, sectores, zonas industriales, eventos).
    pub palabras_clave: Vec<String>,
    /// Límite máximo de registros a recolectar por fuente.
    pub limite_resultados: usize,
    /// Indica si la fuente podría requerir respaldo JS vía `DynamicBrowserFetcher`.
    pub requiere_js: bool,
}

/// Formato original del documento capturado por una fuente OSINT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FormatoCrudo {
    /// Documento HTML público.
    Html,
    /// Respuesta estructurada JSON.
    Json,
    /// Texto plano o fragmento normalizable.
    TextoPlano,
}

/// Mención secundaria detectada en la misma fuente (socios, co-titulares, directivos).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoMencionCruda {
    /// Nombre de la persona co-mencionada.
    pub nombre: String,
    /// Contexto o rol compartido (ej. "Socio fundador", "Director comercial", "Club de Golf").
    pub contexto: String,
    /// Empresa asociada si se observó en la fuente.
    pub empresa: Option<String>,
}

/// Registro crudo extraído de una fuente pública antes de destilación y validación de dominio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroCrudo {
    /// Nombre legible de la fuente pública de origen.
    pub fuente_origen: String,
    /// URL exacta verificable donde se observó el registro.
    pub url_evidencia: String,
    /// Marca temporal UTC de captura.
    pub capturado_en: DateTime<Utc>,
    /// Formato original de `contenido_crudo`.
    pub formato: FormatoCrudo,
    /// Cuerpo HTML, JSON o textual capturado.
    pub contenido_crudo: String,
    /// Nombre de persona o representante detectado preliminarmente.
    pub nombre_detectado: Option<String>,
    /// Razón social o empresa detectada preliminarmente.
    pub empresa_detectada: Option<String>,
    /// Ubicación detectada preliminarmente en la fuente.
    pub ubicacion_detectada: Option<Ubicacion>,
    /// Co-ocurrencias con otras personas dentro del mismo documento.
    pub co_mencionados: Vec<CoMencionCruda>,
    /// Atributos adicionales normalizables (correo, teléfono, giro, sitio web).
    pub datos_extra: BTreeMap<String, String>,
    /// Bandera de exclusión si la fuente reporta solicitud `opt_out` / ARCO.
    pub opt_out_detectado: bool,
}

/// Contrato asíncrono que toda fuente OSINT debe implementar.
#[async_trait]
pub trait OsintSource: Send + Sync {
    /// Nombre estable e identificador de la fuente pública.
    fn nombre_fuente(&self) -> &str;

    /// Ejecuta la recolección de registros públicos para `consulta`.
    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>>;
}

/// Política de control de tráfico para peticiones HTTP del enjambre.
#[derive(Debug, Clone)]
pub struct PoliticaHttp {
    /// Concurrencia máxima permitida por dominio.
    pub max_concurrencia_por_dominio: usize,
    /// Número máximo de reintentos ante `429` o `503`.
    pub max_reintentos: u32,
    /// Espera inicial (en milisegundos) para backoff exponencial.
    pub backoff_base_ms: u64,
    /// Timeout por petición HTTP.
    pub timeout: Duration,
}

impl Default for PoliticaHttp {
    fn default() -> Self {
        Self {
            max_concurrencia_por_dominio: 4,
            max_reintentos: 3,
            backoff_base_ms: 150,
            timeout: Duration::from_secs(15),
        }
    }
}

/// Cliente HTTP asíncrono basado en `reqwest` + `rustls-tls` con semáforos por dominio,
/// rotación de `User-Agent` y backoff exponencial ante `429`/`503`.
#[derive(Clone)]
pub struct HttpSwarmClient {
    client: Client,
    politica: PoliticaHttp,
    semaforos_dominio: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    ua_cursor: Arc<AtomicUsize>,
}

impl HttpSwarmClient {
    /// Construye un cliente HTTP puro en Rust utilizando `rustls-tls`.
    pub fn new(politica: PoliticaHttp) -> OsintResult<Self> {
        let client = Client::builder()
            .use_rustls_tls()
            .timeout(politica.timeout)
            .pool_max_idle_per_host(politica.max_concurrencia_por_dominio.max(1))
            .build()
            .map_err(|e| OsintError::ConfiguracionHttp(e.to_string()))?;

        Ok(Self {
            client,
            politica,
            semaforos_dominio: Arc::new(Mutex::new(HashMap::new())),
            ua_cursor: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn siguiente_user_agent(&self) -> &'static str {
        let idx = self.ua_cursor.fetch_add(1, Ordering::Relaxed);
        USER_AGENTS[idx % USER_AGENTS.len()]
    }

    async fn semaforo_para_dominio(&self, url: &str) -> Arc<Semaphore> {
        let dominio = extraer_dominio(url);
        let mut guard = self.semaforos_dominio.lock().await;
        Arc::clone(guard.entry(dominio).or_insert_with(|| {
            Arc::new(Semaphore::new(
                self.politica.max_concurrencia_por_dominio.max(1),
            ))
        }))
    }

    /// Ejecuta un `GET` protegido por semáforo de dominio y backoff exponencial ante `429`/`503`.
    #[tracing::instrument(skip(self), fields(url = %url))]
    pub async fn get_text_with_backoff(&self, fuente: &str, url: &str) -> OsintResult<String> {
        let sem = self.semaforo_para_dominio(url).await;
        let mut intento: u32 = 0;

        loop {
            let permit = sem
                .acquire()
                .await
                .map_err(|e| OsintError::Concurrencia(format!("semáforo cerrado: {e}")))?;

            let ua = self.siguiente_user_agent();
            let respuesta = self
                .client
                .get(url)
                .header(reqwest::header::USER_AGENT, ua)
                .send()
                .await;

            drop(permit);

            match respuesta {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let body = resp.text().await.map_err(|e| OsintError::HttpRequest {
                            url: url.to_string(),
                            source: e,
                        })?;
                        debug!(status = status.as_u16(), bytes = body.len(), "Respuesta HTTP OK");
                        return Ok(body);
                    }

                    if status == StatusCode::TOO_MANY_REQUESTS
                        || status == StatusCode::SERVICE_UNAVAILABLE
                    {
                        if intento >= self.politica.max_reintentos {
                            return Err(OsintError::RateLimitExcedido {
                                fuente: fuente.to_string(),
                                intentos: intento + 1,
                                ultimo_codigo: status.as_u16(),
                            });
                        }
                        let espera_ms = self
                            .politica
                            .backoff_base_ms
                            .saturating_mul(1_u64 << intento.min(10));
                        warn!(
                            intento = intento + 1,
                            status = status.as_u16(),
                            espera_ms,
                            "Backoff exponencial por rate-limit"
                        );
                        tokio::time::sleep(Duration::from_millis(espera_ms)).await;
                        intento += 1;
                        continue;
                    }

                    return Err(OsintError::HttpStatusInesperado {
                        url: url.to_string(),
                        codigo: status.as_u16(),
                    });
                }
                Err(err) => {
                    if (err.is_timeout() || err.is_connect())
                        && intento < self.politica.max_reintentos
                    {
                        let espera_ms = self
                            .politica
                            .backoff_base_ms
                            .saturating_mul(1_u64 << intento.min(10));
                        warn!(intento = intento + 1, espera_ms, error = %err, "Reintento por error transitorio de red");
                        tokio::time::sleep(Duration::from_millis(espera_ms)).await;
                        intento += 1;
                        continue;
                    }
                    return Err(OsintError::HttpRequest {
                        url: url.to_string(),
                        source: err,
                    });
                }
            }
        }
    }
}

/// Orquestador concurrente de múltiples fuentes `OsintSource` sobre Tokio.
pub struct EnjambreOsint {
    fuentes: Vec<Arc<dyn OsintSource>>,
    concurrencia_fuentes: Arc<Semaphore>,
    browser_fetcher: Option<Arc<dyn DynamicBrowserFetcher>>,
}

impl EnjambreOsint {
    /// Crea un orquestador con límite máximo de fuentes ejecutándose en paralelo.
    #[must_use]
    pub fn new(max_fuentes_paralelas: usize) -> Self {
        Self {
            fuentes: Vec::new(),
            concurrencia_fuentes: Arc::new(Semaphore::new(max_fuentes_paralelas.max(1))),
            browser_fetcher: None,
        }
    }

    /// Registra una fuente OSINT en el enjambre.
    pub fn registrar_fuente(&mut self, fuente: Arc<dyn OsintSource>) {
        self.fuentes.push(fuente);
    }

    /// Configura un adaptador CDP desacoplado para fuentes JS pesadas.
    pub fn con_browser_fetcher(mut self, fetcher: Arc<dyn DynamicBrowserFetcher>) -> Self {
        self.browser_fetcher = Some(fetcher);
        self
    }

    /// Devuelve referencia al adaptador CDP opcional configurado.
    #[must_use]
    pub fn browser_fetcher(&self) -> Option<&Arc<dyn DynamicBrowserFetcher>> {
        self.browser_fetcher.as_ref()
    }

    /// Ejecuta todas las fuentes registradas de forma concurrente y acotada por semáforo.
    #[tracing::instrument(skip(self, consulta), fields(fuentes = self.fuentes.len(), ciudad = %consulta.ubicacion.ciudad))]
    pub async fn ejecutar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let mut join_set: JoinSet<OsintResult<Vec<RegistroCrudo>>> = JoinSet::new();

        for fuente in &self.fuentes {
            let fuente_arc = Arc::clone(fuente);
            let sem = Arc::clone(&self.concurrencia_fuentes);
            let consulta_clon = consulta.clone();
            let nombre = fuente_arc.nombre_fuente().to_string();

            let span = info_span!("osint_source_task", fuente = %nombre);
            join_set.spawn(
                async move {
                    let _permit = sem
                        .acquire_owned()
                        .await
                        .map_err(|e| OsintError::Concurrencia(format!("semáforo enjambre: {e}")))?;
                    debug!(fuente = %nombre, "Iniciando recolección de fuente OSINT");
                    fuente_arc.recolectar(&consulta_clon).await
                }
                .instrument(span),
            );
        }

        let mut acumulados = Vec::new();
        while let Some(res_join) = join_set.join_next().await {
            let registros = res_join
                .map_err(|e| OsintError::Concurrencia(format!("fallo en tarea Tokio: {e}")))??;
            acumulados.extend(registros);
        }

        debug!(total_registros = acumulados.len(), "Enjambre OSINT completado");
        Ok(acumulados)
    }
}

/// Representación JSON de un registro de directorio público empresarial (tipo DENUE / RPC).
#[derive(Debug, Clone, Deserialize)]
pub struct RegistroDirectorioJson {
    /// Razón social o nombre comercial.
    pub razon_social: String,
    /// Representante legal o directivo visible.
    pub representante: Option<String>,
    /// Lista de socios o co-titulares públicos.
    #[serde(default)]
    pub socios: Vec<String>,
    /// Giro o actividad económica.
    pub giro: String,
    /// Ciudad o municipio.
    pub ciudad: String,
    /// Entidad federativa.
    pub estado: String,
    /// Zona o colonia opcional.
    pub zona: Option<String>,
    /// Correo de contacto público opcional.
    pub correo: Option<String>,
    /// Teléfono público opcional.
    pub telefono: Option<String>,
    /// Sitio web corporativo opcional.
    pub sitio_web: Option<String>,
    /// URL específica de la ficha pública.
    pub url_evidencia: Option<String>,
    /// Nota descriptiva o aviso reciente.
    pub descripcion: Option<String>,
    /// Bandera explícita de exclusión si existe en la fuente.
    #[serde(default)]
    pub opt_out: bool,
}

/// Adaptador OSINT para directorios empresariales públicos (HTML y JSON).
pub struct DirectorioPublicoAdapter {
    nombre: String,
    endpoint_url: Option<String>,
    formato: FormatoCrudo,
    http: HttpSwarmClient,
    fixture_inline: Option<String>,
}

impl DirectorioPublicoAdapter {
    /// Construye un adaptador conectado a un endpoint HTTP público.
    pub fn desde_endpoint(
        nombre: impl Into<String>,
        endpoint_url: impl Into<String>,
        formato: FormatoCrudo,
        http: HttpSwarmClient,
    ) -> Self {
        Self {
            nombre: nombre.into(),
            endpoint_url: Some(endpoint_url.into()),
            formato,
            http,
            fixture_inline: None,
        }
    }

    /// Construye un adaptador determinista sobre contenido HTML/JSON local (útil para pruebas offline).
    pub fn desde_contenido(
        nombre: impl Into<String>,
        url_evidencia: impl Into<String>,
        formato: FormatoCrudo,
        contenido: impl Into<String>,
        http: HttpSwarmClient,
    ) -> Self {
        Self {
            nombre: nombre.into(),
            endpoint_url: Some(url_evidencia.into()),
            formato,
            http,
            fixture_inline: Some(contenido.into()),
        }
    }

    /// Parsea un listado JSON de directorio empresarial público.
    pub fn parsear_json(
        fuente: &str,
        raw_json: &str,
        url_base: &str,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        let items: Vec<RegistroDirectorioJson> =
            serde_json::from_str(raw_json).map_err(|e| OsintError::ParseoJson {
                fuente: fuente.to_string(),
                source: e,
            })?;

        let mut registros = Vec::with_capacity(items.len());
        for item in items {
            let mut datos_extra = BTreeMap::new();
            datos_extra.insert("giro".to_string(), item.giro.clone());
            if let Some(correo) = &item.correo {
                datos_extra.insert("correo".to_string(), correo.clone());
            }
            if let Some(tel) = &item.telefono {
                datos_extra.insert("telefono".to_string(), tel.clone());
            }
            if let Some(web) = &item.sitio_web {
                datos_extra.insert("sitio_web".to_string(), web.clone());
            }

            let representante = item
                .representante
                .clone()
                .or_else(|| item.socios.first().cloned());

            let co_mencionados = item
                .socios
                .iter()
                .filter(|s| Some(s.as_str()) != representante.as_deref())
                .map(|socio| CoMencionCruda {
                    nombre: socio.clone(),
                    contexto: format!("Socio en {}", item.razon_social),
                    empresa: Some(item.razon_social.clone()),
                })
                .collect();

            let cuerpo = format!(
                "{} | Giro: {} | {}. {}",
                item.razon_social,
                item.giro,
                item.descripcion.as_deref().unwrap_or(""),
                item.correo.as_deref().unwrap_or("")
            );

            registros.push(RegistroCrudo {
                fuente_origen: fuente.to_string(),
                url_evidencia: item
                    .url_evidencia
                    .unwrap_or_else(|| url_base.to_string()),
                capturado_en: ahora,
                formato: FormatoCrudo::Json,
                contenido_crudo: cuerpo,
                nombre_detectado: representante,
                empresa_detectada: Some(item.razon_social),
                ubicacion_detectada: Some(Ubicacion {
                    ciudad: item.ciudad,
                    estado: item.estado,
                    zona: item.zona,
                }),
                co_mencionados,
                datos_extra,
                opt_out_detectado: item.opt_out,
            });
        }

        Ok(registros)
    }

    /// Parsea tarjetas HTML de un directorio empresarial público usando `scraper`.
    pub fn parsear_html(
        fuente: &str,
        raw_html: &str,
        url_base: &str,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        let doc = Html::parse_document(raw_html);
        let sel_card = parse_selector(fuente, "article.registro-empresa, div.empresa-card")?;
        let sel_razon = parse_selector(fuente, ".razon-social")?;
        let sel_rep = parse_selector(fuente, ".representante")?;
        let sel_socios = parse_selector(fuente, ".socio")?;
        let sel_giro = parse_selector(fuente, ".giro")?;
        let sel_ciudad = parse_selector(fuente, ".ciudad")?;
        let sel_estado = parse_selector(fuente, ".estado")?;
        let sel_contacto = parse_selector(fuente, ".contacto")?;
        let sel_link = parse_selector(fuente, "a.evidencia")?;

        let mut registros = Vec::new();
        for card in doc.select(&sel_card) {
            let empresa = first_text(&card, &sel_razon);
            let rep = first_text(&card, &sel_rep);
            let giro = first_text(&card, &sel_giro).unwrap_or_default();
            let ciudad = first_text(&card, &sel_ciudad).unwrap_or_else(|| "Mexico".to_string());
            let estado = first_text(&card, &sel_estado).unwrap_or_else(|| "Mexico".to_string());
            let contacto = first_text(&card, &sel_contacto).unwrap_or_default();
            let url_ficha = card
                .select(&sel_link)
                .next()
                .and_then(|el| el.value().attr("href"))
                .map(str::to_string)
                .unwrap_or_else(|| url_base.to_string());

            let co_mencionados = card
                .select(&sel_socios)
                .map(|el| {
                    let nombre = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    CoMencionCruda {
                        nombre,
                        contexto: format!(
                            "Socio comercial en {}",
                            empresa.as_deref().unwrap_or("empresa pública")
                        ),
                        empresa: empresa.clone(),
                    }
                })
                .filter(|c| !c.nombre.is_empty())
                .collect();

            let mut datos_extra = BTreeMap::new();
            if !giro.is_empty() {
                datos_extra.insert("giro".to_string(), giro);
            }
            if !contacto.is_empty() {
                datos_extra.insert("contacto_raw".to_string(), contacto);
            }

            let html_fragmento = card.html();
            let opt_out = card.value().attr("data-opt-out") == Some("true");

            registros.push(RegistroCrudo {
                fuente_origen: fuente.to_string(),
                url_evidencia: url_ficha,
                capturado_en: ahora,
                formato: FormatoCrudo::Html,
                contenido_crudo: html_fragmento,
                nombre_detectado: rep,
                empresa_detectada: empresa,
                ubicacion_detectada: Some(Ubicacion {
                    ciudad,
                    estado,
                    zona: None,
                }),
                co_mencionados,
                datos_extra,
                opt_out_detectado: opt_out,
            });
        }

        Ok(registros)
    }
}

#[async_trait]
impl OsintSource for DirectorioPublicoAdapter {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    #[tracing::instrument(skip(self, consulta), fields(fuente = %self.nombre))]
    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let url_base = self
            .endpoint_url
            .as_deref()
            .ok_or_else(|| OsintError::ConfiguracionHttp("endpoint_url requerido".to_string()))?;

        let cuerpo = if let Some(inline) = &self.fixture_inline {
            inline.clone()
        } else {
            self.http.get_text_with_backoff(&self.nombre, url_base).await?
        };

        let ahora = Utc::now();
        let mut registros = match self.formato {
            FormatoCrudo::Json => Self::parsear_json(&self.nombre, &cuerpo, url_base, ahora)?,
            FormatoCrudo::Html | FormatoCrudo::TextoPlano => {
                Self::parsear_html(&self.nombre, &cuerpo, url_base, ahora)?
            }
        };

        if consulta.limite_resultados > 0 && registros.len() > consulta.limite_resultados {
            registros.truncate(consulta.limite_resultados);
        }

        Ok(registros)
    }
}

/// Elemento JSON de mención pública, nota corporativa o perfil profesional abierto.
#[derive(Debug, Clone, Deserialize)]
pub struct SenalAbiertaJson {
    /// Titular de la nota o perfil.
    pub titulo: String,
    /// Persona protagonista detectada.
    pub persona: Option<String>,
    /// Empresa o institución mencionada.
    pub empresa: Option<String>,
    /// Personas co-mencionadas en la misma publicación.
    #[serde(default)]
    pub co_menciones: Vec<CoMencionCruda>,
    /// Contenido textual o HTML de la señal.
    pub contenido: String,
    /// Ciudad asociada.
    pub ciudad: Option<String>,
    /// Estado asociado.
    pub estado: Option<String>,
    /// URL pública exacta de la mención.
    pub url_evidencia: String,
    /// Fecha de publicación/captura opcional en RFC3339.
    pub publicado_en: Option<DateTime<Utc>>,
    /// Bandera de exclusión si aplica.
    #[serde(default)]
    pub opt_out: bool,
}

/// Adaptador OSINT para menciones públicas, noticias corporativas y perfiles profesionales abiertos.
pub struct SenalesAbiertasAdapter {
    nombre: String,
    endpoint_url: Option<String>,
    formato: FormatoCrudo,
    http: HttpSwarmClient,
    browser_fetcher: Option<Arc<dyn DynamicBrowserFetcher>>,
    fixture_inline: Option<String>,
}

impl SenalesAbiertasAdapter {
    /// Construye un adaptador conectado a un feed HTTP/HTML público.
    pub fn desde_endpoint(
        nombre: impl Into<String>,
        endpoint_url: impl Into<String>,
        formato: FormatoCrudo,
        http: HttpSwarmClient,
    ) -> Self {
        Self {
            nombre: nombre.into(),
            endpoint_url: Some(endpoint_url.into()),
            formato,
            http,
            browser_fetcher: None,
            fixture_inline: None,
        }
    }

    /// Construye un adaptador sobre contenido offline HTML o JSON.
    pub fn desde_contenido(
        nombre: impl Into<String>,
        url_evidencia: impl Into<String>,
        formato: FormatoCrudo,
        contenido: impl Into<String>,
        http: HttpSwarmClient,
    ) -> Self {
        Self {
            nombre: nombre.into(),
            endpoint_url: Some(url_evidencia.into()),
            formato,
            http,
            browser_fetcher: None,
            fixture_inline: Some(contenido.into()),
        }
    }

    /// Adjunta un `DynamicBrowserFetcher` opcional para rutas que exijan JS.
    pub fn con_browser_fetcher(mut self, fetcher: Arc<dyn DynamicBrowserFetcher>) -> Self {
        self.browser_fetcher = Some(fetcher);
        self
    }

    /// Parsea señales abiertas en formato JSON.
    pub fn parsear_json(
        fuente: &str,
        raw_json: &str,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        let items: Vec<SenalAbiertaJson> =
            serde_json::from_str(raw_json).map_err(|e| OsintError::ParseoJson {
                fuente: fuente.to_string(),
                source: e,
            })?;

        let mut registros = Vec::with_capacity(items.len());
        for item in items {
            let mut datos_extra = BTreeMap::new();
            datos_extra.insert("titulo".to_string(), item.titulo.clone());

            let ubicacion = item.ciudad.map(|ciudad| Ubicacion {
                ciudad,
                estado: item.estado.unwrap_or_else(|| "Mexico".to_string()),
                zona: None,
            });

            registros.push(RegistroCrudo {
                fuente_origen: fuente.to_string(),
                url_evidencia: item.url_evidencia,
                capturado_en: item.publicado_en.unwrap_or(ahora),
                formato: FormatoCrudo::Json,
                contenido_crudo: format!("{}. {}", item.titulo, item.contenido),
                nombre_detectado: item.persona,
                empresa_detectada: item.empresa,
                ubicacion_detectada: ubicacion,
                co_mencionados: item.co_menciones,
                datos_extra,
                opt_out_detectado: item.opt_out,
            });
        }
        Ok(registros)
    }

    /// Parsea noticias corporativas y perfiles profesionales abiertos en HTML.
    pub fn parsear_html(
        fuente: &str,
        raw_html: &str,
        url_base: &str,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        let doc = Html::parse_document(raw_html);
        let sel_nota = parse_selector(fuente, "article.senal-abierta, article.nota-corporativa, div.perfil-abierto")?;
        let sel_titulo = parse_selector(fuente, "h1, h2, .titulo")?;
        let sel_persona = parse_selector(fuente, ".directivo, .protagonista, .nombre-perfil")?;
        let sel_empresa = parse_selector(fuente, ".empresa")?;
        let sel_cuerpo = parse_selector(fuente, ".cuerpo, p")?;
        let sel_co = parse_selector(fuente, ".co-mencion")?;
        let sel_link = parse_selector(fuente, "a.fuente-link")?;

        let mut registros = Vec::new();
        for nota in doc.select(&sel_nota) {
            let titulo = first_text(&nota, &sel_titulo).unwrap_or_default();
            let persona = first_text(&nota, &sel_persona);
            let empresa = first_text(&nota, &sel_empresa);
            let cuerpo = nota
                .select(&sel_cuerpo)
                .map(|el| el.text().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" ");

            let url_nota = nota
                .select(&sel_link)
                .next()
                .and_then(|el| el.value().attr("href"))
                .map(str::to_string)
                .unwrap_or_else(|| url_base.to_string());

            let co_mencionados = nota
                .select(&sel_co)
                .map(|el| {
                    let nombre = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    let contexto = el
                        .value()
                        .attr("data-contexto")
                        .unwrap_or("Mención pública compartida")
                        .to_string();
                    CoMencionCruda {
                        nombre,
                        contexto,
                        empresa: empresa.clone(),
                    }
                })
                .filter(|c| !c.nombre.is_empty())
                .collect();

            let mut datos_extra = BTreeMap::new();
            if !titulo.is_empty() {
                datos_extra.insert("titulo".to_string(), titulo.clone());
            }

            let opt_out = nota.value().attr("data-opt-out") == Some("true");

            registros.push(RegistroCrudo {
                fuente_origen: fuente.to_string(),
                url_evidencia: url_nota,
                capturado_en: ahora,
                formato: FormatoCrudo::Html,
                contenido_crudo: format!("{titulo}. {cuerpo}"),
                nombre_detectado: persona,
                empresa_detectada: empresa,
                ubicacion_detectada: None,
                co_mencionados,
                datos_extra,
                opt_out_detectado: opt_out,
            });
        }

        Ok(registros)
    }
}

#[async_trait]
impl OsintSource for SenalesAbiertasAdapter {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    #[tracing::instrument(skip(self, consulta), fields(fuente = %self.nombre))]
    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let url_base = self
            .endpoint_url
            .as_deref()
            .ok_or_else(|| OsintError::ConfiguracionHttp("endpoint_url requerido".to_string()))?;

        let cuerpo = if let Some(inline) = &self.fixture_inline {
            inline.clone()
        } else if consulta.requiere_js {
            if let Some(fetcher) = &self.browser_fetcher {
                fetcher.fetch_rendered_html(url_base).await?
            } else {
                self.http.get_text_with_backoff(&self.nombre, url_base).await?
            }
        } else {
            self.http.get_text_with_backoff(&self.nombre, url_base).await?
        };

        let ahora = Utc::now();
        let mut registros = match self.formato {
            FormatoCrudo::Json => Self::parsear_json(&self.nombre, &cuerpo, ahora)?,
            FormatoCrudo::Html | FormatoCrudo::TextoPlano => {
                Self::parsear_html(&self.nombre, &cuerpo, url_base, ahora)?
            }
        };

        for reg in &mut registros {
            if reg.ubicacion_detectada.is_none() {
                reg.ubicacion_detectada = Some(consulta.ubicacion.clone());
            }
        }

        if consulta.limite_resultados > 0 && registros.len() > consulta.limite_resultados {
            registros.truncate(consulta.limite_resultados);
        }

        Ok(registros)
    }
}

/// Endpoint oficial de la API pública DENUE del INEGI para consultas por entidad federativa.
pub const DENUE_API_BASE_DEFAULT: &str =
    "https://www.inegi.org.mx/app/api/denue/v1/consulta/BuscarEntidad";

/// Registro devuelto por la API JSON oficial del DENUE (INEGI México).
#[derive(Debug, Clone, Deserialize)]
pub struct RegistroDenueInegi {
    /// Clave Estadística de Establecimiento (identificador único oficial INEGI).
    #[serde(default, alias = "CLEE", alias = "clee")]
    pub clee: Option<String>,
    /// Identificador numérico del establecimiento en DENUE.
    #[serde(default, alias = "Id", alias = "id")]
    pub id: Option<String>,
    /// Nombre comercial del establecimiento.
    #[serde(default, alias = "Nombre", alias = "nombre")]
    pub nombre: Option<String>,
    /// Razón social registrada.
    #[serde(default, alias = "Razon_social", alias = "razon_social")]
    pub razon_social: Option<String>,
    /// Descripción de clase de actividad económica SCIAN.
    #[serde(default, alias = "Clase_actividad", alias = "clase_actividad")]
    pub clase_actividad: Option<String>,
    /// Estrato de personal ocupado (ej. `51 a 100 personas`, `251 y más personas`).
    #[serde(default, alias = "Estrato", alias = "estrato")]
    pub estrato: Option<String>,
    /// Tipo de vialidad.
    #[serde(default, alias = "Tipo_vialidad", alias = "tipo_vialidad")]
    pub tipo_vialidad: Option<String>,
    /// Nombre de la calle o avenida.
    #[serde(default, alias = "Calle", alias = "calle")]
    pub calle: Option<String>,
    /// Número exterior.
    #[serde(default, alias = "Num_Exterior", alias = "num_exterior")]
    pub num_exterior: Option<String>,
    /// Colonia o asentamiento.
    #[serde(default, alias = "Colonia", alias = "colonia")]
    pub colonia: Option<String>,
    /// Código postal.
    #[serde(default, alias = "CP", alias = "cp")]
    pub cp: Option<String>,
    /// Cadena `"LOCALIDAD, MUNICIPIO, ENTIDAD"` devuelta por INEGI.
    #[serde(default, alias = "Ubicacion", alias = "ubicacion")]
    pub ubicacion: Option<String>,
    /// Teléfono público del establecimiento.
    #[serde(default, alias = "Telefono", alias = "telefono")]
    pub telefono: Option<String>,
    /// Correo electrónico público del establecimiento.
    #[serde(default, alias = "Correo_e", alias = "correo_e")]
    pub correo_e: Option<String>,
    /// Sitio web corporativo público.
    #[serde(default, alias = "Sitio_internet", alias = "sitio_internet")]
    pub sitio_internet: Option<String>,
    /// Latitud geográfica.
    #[serde(default, alias = "Latitud", alias = "latitud")]
    pub latitud: Option<String>,
    /// Longitud geográfica.
    #[serde(default, alias = "Longitud", alias = "longitud")]
    pub longitud: Option<String>,
}

/// Conector OSINT real contra la API pública del DENUE / INEGI de México (`reqwest` + `rustls-tls`).
///
/// Si el token (`AURORA_INEGI_TOKEN`) no está configurado, omite la recolección limpiamente
/// devolviendo `Ok(Vec::new())` sin interrumpir el resto del enjambre.
pub struct DenueInegiAdapter {
    token: Option<String>,
    api_base: String,
    http: HttpSwarmClient,
    fixture_inline: Option<String>,
}

impl DenueInegiAdapter {
    /// Construye el conector oficial contra `www.inegi.org.mx`.
    #[must_use]
    pub fn new(token: Option<String>, http: HttpSwarmClient) -> Self {
        Self::con_api_base(token, DENUE_API_BASE_DEFAULT, http)
    }

    /// Construye el conector apuntando a una URL base personalizable (útil para servidores mock locales).
    #[must_use]
    pub fn con_api_base(
        token: Option<String>,
        api_base: impl Into<String>,
        http: HttpSwarmClient,
    ) -> Self {
        let token_limpio = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        Self {
            token: token_limpio,
            api_base: api_base.into().trim_end_matches('/').to_string(),
            http,
            fixture_inline: None,
        }
    }

    /// Construye un adaptador offline determinista a partir de un fixture JSON oficial de DENUE.
    #[must_use]
    pub fn desde_fixture(raw_json: impl Into<String>, http: HttpSwarmClient) -> Self {
        Self {
            token: Some("fixture-offline".to_string()),
            api_base: DENUE_API_BASE_DEFAULT.to_string(),
            http,
            fixture_inline: Some(raw_json.into()),
        }
    }

    /// Indica si el adaptador cuenta con token configurado para consultar INEGI.
    #[must_use]
    pub fn tiene_token(&self) -> bool {
        self.token.is_some()
    }

    /// Construye la URL oficial `BuscarEntidad/{condicion}/{entidad}/1/{limite}/{token}`.
    #[must_use]
    pub fn construir_url_consulta(
        &self,
        condicion: &str,
        entidad: &str,
        limite: usize,
        token: &str,
    ) -> String {
        let cond_encoded = percent_encode_segment(condicion);
        let reg_final = limite.clamp(1, 50);
        format!(
            "{}/{}/{}/1/{}/{}",
            self.api_base, cond_encoded, entidad, reg_final, token
        )
    }

    /// Mapea las palabras clave y vertical comercial hacia condiciones de búsqueda DENUE.
    #[must_use]
    pub fn condiciones_para_consulta(consulta: &ConsultaObjetivo) -> Vec<String> {
        use aurora_domain::{SubverticalInmobiliaria, SubverticalSeguros};

        let mut condiciones: Vec<String> = consulta
            .palabras_clave
            .iter()
            .map(|k| k.trim())
            .filter(|k| k.len() >= 3)
            .take(2)
            .map(str::to_string)
            .collect();

        let defaults: &[&str] = match consulta.vertical {
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial) => {
                &["parque industrial", "constructora", "logistica"]
            }
            VerticalComercial::Inmobiliaria(
                SubverticalInmobiliaria::Preventa | SubverticalInmobiliaria::Residencial,
            ) => &["inmobiliaria", "constructora"],
            VerticalComercial::Seguros(SubverticalSeguros::Gmm | SubverticalSeguros::Vida) => {
                &["clinica", "hospital"]
            }
            VerticalComercial::Seguros(
                SubverticalSeguros::HombreClave
                | SubverticalSeguros::Educativo
                | SubverticalSeguros::Patrimonial,
            ) => &["logistica", "constructora", "clinica"],
        };

        for term in defaults {
            if condiciones.len() >= 3 {
                break;
            }
            if !condiciones
                .iter()
                .any(|c| c.eq_ignore_ascii_case(term))
            {
                condiciones.push((*term).to_string());
            }
        }

        if condiciones.is_empty() {
            condiciones.push("inmobiliaria".to_string());
        }
        condiciones
    }

    /// Parsea la respuesta JSON oficial de INEGI-DENUE hacia registros crudos trazables (LFPDPPP).
    pub fn parsear_respuesta_denue(
        raw_json: &str,
        consulta: &ConsultaObjetivo,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        let trimmed = raw_json.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("\"No hay resultados.\"")
            || trimmed.eq_ignore_ascii_case("No hay resultados.")
        {
            return Ok(Vec::new());
        }

        let items: Vec<RegistroDenueInegi> =
            serde_json::from_str(trimmed).map_err(|e| OsintError::ParseoJson {
                fuente: "INEGI-DENUE".to_string(),
                source: e,
            })?;

        let mut registros = Vec::with_capacity(items.len());
        for item in items {
            let nombre_comercial = limpiar_opcional(item.nombre.as_deref());
            let razon_social = limpiar_opcional(item.razon_social.as_deref());
            let empresa = razon_social.clone().or_else(|| nombre_comercial.clone());
            let Some(empresa_texto) = empresa.clone() else {
                continue;
            };

            let clee = limpiar_opcional(item.clee.as_deref());
            let denue_id = limpiar_opcional(item.id.as_deref());
            let giro = limpiar_opcional(item.clase_actividad.as_deref())
                .unwrap_or_else(|| "Actividad empresarial registrada en DENUE".to_string());
            let estrato = limpiar_opcional(item.estrato.as_deref())
                .unwrap_or_else(|| "Estrato no especificado".to_string());
            let colonia = limpiar_opcional(item.colonia.as_deref());
            let ubicacion = parsear_ubicacion_denue(
                item.ubicacion.as_deref(),
                colonia.clone(),
                &consulta.ubicacion,
            );

            let url_evidencia = if let Some(ref c) = clee {
                format!(
                    "https://www.inegi.org.mx/app/mapa/denue/default.aspx?clee={}",
                    percent_encode_segment(c)
                )
            } else if let Some(ref id) = denue_id {
                format!(
                    "https://www.inegi.org.mx/app/mapa/denue/default.aspx?id={}",
                    percent_encode_segment(id)
                )
            } else {
                "https://www.inegi.org.mx/app/mapa/denue/".to_string()
            };

            let mut datos_extra = BTreeMap::new();
            datos_extra.insert("giro".to_string(), giro.clone());
            datos_extra.insert("estrato".to_string(), estrato.clone());
            if let Some(ref c) = clee {
                datos_extra.insert("clee".to_string(), c.clone());
            }
            if let Some(ref id) = denue_id {
                datos_extra.insert("denue_id".to_string(), id.clone());
            }
            if let Some(correo) = limpiar_opcional(item.correo_e.as_deref()) {
                datos_extra.insert("correo".to_string(), correo);
            }
            if let Some(tel) = limpiar_opcional(item.telefono.as_deref()) {
                datos_extra.insert("telefono".to_string(), tel);
            }
            if let Some(web) = limpiar_opcional(item.sitio_internet.as_deref()) {
                datos_extra.insert("sitio_web".to_string(), web);
            }
            if let Some(cp) = limpiar_opcional(item.cp.as_deref()) {
                datos_extra.insert("codigo_postal".to_string(), cp);
            }
            if let Some(lat) = limpiar_opcional(item.latitud.as_deref()) {
                datos_extra.insert("latitud".to_string(), lat);
            }
            if let Some(lon) = limpiar_opcional(item.longitud.as_deref()) {
                datos_extra.insert("longitud".to_string(), lon);
            }

            let direccion = [
                limpiar_opcional(item.tipo_vialidad.as_deref()),
                limpiar_opcional(item.calle.as_deref()),
                limpiar_opcional(item.num_exterior.as_deref()),
                colonia,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");

            let nombre_detectado = nombre_comercial
                .filter(|n| !n.eq_ignore_ascii_case(&empresa_texto))
                .unwrap_or_else(|| format!("Dirección General {}", empresa_texto));

            let contenido_crudo = format!(
                "{} ({}) | Giro SCIAN: {} | Personal ocupado: {} | Ubicación: {}, {} {} | Registro público INEGI-DENUE.",
                empresa_texto,
                nombre_detectado,
                giro,
                estrato,
                ubicacion.ciudad,
                ubicacion.estado,
                direccion
            );

            registros.push(RegistroCrudo {
                fuente_origen: "INEGI-DENUE".to_string(),
                url_evidencia,
                capturado_en: ahora,
                formato: FormatoCrudo::Json,
                contenido_crudo,
                nombre_detectado: Some(nombre_detectado),
                empresa_detectada: Some(empresa_texto),
                ubicacion_detectada: Some(ubicacion),
                co_mencionados: Vec::new(),
                datos_extra,
                opt_out_detectado: false,
            });
        }

        Ok(registros)
    }
}

#[async_trait]
impl OsintSource for DenueInegiAdapter {
    fn nombre_fuente(&self) -> &str {
        "INEGI-DENUE"
    }

    #[tracing::instrument(skip(self, consulta), fields(fuente = "INEGI-DENUE", estado = %consulta.ubicacion.estado))]
    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let ahora = Utc::now();
        if let Some(ref fixture) = self.fixture_inline {
            let mut registros = Self::parsear_respuesta_denue(fixture, consulta, ahora)?;
            if consulta.limite_resultados > 0 && registros.len() > consulta.limite_resultados {
                registros.truncate(consulta.limite_resultados);
            }
            return Ok(registros);
        }

        let Some(ref token) = self.token else {
            debug!("AURORA_INEGI_TOKEN no configurado; omitiendo conector INEGI-DENUE sin error");
            return Ok(Vec::new());
        };

        let entidad = clave_entidad_inegi(&consulta.ubicacion.estado);
        let condiciones = Self::condiciones_para_consulta(consulta);
        let limite = if consulta.limite_resultados == 0 {
            15
        } else {
            consulta.limite_resultados
        };

        let mut acumulados = Vec::new();
        let mut vistos = std::collections::HashSet::new();

        for condicion in condiciones {
            if acumulados.len() >= limite {
                break;
            }
            let url = self.construir_url_consulta(&condicion, entidad, limite, token);
            match self.http.get_text_with_backoff("INEGI-DENUE", &url).await {
                Ok(cuerpo) => {
                    let lote = Self::parsear_respuesta_denue(&cuerpo, consulta, ahora)?;
                    for reg in lote {
                        let clave = reg
                            .datos_extra
                            .get("clee")
                            .cloned()
                            .unwrap_or_else(|| reg.url_evidencia.clone());
                        if vistos.insert(clave) {
                            acumulados.push(reg);
                            if acumulados.len() >= limite {
                                break;
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(condicion = %condicion, error = %err, "Error consultando condición en DENUE INEGI");
                }
            }
        }

        Ok(acumulados)
    }
}

/// Buscador web público sin API key para señales corporativas y directorios empresariales de México.
///
/// Combina `HttpSwarmClient` (semáforos por dominio, rotación de `User-Agent`, backoff exponencial)
/// con extracción HTML/XML mediante `scraper` respetando trazabilidad LFPDPPP y exclusiones `opt_out`.
pub struct WebPublicaMxAdapter {
    nombre: String,
    urls_fuente: Vec<String>,
    http: HttpSwarmClient,
    fixture_inline: Option<(String, String)>,
}

impl WebPublicaMxAdapter {
    /// Crea un adaptador orientado a señales abiertas corporativas de México usando feed público RSS/HTML.
    #[must_use]
    pub fn new(http: HttpSwarmClient) -> Self {
        Self {
            nombre: "WebPublicaMX".to_string(),
            urls_fuente: Vec::new(),
            http,
            fixture_inline: None,
        }
    }

    /// Crea un adaptador consultando una lista explícita de directorios/portales públicos mexicanos.
    #[must_use]
    pub fn con_urls(
        nombre: impl Into<String>,
        urls_fuente: Vec<String>,
        http: HttpSwarmClient,
    ) -> Self {
        let urls_limpias = urls_fuente
            .into_iter()
            .map(|u| u.trim().to_string())
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .collect();
        Self {
            nombre: nombre.into(),
            urls_fuente: urls_limpias,
            http,
            fixture_inline: None,
        }
    }

    /// Crea un adaptador offline determinista sobre HTML o RSS corporativo mexicano para tests.
    #[must_use]
    pub fn desde_fixture_html(
        nombre: impl Into<String>,
        url_base: impl Into<String>,
        raw_html: impl Into<String>,
        http: HttpSwarmClient,
    ) -> Self {
        Self {
            nombre: nombre.into(),
            urls_fuente: Vec::new(),
            http,
            fixture_inline: Some((url_base.into(), raw_html.into())),
        }
    }

    /// Construye el query de búsqueda corporativa en español de México para la consulta dada.
    #[must_use]
    pub fn construir_query_mx(consulta: &ConsultaObjetivo) -> String {
        let terminos = if consulta.palabras_clave.is_empty() {
            match consulta.vertical {
                VerticalComercial::Inmobiliaria(_) => {
                    "nave industrial parque industrial expansion".to_string()
                }
                VerticalComercial::Seguros(_) => {
                    "empresa corporativo nombramiento director expansion".to_string()
                }
            }
        } else {
            consulta
                .palabras_clave
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" ")
        };

        format!(
            "{} {} {} Mexico",
            terminos,
            consulta.ubicacion.ciudad.trim(),
            consulta.ubicacion.estado.trim()
        )
    }

    /// Parsea documentos HTML de noticias corporativas o directorios empresariales mexicanos
    /// (y feeds RSS/XML públicos de prensa de negocios) hacia `RegistroCrudo` trazables.
    pub fn parsear_html_corporativo_mx(
        fuente: &str,
        raw_doc: &str,
        url_base: &str,
        consulta: &ConsultaObjetivo,
        ahora: DateTime<Utc>,
    ) -> OsintResult<Vec<RegistroCrudo>> {
        if raw_doc.contains("<rss") || raw_doc.contains("<item>") {
            let rss_items = parsear_rss_corporativo_mx(fuente, raw_doc, url_base, consulta, ahora);
            if !rss_items.is_empty() {
                return Ok(rss_items);
            }
        }

        let doc = Html::parse_document(raw_doc);
        let sel_cards = parse_selector(
            fuente,
            "article, div.nota-corporativa, div.noticia, div.directorio-empresa, div.empresa-card, div.result, li.empresa-item",
        )?;
        let sel_titulo = parse_selector(fuente, "h1, h2, h3, .titulo, .headline, a.result__a")?;
        let sel_cuerpo = parse_selector(
            fuente,
            "p, .resumen, .extracto, .descripcion, .cuerpo, .result__snippet",
        )?;
        let sel_directivo = parse_selector(
            fuente,
            ".directivo, .representante, .ejecutivo, .protagonista, .contacto-nombre",
        )?;
        let sel_empresa = parse_selector(
            fuente,
            ".empresa, .razon-social, .compania, .organizacion",
        )?;
        let sel_co = parse_selector(fuente, ".co-mencion, .socio")?;
        let sel_link = parse_selector(
            fuente,
            "a.evidencia, a.fuente-link, a.result__a, h2 a, h3 a, a[href]",
        )?;

        let mut registros = Vec::new();
        let mut urls_vistas = std::collections::HashSet::new();

        for card in doc.select(&sel_cards) {
            let titulo = first_text(&card, &sel_titulo).unwrap_or_default();
            let cuerpo = card
                .select(&sel_cuerpo)
                .map(|el| el.text().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" ");
            let texto_total = format!("{titulo}. {cuerpo}");
            if texto_total.trim().len() < 20 {
                continue;
            }

            let href = card
                .select(&sel_link)
                .filter_map(|el| el.value().attr("href"))
                .find(|h| {
                    !h.trim().is_empty()
                        && !h.trim().starts_with('#')
                        && !h.trim().starts_with("javascript:")
                })
                .map(|h| resolver_url_absoluta(url_base, h))
                .unwrap_or_else(|| url_base.to_string());

            let empresa = first_text(&card, &sel_empresa)
                .or_else(|| inferir_empresa_mx(&texto_total));
            let directivo = first_text(&card, &sel_directivo)
                .or_else(|| inferir_directivo_mx(&texto_total))
                .or_else(|| empresa.as_ref().map(|e| format!("Dirección General {e}")));

            let co_mencionados = card
                .select(&sel_co)
                .map(|el| {
                    let nombre = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    let contexto = el
                        .value()
                        .attr("data-contexto")
                        .unwrap_or("Directivo co-mencionado en nota corporativa")
                        .to_string();
                    CoMencionCruda {
                        nombre,
                        contexto,
                        empresa: empresa.clone(),
                    }
                })
                .filter(|c| !c.nombre.is_empty())
                .collect();

            let lower = texto_total.to_ascii_lowercase();
            let opt_out = card.value().attr("data-opt-out") == Some("true")
                || lower.contains("opt-out")
                || lower.contains("derecho de oposición arco")
                || lower.contains("no contactar");

            let clave_dedup = format!("{}|{}", href, directivo.as_deref().unwrap_or(&titulo));
            if !urls_vistas.insert(clave_dedup) {
                continue;
            }

            let mut datos_extra = BTreeMap::new();
            if !titulo.is_empty() {
                datos_extra.insert("titulo".to_string(), titulo);
            }
            datos_extra.insert("origen_web_mx".to_string(), extraer_dominio(&href));

            registros.push(RegistroCrudo {
                fuente_origen: fuente.to_string(),
                url_evidencia: href,
                capturado_en: ahora,
                formato: FormatoCrudo::Html,
                contenido_crudo: texto_total,
                nombre_detectado: directivo,
                empresa_detectada: empresa,
                ubicacion_detectada: Some(consulta.ubicacion.clone()),
                co_mencionados,
                datos_extra,
                opt_out_detectado: opt_out,
            });
        }

        Ok(registros)
    }
}

#[async_trait]
impl OsintSource for WebPublicaMxAdapter {
    fn nombre_fuente(&self) -> &str {
        &self.nombre
    }

    #[tracing::instrument(skip(self, consulta), fields(fuente = %self.nombre, ciudad = %consulta.ubicacion.ciudad))]
    async fn recolectar(&self, consulta: &ConsultaObjetivo) -> OsintResult<Vec<RegistroCrudo>> {
        let ahora = Utc::now();
        if let Some((ref url_base, ref html)) = self.fixture_inline {
            let mut registros =
                Self::parsear_html_corporativo_mx(&self.nombre, html, url_base, consulta, ahora)?;
            if consulta.limite_resultados > 0 && registros.len() > consulta.limite_resultados {
                registros.truncate(consulta.limite_resultados);
            }
            return Ok(registros);
        }

        let urls: Vec<String> = if self.urls_fuente.is_empty() {
            let query = Self::construir_query_mx(consulta);
            vec![format!(
                "https://news.google.com/rss/search?q={}&hl=es-419&gl=MX&ceid=MX:es-419",
                percent_encode_segment(&query)
            )]
        } else {
            self.urls_fuente.clone()
        };

        let limite = if consulta.limite_resultados == 0 {
            15
        } else {
            consulta.limite_resultados
        };
        let mut acumulados = Vec::new();

        for url in urls {
            if acumulados.len() >= limite {
                break;
            }
            match self.http.get_text_with_backoff(&self.nombre, &url).await {
                Ok(cuerpo) => {
                    match Self::parsear_html_corporativo_mx(
                        &self.nombre,
                        &cuerpo,
                        &url,
                        consulta,
                        ahora,
                    ) {
                        Ok(lote) => acumulados.extend(lote),
                        Err(err) => {
                            warn!(url = %url, error = %err, "Error parseando documento público MX");
                        }
                    }
                }
                Err(err) => {
                    warn!(url = %url, error = %err, "Error consultando fuente web pública MX");
                }
            }
        }

        if acumulados.len() > limite {
            acumulados.truncate(limite);
        }
        Ok(acumulados)
    }
}

/// Convierte el nombre o abreviatura de una entidad federativa mexicana en su clave INEGI (`01`..`32`).
#[must_use]
pub fn clave_entidad_inegi(estado: &str) -> &'static str {
    let norm = normalizar_texto_ascii(estado);
    match norm.as_str() {
        "aguascalientes" | "ags" => "01",
        "baja california" | "bc" | "bcn" => "02",
        "baja california sur" | "bcs" => "03",
        "campeche" | "camp" => "04",
        "coahuila" | "coahuila de zaragoza" | "coah" => "05",
        "colima" | "col" => "06",
        "chiapas" | "chis" => "07",
        "chihuahua" | "chih" => "08",
        "ciudad de mexico" | "cdmx" | "distrito federal" | "df" => "09",
        "durango" | "dgo" => "10",
        "guanajuato" | "gto" => "11",
        "guerrero" | "gro" => "12",
        "hidalgo" | "hgo" => "13",
        "jalisco" | "jal" => "14",
        "mexico" | "estado de mexico" | "edomex" | "mex" => "15",
        "michoacan" | "michoacan de ocampo" | "mich" => "16",
        "morelos" | "mor" => "17",
        "nayarit" | "nay" => "18",
        "nuevo leon" | "nl" => "19",
        "oaxaca" | "oax" => "20",
        "puebla" | "pue" => "21",
        "queretaro" | "qro" => "22",
        "quintana roo" | "qroo" | "q roo" => "23",
        "san luis potosi" | "slp" => "24",
        "sinaloa" | "sin" => "25",
        "sonora" | "son" => "26",
        "tabasco" | "tab" => "27",
        "tamaulipas" | "tamps" => "28",
        "tlaxcala" | "tlax" => "29",
        "veracruz" | "veracruz de ignacio de la llave" | "ver" => "30",
        "yucatan" | "yuc" => "31",
        "zacatecas" | "zac" => "32",
        _ => "00",
    }
}

fn parsear_ubicacion_denue(
    raw_ubicacion: Option<&str>,
    colonia: Option<String>,
    fallback: &Ubicacion,
) -> Ubicacion {
    if let Some(texto) = limpiar_opcional(raw_ubicacion) {
        let partes: Vec<&str> = texto
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if partes.len() >= 3 {
            return Ubicacion {
                ciudad: capitalizar_frase(partes[1]),
                estado: capitalizar_frase(partes[2]),
                zona: colonia.or_else(|| fallback.zona.clone()),
            };
        }
        if partes.len() == 2 {
            return Ubicacion {
                ciudad: capitalizar_frase(partes[0]),
                estado: capitalizar_frase(partes[1]),
                zona: colonia.or_else(|| fallback.zona.clone()),
            };
        }
    }
    Ubicacion {
        ciudad: fallback.ciudad.clone(),
        estado: fallback.estado.clone(),
        zona: colonia.or_else(|| fallback.zona.clone()),
    }
}

fn parsear_rss_corporativo_mx(
    fuente: &str,
    raw_xml: &str,
    url_base: &str,
    consulta: &ConsultaObjetivo,
    ahora: DateTime<Utc>,
) -> Vec<RegistroCrudo> {
    let mut registros = Vec::new();
    let mut cursor = raw_xml;

    while let Some(inicio) = cursor.find("<item>") {
        let resto = &cursor[inicio + "<item>".len()..];
        let Some(fin) = resto.find("</item>") else {
            break;
        };
        let bloque = &resto[..fin];
        cursor = &resto[fin + "</item>".len()..];

        let titulo = extraer_tag_xml(bloque, "title").unwrap_or_default();
        let link = extraer_tag_xml(bloque, "link").unwrap_or_else(|| url_base.to_string());
        let desc_raw = extraer_tag_xml(bloque, "description").unwrap_or_default();
        let desc_limpia = Html::parse_fragment(&desc_raw)
            .root_element()
            .text()
            .collect::<Vec<_>>()
            .join(" ");
        let source_name = extraer_tag_xml(bloque, "source");

        let texto_total = format!("{titulo}. {desc_limpia}");
        if texto_total.trim().len() < 15 {
            continue;
        }

        let empresa = inferir_empresa_mx(&texto_total).or(source_name);
        let directivo = inferir_directivo_mx(&texto_total)
            .or_else(|| empresa.as_ref().map(|e| format!("Dirección General {e}")));

        let mut datos_extra = BTreeMap::new();
        if !titulo.is_empty() {
            datos_extra.insert("titulo".to_string(), titulo);
        }
        datos_extra.insert("origen_web_mx".to_string(), extraer_dominio(&link));

        registros.push(RegistroCrudo {
            fuente_origen: fuente.to_string(),
            url_evidencia: link,
            capturado_en: ahora,
            formato: FormatoCrudo::Html,
            contenido_crudo: texto_total,
            nombre_detectado: directivo,
            empresa_detectada: empresa,
            ubicacion_detectada: Some(consulta.ubicacion.clone()),
            co_mencionados: Vec::new(),
            datos_extra,
            opt_out_detectado: false,
        });
    }

    registros
}

fn extraer_tag_xml(bloque: &str, tag: &str) -> Option<String> {
    let apertura_simple = format!("<{tag}>");
    let cierre = format!("</{tag}>");

    let inicio_contenido = if let Some(idx) = bloque.find(&apertura_simple) {
        idx + apertura_simple.len()
    } else {
        let apertura_attr = format!("<{tag} ");
        let idx = bloque.find(&apertura_attr)?;
        let fin_tag = bloque[idx..].find('>')?;
        idx + fin_tag + 1
    };

    let resto = &bloque[inicio_contenido..];
    let fin = resto.find(&cierre)?;
    let valor = resto[..fin].trim();
    let sin_cdata = valor
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(valor);
    let decodificado = sin_cdata
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string();

    if decodificado.is_empty() {
        None
    } else {
        Some(decodificado)
    }
}

fn inferir_empresa_mx(texto: &str) -> Option<String> {
    let marcadores = [
        "S.A. de C.V.",
        "SA de CV",
        "S.A.P.I. de C.V.",
        "S. de R.L. de C.V.",
        "S.C.",
    ];
    for marcador in marcadores {
        if let Some(pos) = texto.find(marcador) {
            let prefijo = texto[..pos].trim_end();
            let palabras: Vec<&str> = prefijo
                .split_whitespace()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let inicio_mayus = palabras
                .iter()
                .position(|w| w.chars().next().is_some_and(char::is_uppercase))
                .unwrap_or(0);
            let candidato = format!("{} {}", palabras[inicio_mayus..].join(" "), marcador)
                .trim()
                .to_string();
            if candidato.len() > marcador.len() + 2 {
                return Some(candidato);
            }
        }
    }

    for pref in [
        "Grupo ",
        "Parque Industrial ",
        "Constructora ",
        "Clínica ",
        "Inmobiliaria ",
    ] {
        if let Some(pos) = texto.find(pref) {
            let segmento = &texto[pos..];
            let palabras: Vec<&str> = segmento
                .split([',', '.', ';', '|', '(', ')'])
                .next()
                .unwrap_or("")
                .split_whitespace()
                .take(4)
                .collect();
            if palabras.len() >= 2 {
                return Some(palabras.join(" "));
            }
        }
    }
    None
}

fn inferir_directivo_mx(texto: &str) -> Option<String> {
    let patrones = [
        "nombra a ",
        "designa a ",
        "encabezado por ",
        "dirigido por ",
        "director general ",
        "directora general ",
        "fundador ",
    ];
    let lower = texto.to_lowercase();
    for pat in patrones {
        if let Some(byte_pos) = lower.find(pat) {
            let inicio = byte_pos + pat.len();
            if inicio < texto.len() && texto.is_char_boundary(inicio) {
                let candidato: Vec<&str> = texto[inicio..]
                    .split([',', '.', ';', ':', '(', ')'])
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .take(3)
                    .filter(|w| w.chars().next().is_some_and(char::is_uppercase))
                    .collect();
                if candidato.len() >= 2 {
                    return Some(candidato.join(" "));
                }
            }
        }
    }
    None
}

fn resolver_url_absoluta(url_base: &str, href: &str) -> String {
    let h = href.trim();
    if h.starts_with("http://") || h.starts_with("https://") {
        return h.to_string();
    }
    let esquema = if url_base.starts_with("http://") {
        "http://"
    } else {
        "https://"
    };
    let dominio = extraer_dominio(url_base);
    if h.starts_with("//") {
        return format!("https:{h}");
    }
    if h.starts_with('/') {
        format!("{esquema}{dominio}{h}")
    } else {
        format!("{esquema}{dominio}/{h}")
    }
}

fn limpiar_opcional(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null") && *s != "0")
        .map(str::to_string)
}

fn normalizar_texto_ascii(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    input
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalizar_frase(input: &str) -> String {
    input
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let up: String = first.to_uppercase().collect();
                    let rest: String = chars.as_str().to_lowercase();
                    format!("{up}{rest}")
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn percent_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            use std::fmt::Write;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

fn extraer_dominio(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("desconocido")
        .to_ascii_lowercase()
}

fn parse_selector(fuente: &str, css: &str) -> OsintResult<Selector> {
    Selector::parse(css).map_err(|e| OsintError::ParseoHtml {
        fuente: fuente.to_string(),
        detalle: format!("selector CSS '{css}' inválido: {e:?}"),
    })
}

fn first_text(el: &scraper::ElementRef<'_>, selector: &Selector) -> Option<String> {
    el.select(selector)
        .next()
        .map(|node| node.text().collect::<Vec<_>>().join(" ").trim().to_string())
        .filter(|s| !s.is_empty())
}
