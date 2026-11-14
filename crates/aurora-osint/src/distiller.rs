use std::collections::BTreeSet;
use std::sync::OnceLock;

use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    TipoGatilloVital, Ubicacion,
};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use regex::Regex;
use scraper::Html;
use tracing::{debug, info_span};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::error::{OsintError, OsintResult};
use crate::sources::{CoMencionCruda, FormatoCrudo, RegistroCrudo};

/// Contactos e identificadores deterministas extraídos de un registro público.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactosExtraidos {
    /// Correos electrónicos normalizados en minúsculas.
    pub correos: BTreeSet<String>,
    /// Teléfonos normalizados (dígitos).
    pub telefonos: BTreeSet<String>,
    /// Dominios web corporativos extraídos.
    pub dominios: BTreeSet<String>,
    /// Handles sociales (`@usuario` o slug de perfil).
    pub handles_sociales: BTreeSet<String>,
}

/// Resultado de destilar un `RegistroCrudo` antes de la resolución de entidades.
#[derive(Debug, Clone)]
pub struct ProspectoDestilado {
    /// Prospecto candidato con trazabilidad LFPDPPP adjunta.
    pub prospecto: Prospecto,
    /// Empresa asociada si se detectó en la fuente o en el sufijo del nombre.
    pub empresa: Option<String>,
    /// Identificadores de contacto extraídos para deduplicación exacta.
    pub contactos: ContactosExtraidos,
    /// Co-menciones detectadas en la fuente origen.
    pub co_menciones: Vec<CoMencionCruda>,
    /// Texto limpio sobre el cual se infirieron gatillos y perfil.
    pub texto_limpio: String,
}

/// Destilador de texto puro en CPU (heurísticas de `destilador_maestro` y `revelador`).
#[derive(Debug, Default, Clone)]
pub struct DestiladorOsint;

impl DestiladorOsint {
    /// Crea una nueva instancia del destilador determinista.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Destila un lote completo en paralelo sobre CPU/RAM mediante `rayon`.
    #[tracing::instrument(skip(self, registros, ubicacion_fallback), fields(total = registros.len()))]
    pub fn destilar_lote(
        &self,
        registros: &[RegistroCrudo],
        ubicacion_fallback: &Ubicacion,
    ) -> OsintResult<Vec<ProspectoDestilado>> {
        let resultados: Vec<OsintResult<ProspectoDestilado>> = registros
            .par_iter()
            .map(|reg| self.destilar_registro(reg, ubicacion_fallback))
            .collect();

        let mut destilados = Vec::with_capacity(resultados.len());
        for res in resultados {
            destilados.push(res?);
        }

        debug!(destilados = destilados.len(), "Lote OSINT destilado en CPU");
        Ok(destilados)
    }

    /// Destila un único `RegistroCrudo` limpiando HTML, normalizando nombre,
    /// extrayendo contactos y detectando `GatilloVital` + `PerfilPsicologico`.
    pub fn destilar_registro(
        &self,
        registro: &RegistroCrudo,
        ubicacion_fallback: &Ubicacion,
    ) -> OsintResult<ProspectoDestilado> {
        let _span = info_span!("destilar_registro", fuente = %registro.fuente_origen).entered();

        let texto_base = match registro.formato {
            FormatoCrudo::Html => limpiar_ruido_html(&registro.contenido_crudo),
            FormatoCrudo::Json | FormatoCrudo::TextoPlano => {
                normalizar_espacios(&registro.contenido_crudo)
            }
        };

        let mut partes_texto = vec![texto_base];
        for v in registro.datos_extra.values() {
            if !v.trim().is_empty() {
                partes_texto.push(v.trim().to_string());
            }
        }
        let texto_limpio = partes_texto.join(" ");

        let contactos = extraer_contactos(&texto_limpio, &registro.datos_extra)?;

        let (nombre_bruto, empresa_sufijo) = separar_nombre_y_empresa(
            registro
                .nombre_detectado
                .as_deref()
                .unwrap_or("Prospecto Sin Nombre"),
        );

        let empresa = registro
            .empresa_detectada
            .clone()
            .or(empresa_sufijo)
            .map(|e| normalizar_espacios(&e))
            .filter(|e| !e.is_empty());

        let nombre_normalizado = normalizar_nombre_persona(&nombre_bruto);
        let mut aliases = Vec::new();
        let bruto_limpio = normalizar_espacios(
            registro
                .nombre_detectado
                .as_deref()
                .unwrap_or(&nombre_normalizado),
        );
        if !bruto_limpio.is_empty() && bruto_limpio != nombre_normalizado {
            aliases.push(bruto_limpio);
        }
        if let Some(emp) = &empresa {
            aliases.push(emp.clone());
        }

        let gatillos = detectar_gatillos(&texto_limpio, registro.capturado_en);
        let perfil_psicologico =
            clasificar_perfil_psicologico(&texto_limpio, &registro.fuente_origen);
        let capacidad_adquisitiva = estimar_capacidad_adquisitiva(&texto_limpio, &gatillos);

        let ubicacion = registro
            .ubicacion_detectada
            .clone()
            .unwrap_or_else(|| ubicacion_fallback.clone());

        let prospecto = Prospecto {
            id: Uuid::new_v4(),
            nombre_normalizado,
            aliases,
            ubicacion,
            capacidad_adquisitiva,
            perfil_psicologico,
            lfpdppp: MetadatosLfpdppp {
                fuente_origen: registro.fuente_origen.clone(),
                url_evidencia: registro.url_evidencia.clone(),
                capturado_en: registro.capturado_en,
                opt_out: registro.opt_out_detectado,
            },
            gatillos,
        };

        Ok(ProspectoDestilado {
            prospecto,
            empresa,
            contactos,
            co_menciones: registro.co_mencionados.clone(),
            texto_limpio,
        })
    }
}

