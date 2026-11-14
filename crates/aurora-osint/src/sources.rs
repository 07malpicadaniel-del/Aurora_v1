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
