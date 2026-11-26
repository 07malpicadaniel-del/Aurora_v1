use std::collections::HashSet;
use std::sync::OnceLock;

use aurora_domain::Prospecto;
use regex::{Regex, RegexBuilder};

use crate::error::{CouncilError, CouncilResult};

static RE_EMAIL: OnceLock<Option<Regex>> = OnceLock::new();
static RE_TEL_MX: OnceLock<Option<Regex>> = OnceLock::new();
static RE_RFC_MX: OnceLock<Option<Regex>> = OnceLock::new();
static RE_DIRECCION_MX: OnceLock<Option<Regex>> = OnceLock::new();

/// Palabras comunes del dominio comercial mexicano que no deben confundirse con apellidos propios.
const STOPWORDS_NO_PII: &[&str] = &[
    "grupo",
    "empresa",
    "empresas",
    "corporativo",
    "inmobiliaria",
    "desarrollos",
    "desarrollo",
    "constructora",
    "construcciones",
    "industrial",
    "comercial",
    "logistica",
    "logística",
    "servicios",
    "seguros",
    "patrimonial",
    "consultores",
    "asociados",
    "inversiones",
    "capital",
    "holding",
    "bienes",
    "raices",
    "raíces",
    "mexico",
    "méxico",
    "monterrey",
    "veracruz",
    "guadalajara",
    "queretaro",
    "querétaro",
    "cdmx",
    "puebla",
    "merida",
    "mérida",
    "nuevo",
    "leon",
    "león",
    "norte",
    "centro",
    "director",
    "directora",
    "gerente",
    "socio",
    "socia",
    "licenciado",
    "licenciada",
    "ingeniero",
    "ingeniera",
    "arquitecto",
    "doctor",
    "doctora",
    "contador",
    "para",
    "como",
    "sobre",
    "desde",
    "hasta",
    "entre",
];

/// Indicadores típicos de razón social o entidad mercantil en México.
const MARCADORES_EMPRESA: &[&str] = &[
    "s.a.",
    "s.a.p.i",
    "s. de r.l",
    "sa de cv",
    "s de rl",
    "sapi",
    "grupo ",
    "corporativo",
    "inmobiliaria",
    "logística",
    "logistica",
    "constructora",
    "desarrollos",
    "servicios",
    "holding",
    "capital",
    "consultores",
    "industrias",
    "comercializadora",
    "operadora",
];

fn regex_email() -> CouncilResult<&'static Regex> {
    RE_EMAIL
        .get_or_init(|| Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b").ok())
        .as_ref()
        .ok_or_else(|| {
            CouncilError::FalloSanitizacionPii("error interno compilando regex de correo".to_string())
        })
}

fn regex_tel_mx() -> CouncilResult<&'static Regex> {
    RE_TEL_MX
        .get_or_init(|| {
            Regex::new(
                r"(?:\+?52[\s.-]?(?:1[\s.-]?)?)?(?:\(\d{2,3}\)|\d{2,3})[\s.-]?\d{3,4}[\s.-]?\d{4}",
            )
            .ok()
        })
        .as_ref()
        .ok_or_else(|| {
            CouncilError::FalloSanitizacionPii(
                "error interno compilando regex de teléfono mexicano".to_string(),
            )
        })
}

fn regex_rfc_mx() -> CouncilResult<&'static Regex> {
    RE_RFC_MX
        .get_or_init(|| Regex::new(r"\b[A-ZÑ&]{3,4}\d{6}[A-Z0-9]{3}\b").ok())
        .as_ref()
        .ok_or_else(|| {
            CouncilError::FalloSanitizacionPii("error interno compilando regex de RFC".to_string())
        })
}

fn regex_direccion_mx() -> CouncilResult<&'static Regex> {
    RE_DIRECCION_MX
        .get_or_init(|| {
            Regex::new(
                r"(?i)\b(?:calle|av\.?|avenida|blvd\.?|boulevard|calz\.?|calzada|privada|paseo|prol\.?|prolongaci[oó]n|carretera)\s+[a-záéíóúñü0-9 .#-]{3,55}?(?:\s+(?:no\.?|n[uú]m\.?|#|int\.?|ext\.?)\s*[a-z0-9-]+|\s+\d{1,5})(?:,?\s*(?:col\.?|colonia|fracc\.?|fraccionamiento)\s+[a-záéíóúñü0-9 .-]{2,40})?(?:,?\s*c\.?p\.?\s*\d{5})?",
            )
            .ok()
        })
        .as_ref()
        .ok_or_else(|| {
            CouncilError::FalloSanitizacionPii(
                "error interno compilando regex de dirección postal".to_string(),
            )
        })
}