/// Elimina etiquetas HTML, scripts/estilos y colapsa espacios en blanco.
#[must_use]
pub fn limpiar_ruido_html(raw_html: &str) -> String {
    use scraper::node::Node;

    fn recolectar<'a>(node: scraper::ElementRef<'a>, out: &mut Vec<&'a str>) {
        for child in node.children() {
            match child.value() {
                Node::Text(txt) => {
                    let trimmed = txt.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed);
                    }
                }
                Node::Element(el)
                    if !matches!(el.name(), "script" | "style" | "noscript" | "template") =>
                {
                    if let Some(child_el) = scraper::ElementRef::wrap(child) {
                        recolectar(child_el, out);
                    }
                }
                _ => {}
            }
        }
    }

    let doc = Html::parse_document(raw_html);
    let mut fragmentos = Vec::new();
    recolectar(doc.root_element(), &mut fragmentos);
    let unido = fragmentos.join(" ");
    normalizar_espacios(&unido)
}

/// Normaliza espacios consecutivos y formas Unicode NFC.
#[must_use]
pub fn normalizar_espacios(texto: &str) -> String {
    let nfc: String = texto.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Convierte a minúsculas ASCII/latino sin diacríticos para comparación léxica estable.
#[must_use]
pub fn normalizar_para_busqueda(texto: &str) -> String {
    let sin_diacriticos: String = texto
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    normalizar_espacios(&sin_diacriticos).to_lowercase()
}

/// Separa patrones comunes tipo `"C. Mendoza - Logística del Golfo"` en `(nombre, empresa)`.
#[must_use]
pub fn separar_nombre_y_empresa(raw: &str) -> (String, Option<String>) {
    if let Some((izq, der)) = raw.split_once(" - ") {
        let n = izq.trim().to_string();
        let e = der.trim().to_string();
        if !n.is_empty() && !e.is_empty() {
            return (n, Some(e));
        }
    }
    (raw.trim().to_string(), None)
}

/// Normaliza nombres removiendo títulos honoríficos comunes en México y unificando capitalización.
#[must_use]
pub fn normalizar_nombre_persona(raw: &str) -> String {
    const PREFIJOS: &[&str] = &[
        "ing.", "ing", "lic.", "lic", "arq.", "arq", "dr.", "dr", "dra.", "dra", "c.p.", "cp",
        "mtro.", "mtro", "mtra.", "mtra", "sr.", "sra.",
    ];

    let limpio = normalizar_espacios(raw);
    let mut tokens: Vec<&str> = limpio.split_whitespace().collect();

    while let Some(primero) = tokens.first() {
        let lower = primero.to_lowercase();
        if PREFIJOS.contains(&lower.as_str()) {
            tokens.remove(0);
        } else {
            break;
        }
    }

    if tokens.is_empty() {
        return limpio;
    }

    tokens
        .into_iter()
        .map(capitalizar_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalizar_token(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) => {
            let up: String = first.to_uppercase().collect();
            let rest: String = chars.as_str().to_lowercase();
            format!("{up}{rest}")
        }
        None => String::new(),
    }
}

/// Extrae correos, teléfonos, dominios web y handles sociales de forma determinista.
pub fn extraer_contactos(
    texto: &str,
    datos_extra: &std::collections::BTreeMap<String, String>,
) -> OsintResult<ContactosExtraidos> {
    let mut out = ContactosExtraidos::default();
    let mut fuente_combinada = texto.to_string();
    for v in datos_extra.values() {
        fuente_combinada.push(' ');
        fuente_combinada.push_str(v);
    }

    let re_email = get_email_regex()?;
    for cap in re_email.find_iter(&fuente_combinada) {
        let email = cap.as_str().to_ascii_lowercase();
        if let Some((_, dom)) = email.split_once('@') {
            if !es_dominio_generico(dom) {
                out.dominios.insert(dom.to_string());
            }
        }
        out.correos.insert(email);
    }

    let re_phone = get_phone_regex()?;
    for cap in re_phone.find_iter(&fuente_combinada) {
        let digitos: String = cap.as_str().chars().filter(char::is_ascii_digit).collect();
        if digitos.len() >= 10 && digitos.len() <= 13 {
            out.telefonos.insert(digitos);
        }
    }

    let re_domain = get_domain_regex()?;
    for cap in re_domain.captures_iter(&fuente_combinada) {
        if let Some(m) = cap.get(1) {
            let dom = m.as_str().to_ascii_lowercase();
            if !es_dominio_generico(&dom) {
                out.dominios.insert(dom);
            }
        }
    }

    let re_handle = get_handle_regex()?;
    let bytes = fuente_combinada.as_bytes();
    for m in re_handle.find_iter(&fuente_combinada) {
        let precedido_por_correo = m.start() > 0 && {
            let prev = bytes[m.start() - 1];
            prev.is_ascii_alphanumeric() || matches!(prev, b'.' | b'_' | b'%' | b'+' | b'-')
        };
        if precedido_por_correo {
            continue;
        }
        let handle = m.as_str().to_ascii_lowercase();
        if !out.correos.iter().any(|c| c.contains(&handle)) {
            out.handles_sociales.insert(handle);
        }
    }

    Ok(out)
}

/// Motor léxico de detección y ponderación de `GatilloVital`.
#[must_use]
pub fn detectar_gatillos(texto: &str, ocurrido_en: DateTime<Utc>) -> Vec<GatilloVital> {
    let norm = normalizar_para_busqueda(texto);
    let mut gatillos = Vec::new();

    let reglas: &[(TipoGatilloVital, &[&str], f32, &str)] = &[
        (
            TipoGatilloVital::ExpansionLogistica,
            &[
                "nueva bodega",
                "nave industrial",
                "parque industrial",
                "centro de distribucion",
                "cedis",
                "expansion logistica",
                "nueva sucursal",
                "terminal portuaria",
                "bodega aduanal",
            ],
            0.84,
            "Señal de expansión operativa, bodega o infraestructura logística",
        ),
        (
            TipoGatilloVital::AperturaEmpresa,
            &[
                "constituyo",
                "nueva empresa",
                "razon social",
                "apertura de sociedad",
                "socio fundador",
                "acta constitutiva",
                "hombre clave",
                "nueva sociedad",
            ],
            0.82,
            "Constitución de nueva sociedad o estructura directiva clave",
        ),
        (
            TipoGatilloVital::CambioPuesto,
            &[
                "nombrado director",
                "nuevo director",
                "asume la direccion",
                "ascenso",
                "director general",
                "ceo",
                "socio director",
                "vicepresidente",
            ],
            0.76,
            "Cambio directivo o nombramiento ejecutivo relevante",
        ),
        (
            TipoGatilloVital::CompraActivo,
            &[
                "adquirio inmueble",
                "compra de flotilla",
                "inversion inmobiliaria",
                "preventa",
                "escrituracion",
                "adquisicion de terreno",
                "lote residencial",
                "residencia",
            ],
            0.78,
            "Adquisición o intención activa de compra de activo inmobiliario/productivo",
        ),
        (
            TipoGatilloVital::Matrimonio,
            &[
                "matrimonio",
                "boda",
                "contrajo nupcias",
                "recien casados",
                "enlace matrimonial",
            ],
            0.70,
            "Evento familiar de matrimonio detectado en fuente pública",
        ),
        (
            TipoGatilloVital::Nacimiento,
            &[
                "nacimiento",
                "nuevo hijo",
                "recien nacido",
                "fideicomiso educativo",
                "paternidad",
                "maternidad",
            ],
            0.74,
            "Nacimiento o crecimiento del núcleo familiar",
        ),
    ];

    let modificador_urgencia: f32 = if norm.contains("inmediat")
        || norm.contains("este trimestre")
        || norm.contains("2026")
        || norm.contains("en marcha")
        || norm.contains("inaugura")
    {
        0.08
    } else {
        0.0
    };

    for (tipo, patrones, peso_base, descripcion_base) in reglas {
        let coincidencias = patrones.iter().filter(|p| norm.contains(**p)).count();
        if coincidencias > 0 {
            let bonus = ((coincidencias - 1) as f32 * 0.04).min(0.12);
            let peso = (*peso_base + bonus + modificador_urgencia).clamp(0.0, 1.0);
            gatillos.push(GatilloVital {
                tipo: *tipo,
                descripcion: (*descripcion_base).to_string(),
                ocurrido_en,
                peso_urgencia: peso,
            });
        }
    }

    gatillos
}

/// Clasifica el `PerfilPsicologico` heurístico a partir del vocabulario y origen del documento.
#[must_use]
pub fn clasificar_perfil_psicologico(texto: &str, fuente: &str) -> PerfilPsicologico {
    let norm = normalizar_para_busqueda(texto);
    let fuente_norm = normalizar_para_busqueda(fuente);

    let terminos_analiticos = [
        "roi",
        "tir",
        "cap rate",
        "ebitda",
        "deducible",
        "fiscal",
        "rentabilidad",
        "flujo",
        "logistica",
        "aduanal",
        "arancel",
        "nave industrial",
        "inversion",
    ];
    let terminos_dinamicos = [
        "diseno",
        "exclusiv",
        "lanzamiento",
        "lifestyle",
        "arquitectura",
        "penthouse",
        "vanguardia",
        "vista",
        "premio",
        "evento",
    ];
    let terminos_conservadores = [
        "patrimonio",
        "legado",
        "familia",
        "sucesion",
        "fideicomiso",
        "resguardo",
        "retiro",
        "proteccion",
        "certidumbre",
        "gastos medicos",
        "educativo",
    ];

    let mut score_analitico: usize = terminos_analiticos
        .iter()
        .filter(|t| norm.contains(**t))
        .count();
    let score_dinamico: usize = terminos_dinamicos
        .iter()
        .filter(|t| norm.contains(**t))
        .count();
    let mut score_conservador: usize = terminos_conservadores
        .iter()
        .filter(|t| norm.contains(**t))
        .count();

    if fuente_norm.contains("denue") || fuente_norm.contains("directorio") {
        score_analitico += 1;
    }
    if fuente_norm.contains("registro") || fuente_norm.contains("notaria") {
        score_conservador += 1;
    }

    if score_dinamico > score_analitico && score_dinamico >= score_conservador {
        PerfilPsicologico::DinamicoVisual
    } else if score_conservador > score_analitico {
        PerfilPsicologico::ConservadorPatrimonial
    } else {
        PerfilPsicologico::AnaliticoFinanciero
    }
}

/// Estima una capacidad adquisitiva base en MXN a partir de señales de rol y gatillos.
#[must_use]
pub fn estimar_capacidad_adquisitiva(
    texto: &str,
    gatillos: &[GatilloVital],
) -> CapacidadAdquisitiva {
    let norm = normalizar_para_busqueda(texto);
    let mut ingreso: u64 = 1_800_000;
    let mut patrimonio: Option<u64> = None;

    if norm.contains("director") || norm.contains("ceo") || norm.contains("socio") {
        ingreso += 1_500_000;
    }
    if norm.contains("bodega") || norm.contains("nave industrial") || norm.contains("flotilla") {
        ingreso += 2_200_000;
        patrimonio = Some(18_000_000);
    }
    if gatillos.len() >= 2 {
        ingreso += 600_000;
    }

    CapacidadAdquisitiva {
        ingreso_anual_estimado_mxn: ingreso,
        patrimonio_estimado_mxn: patrimonio,
    }
}

fn es_dominio_generico(dom: &str) -> bool {
    matches!(
        dom,
        "gmail.com"
            | "hotmail.com"
            | "outlook.com"
            | "yahoo.com"
            | "yahoo.com.mx"
            | "icloud.com"
            | "proton.me"
            | "linkedin.com"
            | "facebook.com"
            | "instagram.com"
            | "x.com"
    )
}

fn get_email_regex() -> OsintResult<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}").ok()
    })
    .as_ref()
    .ok_or_else(|| OsintError::ResolucionEntidades("regex email inválido".to_string()))
}

fn get_phone_regex() -> OsintResult<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:\+?52[\s\-.]?)?(?:\(?\d{2,3}\)?[\s\-.]?)?\d{3,4}[\s\-.]?\d{4}").ok()
    })
    .as_ref()
    .ok_or_else(|| OsintError::ResolucionEntidades("regex teléfono inválido".to_string()))
}

fn get_domain_regex() -> OsintResult<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:https?://|www\.)([a-z0-9\-]+(?:\.[a-z0-9\-]+)+)").ok()
    })
    .as_ref()
    .ok_or_else(|| OsintError::ResolucionEntidades("regex dominio inválido".to_string()))
}

fn get_handle_regex() -> OsintResult<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"@[A-Za-z0-9_]{3,30}").ok())
        .as_ref()
        .ok_or_else(|| OsintError::ResolucionEntidades("regex handle inválido".to_string()))
}