/// Mapa efímero de rehidratación isomórfica que vive exclusivamente en RAM.
///
/// Intencionalmente no implementa `Serialize` ni `Deserialize` para evitar que
/// cualquier tabla de correspondencia PII se vuelque accidentalmente a NVMe o logs.
#[derive(Debug, Default, Clone)]
pub struct MapaSanitizacionPii {
    entradas: Vec<(String, String)>,
    contador_prospecto: usize,
    contador_correo: usize,
    contador_tel: usize,
    contador_empresa: usize,
    contador_rfc: usize,
    contador_direccion: usize,
}

impl MapaSanitizacionPii {
    /// Cantidad total de marcadores neutros generados en esta llamada.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entradas.len()
    }

    /// Indica si no fue necesario sustituir ninguna entidad PII.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entradas.is_empty()
    }

    /// Devuelve la lista de marcadores neutros registrados (sin exponer los valores originales).
    #[must_use]
    pub fn marcadores(&self) -> Vec<&str> {
        self.entradas.iter().map(|(k, _)| k.as_str()).collect()
    }

    fn registrar_o_recuperar(&mut self, prefijo: &str, original: &str) -> String {
        let original_trim = original.trim();
        for (marcador, valor) in &self.entradas {
            if valor.eq_ignore_ascii_case(original_trim) {
                return marcador.clone();
            }
        }

        let indice = match prefijo {
            "PROSPECTO" => {
                self.contador_prospecto += 1;
                self.contador_prospecto
            }
            "CORREO" => {
                self.contador_correo += 1;
                self.contador_correo
            }
            "TEL" => {
                self.contador_tel += 1;
                self.contador_tel
            }
            "EMPRESA" => {
                self.contador_empresa += 1;
                self.contador_empresa
            }
            "RFC" => {
                self.contador_rfc += 1;
                self.contador_rfc
            }
            _ => {
                self.contador_direccion += 1;
                self.contador_direccion
            }
        };

        let marcador = format!("[{prefijo}_{indice:02}]");
        self.entradas
            .push((marcador.clone(), original_trim.to_string()));
        marcador
    }
}

impl Drop for MapaSanitizacionPii {
    fn drop(&mut self) {
        for (marcador, original) in &mut self.entradas {
            marcador.clear();
            original.clear();
        }
        self.entradas.clear();
    }
}

/// Cortafuegos isomórfico LFPDPPP previo a toda invocación de agentes externos.
pub struct PiiFirewall;

impl PiiFirewall {
    /// Detecta y reemplaza deterministamente PII del prospecto y patrones mexicanos
    /// (correos, teléfonos, RFCs, direcciones y aliases) por marcadores neutros.
    #[must_use]
    pub fn sanitizar_para_nube(
        prospecto: &Prospecto,
        texto_contexto: &str,
    ) -> (String, MapaSanitizacionPii) {
        let mut mapa = MapaSanitizacionPii::default();
        let mut texto = texto_contexto.to_string();

        // 1. Correos electrónicos (en texto o en aliases).
        if let Ok(re_email) = regex_email() {
            texto = reemplazar_coincidencias_regex(&texto, re_email, "CORREO", &mut mapa, |_| true);
        }

        // 2. Teléfonos mexicanos (10 dígitos nacionales o +52 / +52 1).
        if let Ok(re_tel) = regex_tel_mx() {
            texto = reemplazar_coincidencias_regex(&texto, re_tel, "TEL", &mut mapa, |m| {
                es_telefono_mx_valido(&texto, m.start(), m.end(), m.as_str())
            });
        }

        // 3. RFC mexicano (persona física o moral).
        if let Ok(re_rfc) = regex_rfc_mx() {
            texto = reemplazar_coincidencias_regex(&texto, re_rfc, "RFC", &mut mapa, |m| {
                es_rfc_mx_plausible(m.as_str())
            });
        }

        // 4. Direcciones específicas (calle/av/blvd + número + colonia/CP).
        if let Ok(re_dir) = regex_direccion_mx() {
            texto =
                reemplazar_coincidencias_regex(&texto, re_dir, "DIRECCION", &mut mapa, |_| true);
        }

        // 5. Nombre normalizado principal del prospecto -> [PROSPECTO_01].
        let nombre_principal = prospecto.nombre_normalizado.trim();
        if nombre_principal.chars().count() >= 3 {
            let marcador = mapa.registrar_o_recuperar("PROSPECTO", nombre_principal);
            texto = reemplazar_literal_insensible(&texto, nombre_principal, &marcador);
        }

        // 6. Aliases del prospecto ordenados de mayor a menor longitud para evitar recortes parciales.
        let mut aliases_ordenados: Vec<&str> = prospecto
            .aliases
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|a| a.chars().count() >= 3)
            .collect();
        aliases_ordenados.sort_by_key(|b| std::cmp::Reverse(b.chars().count()));

        for alias in aliases_ordenados {
            let prefijo = clasificar_alias(alias);
            let marcador = mapa.registrar_o_recuperar(prefijo, alias);
            texto = reemplazar_literal_insensible(&texto, alias, &marcador);
        }

        // 7. Tokens distintivos del nombre propio (ej. primer nombre o apellidos mencionados sueltos).
        for token in extraer_tokens_distintivos_nombre(nombre_principal) {
            if contiene_palabra_completa(&texto, &token) {
                let marcador = mapa.registrar_o_recuperar("PROSPECTO", &token);
                texto = reemplazar_palabra_completa_insensible(&texto, &token, &marcador);
            }
        }

        (texto, mapa)
    }

    /// Sanitiza texto genérico cuando la delegación no está asociada a un `Prospecto` concreto.
    #[must_use]
    pub fn sanitizar_texto_generico(texto_contexto: &str) -> (String, MapaSanitizacionPii) {
        let mut mapa = MapaSanitizacionPii::default();
        let mut texto = texto_contexto.to_string();

        if let Ok(re_email) = regex_email() {
            texto = reemplazar_coincidencias_regex(&texto, re_email, "CORREO", &mut mapa, |_| true);
        }
        if let Ok(re_tel) = regex_tel_mx() {
            texto = reemplazar_coincidencias_regex(&texto, re_tel, "TEL", &mut mapa, |m| {
                es_telefono_mx_valido(&texto, m.start(), m.end(), m.as_str())
            });
        }
        if let Ok(re_rfc) = regex_rfc_mx() {
            texto = reemplazar_coincidencias_regex(&texto, re_rfc, "RFC", &mut mapa, |m| {
                es_rfc_mx_plausible(m.as_str())
            });
        }
        if let Ok(re_dir) = regex_direccion_mx() {
            texto =
                reemplazar_coincidencias_regex(&texto, re_dir, "DIRECCION", &mut mapa, |_| true);
        }

        (texto, mapa)
    }

    /// Restaura localmente los datos originales a partir del mapa efímero en RAM.
    #[must_use]
    pub fn rehidratar_respuesta(respuesta_nube: &str, mapa: &MapaSanitizacionPii) -> String {
        let mut salida = respuesta_nube.to_string();
        for (marcador, original) in &mapa.entradas {
            salida = salida.replace(marcador, original);
        }
        salida
    }

    /// Verificador estricto ejecutado inmediatamente antes de invocar cualquier driver externo.
    ///
    /// Retorna `Err(CouncilError::FalloSanitizacionPii)` ante cualquier fuga residual de PII
    /// (sin incluir los valores PII en el mensaje de error).
    pub fn verificar_libre_de_pii(texto: &str, prospecto: &Prospecto) -> CouncilResult<()> {
        Self::verificar_texto_sin_pii_generica(texto)?;

        let texto_lower = texto.to_lowercase();
        let nombre = prospecto.nombre_normalizado.trim();
        if nombre.chars().count() >= 3 && texto_lower.contains(&nombre.to_lowercase()) {
            return Err(CouncilError::FalloSanitizacionPii(
                "fuga detectada: nombre completo del prospecto presente en payload saliente"
                    .to_string(),
            ));
        }

        for alias in &prospecto.aliases {
            let alias_trim = alias.trim();
            if alias_trim.chars().count() >= 3 && texto_lower.contains(&alias_trim.to_lowercase()) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga detectada: alias del prospecto presente en payload saliente".to_string(),
                ));
            }
        }

        for token in extraer_tokens_distintivos_nombre(nombre) {
            if contiene_palabra_completa(texto, &token) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga detectada: fragmento identificable del nombre del prospecto presente en payload saliente"
                        .to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Verifica ausencia de patrones genéricos de PII mexicana (correos, teléfonos, RFCs, direcciones).
    pub fn verificar_texto_sin_pii_generica(texto: &str) -> CouncilResult<()> {
        let re_email = regex_email()?;
        if re_email.is_match(texto) {
            return Err(CouncilError::FalloSanitizacionPii(
                "fuga detectada: dirección de correo electrónico presente en payload saliente"
                    .to_string(),
            ));
        }

        let re_tel = regex_tel_mx()?;
        for m in re_tel.find_iter(texto) {
            if es_telefono_mx_valido(texto, m.start(), m.end(), m.as_str()) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga detectada: número telefónico mexicano presente en payload saliente"
                        .to_string(),
                ));
            }
        }

        let re_rfc = regex_rfc_mx()?;
        for m in re_rfc.find_iter(texto) {
            if es_rfc_mx_plausible(m.as_str()) {
                return Err(CouncilError::FalloSanitizacionPii(
                    "fuga detectada: RFC mexicano presente en payload saliente".to_string(),
                ));
            }
        }

        let re_dir = regex_direccion_mx()?;
        if re_dir.is_match(texto) {
            return Err(CouncilError::FalloSanitizacionPii(
                "fuga detectada: dirección física específica presente en payload saliente"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

fn clasificar_alias(alias: &str) -> &'static str {
    if regex_email().is_ok_and(|re| re.is_match(alias)) {
        return "CORREO";
    }
    if regex_tel_mx().is_ok_and(|re| {
        re.find(alias)
            .is_some_and(|m| es_telefono_mx_valido(alias, m.start(), m.end(), m.as_str()))
    }) {
        return "TEL";
    }
    let lower = alias.to_lowercase();
    if MARCADORES_EMPRESA.iter().any(|m| lower.contains(m)) {
        return "EMPRESA";
    }
    "PROSPECTO"
}

fn es_telefono_mx_valido(texto: &str, start: usize, end: usize, coincidencia: &str) -> bool {
    let previo = texto[..start].chars().next_back();
    let mut tras = texto[end..].chars();
    let siguiente = tras.next();
    let siguiente_segundo = tras.next();

    if previo.is_some_and(|c| c.is_ascii_alphanumeric() || c == '$' || c == ',') {
        return false;
    }
    if siguiente.is_some_and(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    if siguiente == Some(',') && siguiente_segundo.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }

    let digitos: String = coincidencia.chars().filter(char::is_ascii_digit).collect();
    match digitos.len() {
        10 => true,
        12 => digitos.starts_with("52"),
        13 => digitos.starts_with("521"),
        _ => false,
    }
}

fn es_rfc_mx_plausible(candidato: &str) -> bool {
    let chars: Vec<char> = candidato.chars().collect();
    let offset = match chars.len() {
        12 => 3,
        13 => 4,
        _ => return false,
    };
    let mes_str: String = chars[offset + 2..offset + 4].iter().collect();
    let dia_str: String = chars[offset + 4..offset + 6].iter().collect();
    let Ok(mes) = mes_str.parse::<u8>() else {
        return false;
    };
    let Ok(dia) = dia_str.parse::<u8>() else {
        return false;
    };
    (1..=12).contains(&mes) && (1..=31).contains(&dia)
}

fn reemplazar_coincidencias_regex<F>(
    texto: &str,
    re: &Regex,
    prefijo: &str,
    mapa: &mut MapaSanitizacionPii,
    filtro: F,
) -> String
where
    F: Fn(&regex::Match<'_>) -> bool,
{
    let mut resultado = String::with_capacity(texto.len());
    let mut cursor = 0;

    for m in re.find_iter(texto) {
        if !filtro(&m) {
            continue;
        }
        resultado.push_str(&texto[cursor..m.start()]);
        let marcador = mapa.registrar_o_recuperar(prefijo, m.as_str());
        resultado.push_str(&marcador);
        cursor = m.end();
    }
    resultado.push_str(&texto[cursor..]);
    resultado
}

fn reemplazar_literal_insensible(texto: &str, objetivo: &str, reemplazo: &str) -> String {
    let patron = regex::escape(objetivo);
    let Ok(re) = RegexBuilder::new(&patron).case_insensitive(true).build() else {
        return texto.to_string();
    };
    re.replace_all(texto, reemplazo).into_owned()
}

fn extraer_tokens_distintivos_nombre(nombre: &str) -> Vec<String> {
    let mut vistos = HashSet::new();
    let mut tokens = Vec::new();
    for parte in nombre.split(|c: char| !c.is_alphanumeric()) {
        let limpio = parte.trim();
        if limpio.chars().count() < 4 {
            continue;
        }
        let lower = limpio.to_lowercase();
        if STOPWORDS_NO_PII.contains(&lower.as_str()) {
            continue;
        }
        if vistos.insert(lower) {
            tokens.push(limpio.to_string());
        }
    }
    tokens.sort_by_key(|b| std::cmp::Reverse(b.chars().count()));
    tokens
}

fn contiene_palabra_completa(texto: &str, palabra: &str) -> bool {
    let patron = format!(r"\b{}\b", regex::escape(palabra));
    RegexBuilder::new(&patron)
        .case_insensitive(true)
        .build()
        .is_ok_and(|re| re.is_match(texto))
}

fn reemplazar_palabra_completa_insensible(texto: &str, palabra: &str, reemplazo: &str) -> String {
    let patron = format!(r"\b{}\b", regex::escape(palabra));
    let Ok(re) = RegexBuilder::new(&patron).case_insensitive(true).build() else {
        return texto.to_string();
    };
    re.replace_all(texto, reemplazo).into_owned()
}
