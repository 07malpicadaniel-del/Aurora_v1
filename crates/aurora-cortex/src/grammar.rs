use std::collections::HashSet;

use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use crate::error::{CortexError, CortexResult};
use crate::roles::{AjusteEstrategiaAuditor, BriefCreativo, DiagnosticoComercial, RolCognitivo};

const GBNF_PRIMITIVAS_COMUNES: &str = r#"ws ::= (" " | "\n" (" " | "  " | "    ")?)?
json-string ::= "\"" string-char* "\"" ws
string-char ::= [^"\\\x00-\x1f] | "\\" escape-char
escape-char ::= ["\\/bfnrt] | "u" hex-digit hex-digit hex-digit hex-digit
hex-digit ::= [0-9a-fA-F]
uuid-str ::= "\"" hex8 "-" hex4 "-" hex4 "-" hex4 "-" hex12 "\"" ws
hex4 ::= hex-digit hex-digit hex-digit hex-digit
hex8 ::= hex4 hex4
hex12 ::= hex4 hex4 hex4
probabilidad ::= ("0" ("." digit-seq)? | "1" ".0"?) ws
digit-seq ::= [0-9] [0-9]? [0-9]? [0-9]?
uint-small ::= ([1-9] [0-9]?) ws
bool-val ::= ("true" | "false") ws
vertical-comercial ::= ("{" ws "\"Inmobiliaria\"" ws ":" ws subvertical-inm "}" | "{" ws "\"Seguros\"" ws ":" ws subvertical-seg "}") ws
subvertical-inm ::= ("\"Preventa\"" | "\"Residencial\"" | "\"ComercialIndustrial\"") ws
subvertical-seg ::= ("\"Vida\"" | "\"Gmm\"" | "\"HombreClave\"" | "\"Educativo\"" | "\"Patrimonial\"") ws
perfil-psicologico ::= ("\"AnaliticoFinanciero\"" | "\"DinamicoVisual\"" | "\"ConservadorPatrimonial\"") ws
canal-contacto ::= ("\"WhatsApp\"" | "\"LinkedIn\"" | "\"CorreoEjecutivo\"" | "\"LlamadaDirecta\"") ws
"#;

/// Genera la gramática GBNF estricta compatible con `llama.cpp` para `DiagnosticoComercial`.
#[must_use]
pub fn gbnf_diagnostico_comercial() -> String {
    format!(
        r#"root ::= (
  "{{" ws
  "\"prospecto_id\"" ws ":" ws uuid-str "," ws
  "\"vertical_optima\"" ws ":" ws vertical-comercial "," ws
  "\"producto_gancho\"" ws ":" ws json-string "," ws
  "\"dolor_principal\"" ws ":" ws json-string "," ws
  "\"angulo_entrada\"" ws ":" ws json-string "," ws
  "\"probabilidad_cierre\"" ws ":" ws probabilidad "," ws
  "\"justificacion_topologica\"" ws ":" ws json-string
  "}}" ws
)
{GBNF_PRIMITIVAS_COMUNES}"#
    )
}

/// Genera la gramática GBNF estricta compatible con `llama.cpp` para `BriefCreativo`.
#[must_use]
pub fn gbnf_brief_creativo() -> String {
    format!(
        r#"root ::= (
  "{{" ws
  "\"prospecto_id\"" ws ":" ws uuid-str "," ws
  "\"perfil_psicologico\"" ws ":" ws perfil-psicologico "," ws
  "\"gancho_3s\"" ws ":" ws json-string "," ws
  "\"tono_narrativo\"" ws ":" ws json-string "," ws
  "\"guion_tts\"" ws ":" ws json-string "," ws
  "\"escenas_reel\"" ws ":" ws escenas-array "," ws
  "\"secciones_dossier_pdf\"" ws ":" ws string-array "," ws
  "\"canal_sugerido\"" ws ":" ws canal-contacto
  "}}" ws
)
escenas-array ::= "[" ws escena-item ("," ws escena-item)* "]" ws
escena-item ::= (
  "{{" ws
  "\"indice\"" ws ":" ws uint-small "," ws
  "\"duracion_segundos\"" ws ":" ws uint-small "," ws
  "\"descripcion_visual\"" ws ":" ws json-string "," ws
  "\"texto_overlay\"" ws ":" ws json-string
  "}}" ws
)
string-array ::= "[" ws json-string ("," ws json-string)* "]" ws
{GBNF_PRIMITIVAS_COMUNES}"#
    )
}

/// Genera la gramática GBNF estricta para invocación determinista de `AuroraToolCall`.
#[must_use]
pub fn gbnf_tool_call() -> String {
    format!(
        r#"root ::= (call-consultar | call-diagnostico | call-paquete | call-calentamiento) ws
call-consultar ::= "{{" ws "\"tool\"" ws ":" ws "\"ConsultarSubgrafo4D\"" ws "," ws "\"params\"" ws ":" ws "{{" ws "\"prospecto_id\"" ws ":" ws uuid-str "," ws "\"saltos_max\"" ws ":" ws ("1" | "2") ws "}}" ws "}}" ws
call-diagnostico ::= "{{" ws "\"tool\"" ws ":" ws "\"EmitirDiagnosticoComercial\"" ws "," ws "\"params\"" ws ":" ws "{{" ws "\"prospecto_id\"" ws ":" ws uuid-str "," ws "\"vertical\"" ws ":" ws vertical-comercial "," ws "\"producto_gancho\"" ws ":" ws json-string "," ws "\"dolor_principal\"" ws ":" ws json-string "," ws "\"probabilidad_cierre\"" ws ":" ws probabilidad "," ws "\"justificacion_topologica\"" ws ":" ws json-string "}}" ws "}}" ws
call-paquete ::= "{{" ws "\"tool\"" ws ":" ws "\"SolicitarPaqueteMultimodal\"" ws "," ws "\"params\"" ws ":" ws "{{" ws "\"prospecto_id\"" ws ":" ws uuid-str "," ws "\"gancho_3s\"" ws ":" ws json-string "," ws "\"guion_tts\"" ws ":" ws json-string "," ws "\"escenas_reel\"" ws ":" ws escenas-array "," ws "\"secciones_dossier_pdf\"" ws ":" ws string-array "," ws "\"canal_sugerido\"" ws ":" ws canal-contacto "}}" ws "}}" ws
call-calentamiento ::= "{{" ws "\"tool\"" ws ":" ws "\"ProgramarCalentamientoPeriferico\"" ws "," ws "\"params\"" ws ":" ws "{{" ws "\"prospecto_id\"" ws ":" ws uuid-str "," ws "\"plataforma\"" ws ":" ws plataforma "," ws "\"tipo_interaccion\"" ws ":" ws tipo-interaccion "," ws "\"contexto_publico\"" ws ":" ws json-string "}}" ws "}}" ws
escenas-array ::= "[" ws escena-item ("," ws escena-item)* "]" ws
escena-item ::= "{{" ws "\"indice\"" ws ":" ws uint-small "," ws "\"duracion_segundos\"" ws ":" ws uint-small "," ws "\"descripcion_visual\"" ws ":" ws json-string "," ws "\"texto_overlay\"" ws ":" ws json-string "}}" ws
string-array ::= "[" ws json-string ("," ws json-string)* "]" ws
plataforma ::= ("\"LinkedIn\"" | "\"X\"" | "\"Instagram\"" | "\"CamaraEmpresarial\"") ws
tipo-interaccion ::= ("\"ComentarioValor\"" | "\"ReaccionContextual\"" | "\"InvitacionEvento\"") ws
{GBNF_PRIMITIVAS_COMUNES}"#
    )
}

/// Genera la gramática GBNF estricta para `AjusteEstrategiaAuditor`.
#[must_use]
pub fn gbnf_ajuste_auditor() -> String {
    format!(
        r#"root ::= (
  "{{" ws
  "\"campana_id\"" ws ":" ws uuid-str "," ws
  "\"factor_ajuste_urgencia\"" ws ":" ws factor-urgencia "," ws
  "\"nuevo_gancho_sugerido\"" ws ":" ws json-string "," ws
  "\"diagnostico_rendimiento\"" ws ":" ws json-string "," ws
  "\"requiere_pausa_vertical\"" ws ":" ws bool-val
  "}}" ws
)
factor-urgencia ::= ("0." [5-9] digit-seq? | "1." [0-4] digit-seq? | "1.5" | "1.0" | "1" | "0.5") ws
{GBNF_PRIMITIVAS_COMUNES}"#
    )
}

/// Valida sintácticamente una gramática GBNF (`root ::= ...`, reglas definidas y delimitadores balanceados).
pub fn validar_sintaxis_gbnf(gramatica: &str) -> CortexResult<()> {
    if gramatica.trim().is_empty() {
        return Err(CortexError::GramaticaInvalida(
            "la gramática GBNF está vacía".to_string(),
        ));
    }

    // Reconstruir definiciones lógicas (una regla puede ocupar múltiples líneas hasta el siguiente `::=`).
    let mut definiciones: Vec<String> = Vec::new();
    for linea in gramatica.lines() {
        let trimmed = linea.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.contains("::=") {
            definiciones.push(trimmed.to_string());
        } else if let Some(ultima) = definiciones.last_mut() {
            ultima.push(' ');
            ultima.push_str(trimmed);
        } else {
            return Err(CortexError::GramaticaInvalida(format!(
                "línea fuera de regla GBNF: '{trimmed}'"
            )));
        }
    }

    let mut reglas_definidas = HashSet::new();
    let mut cuerpos = Vec::with_capacity(definiciones.len());

    for def in &definiciones {
        let Some((nombre, cuerpo)) = def.split_once("::=") else {
            return Err(CortexError::GramaticaInvalida(format!(
                "regla sin separador '::=': '{def}'"
            )));
        };
        let nombre_limpio = nombre.trim();
        if nombre_limpio.is_empty()
            || !nombre_limpio
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(CortexError::GramaticaInvalida(format!(
                "nombre de regla GBNF inválido: '{nombre_limpio}'"
            )));
        }
        if cuerpo.trim().is_empty() {
            return Err(CortexError::GramaticaInvalida(format!(
                "la regla '{nombre_limpio}' tiene cuerpo vacío"
            )));
        }
        reglas_definidas.insert(nombre_limpio.to_string());
        cuerpos.push(cuerpo.trim());
    }

    if !reglas_definidas.contains("root") {
        return Err(CortexError::GramaticaInvalida(
            "toda gramática GBNF debe definir la regla inicial 'root'".to_string(),
        ));
    }

    for cuerpo in cuerpos {
        verificar_balance_y_referencias(cuerpo, &reglas_definidas)?;
    }

    Ok(())
}

fn verificar_balance_y_referencias(
    cuerpo: &str,
    reglas_definidas: &HashSet<String>,
) -> CortexResult<()> {
    let chars: Vec<char> = cuerpo.chars().collect();
    let mut i = 0;
    let mut paren_depth: i32 = 0;

    while i < chars.len() {
        match chars[i] {
            '"' => {
                i += 1;
                let mut cerrada = false;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' {
                        cerrada = true;
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                if !cerrada {
                    return Err(CortexError::GramaticaInvalida(
                        "literal entre comillas sin cerrar en GBNF".to_string(),
                    ));
                }
            }
            '[' => {
                i += 1;
                let mut cerrada = false;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == ']' {
                        cerrada = true;
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                if !cerrada {
                    return Err(CortexError::GramaticaInvalida(
                        "clase de caracteres '[' sin cerrar en GBNF".to_string(),
                    ));
                }
            }
            '(' => {
                paren_depth += 1;
                i += 1;
            }
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err(CortexError::GramaticaInvalida(
                        "paréntesis ')' desbalanceado en GBNF".to_string(),
                    ));
                }
                i += 1;
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '-' || chars[i] == '_')
                {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                if !reglas_definidas.contains(&ident) {
                    return Err(CortexError::GramaticaInvalida(format!(
                        "no-terminal '{ident}' referenciado sin definir en gramática GBNF"
                    )));
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if paren_depth != 0 {
        return Err(CortexError::GramaticaInvalida(
            "paréntesis '(' sin cerrar en GBNF".to_string(),
        ));
    }
    Ok(())
}

/// Validación semántica adicional sobre tipos deserializados desde BOB.
pub trait ValidarRespuestaCortex {
    /// Verifica invariantes numéricos y de contenido no expresables únicamente en JSON.
    fn validar_semantica(&self, rol: RolCognitivo) -> CortexResult<()>;
}

impl ValidarRespuestaCortex for DiagnosticoComercial {
    fn validar_semantica(&self, rol: RolCognitivo) -> CortexResult<()> {
        if !self.probabilidad_cierre.is_finite()
            || !(0.0..=1.0).contains(&self.probabilidad_cierre)
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: format!(
                    "probabilidad_cierre ({}) fuera de [0.0, 1.0]",
                    self.probabilidad_cierre
                ),
            });
        }
        if self.producto_gancho.trim().is_empty()
            || self.dolor_principal.trim().is_empty()
            || self.angulo_entrada.trim().is_empty()
            || self.justificacion_topologica.trim().is_empty()
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: "DiagnosticoComercial contiene campos textuales vacíos".to_string(),
            });
        }
        Ok(())
    }
}

impl ValidarRespuestaCortex for BriefCreativo {
    fn validar_semantica(&self, rol: RolCognitivo) -> CortexResult<()> {
        if self.gancho_3s.trim().is_empty()
            || self.tono_narrativo.trim().is_empty()
            || self.guion_tts.trim().is_empty()
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: "BriefCreativo requiere gancho_3s, tono_narrativo y guion_tts no vacíos"
                    .to_string(),
            });
        }
        if self.escenas_reel.is_empty() {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: "BriefCreativo debe incluir al menos una escena de Reel".to_string(),
            });
        }
        for escena in &self.escenas_reel {
            if escena.duracion_segundos == 0 || escena.duracion_segundos > 60 {
                return Err(CortexError::ViolacionEsquema {
                    rol: rol.etiqueta().to_string(),
                    detalle: format!(
                        "duracion_segundos={} fuera de [1, 60] en escena {}",
                        escena.duracion_segundos, escena.indice
                    ),
                });
            }
            if escena.descripcion_visual.trim().is_empty() || escena.texto_overlay.trim().is_empty()
            {
                return Err(CortexError::ViolacionEsquema {
                    rol: rol.etiqueta().to_string(),
                    detalle: format!("escena {} contiene campos vacíos", escena.indice),
                });
            }
        }
        if self.secciones_dossier_pdf.is_empty()
            || self
                .secciones_dossier_pdf
                .iter()
                .any(|s| s.trim().is_empty())
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: "secciones_dossier_pdf no puede estar vacío ni contener títulos vacíos"
                    .to_string(),
            });
        }
        Ok(())
    }
}

impl ValidarRespuestaCortex for AjusteEstrategiaAuditor {
    fn validar_semantica(&self, rol: RolCognitivo) -> CortexResult<()> {
        if !self.factor_ajuste_urgencia.is_finite()
            || !(0.5..=1.5).contains(&self.factor_ajuste_urgencia)
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: format!(
                    "factor_ajuste_urgencia ({}) fuera de [0.5, 1.5]",
                    self.factor_ajuste_urgencia
                ),
            });
        }
        if self.nuevo_gancho_sugerido.trim().is_empty()
            || self.diagnostico_rendimiento.trim().is_empty()
        {
            return Err(CortexError::ViolacionEsquema {
                rol: rol.etiqueta().to_string(),
                detalle: "AjusteEstrategiaAuditor requiere campos textuales no vacíos".to_string(),
            });
        }
        Ok(())
    }
}

/// Parsea y valida estrictamente la salida JSON de BOB, rechazando prosa libre,
/// campos desconocidos (`deny_unknown_fields`), JSON truncado o valores fuera de rango.
pub fn parsear_respuesta_estricta<T>(raw: &str, rol: RolCognitivo) -> CortexResult<T>
where
    T: DeserializeOwned + ValidarRespuestaCortex,
{
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        warn!(
            rol = rol.etiqueta(),
            "Respuesta rechazada por contener texto fuera de objeto JSON estricto"
        );
        return Err(CortexError::ViolacionEsquema {
            rol: rol.etiqueta().to_string(),
            detalle: "la respuesta no es un objeto JSON puro delimitado por '{...}'".to_string(),
        });
    }

    let valor: T = serde_json::from_str(trimmed).map_err(|e| CortexError::ViolacionEsquema {
        rol: rol.etiqueta().to_string(),
        detalle: format!("JSON inválido o campo extra no permitido: {e}"),
    })?;

    valor.validar_semantica(rol)?;
    debug!(rol = rol.etiqueta(), "Respuesta GBNF/JSON validada estrictamente");
    Ok(valor)
}

#[cfg(test)]
mod gbnf_llamacpp_compat_tests {
    use super::*;

    #[test]
    fn gramaticas_multilinea_envueltas_en_parentesis_y_ws_acotado() {
        for g in [
            gbnf_diagnostico_comercial(),
            gbnf_brief_creativo(),
            gbnf_tool_call(),
            gbnf_ajuste_auditor(),
        ] {
            assert!(validar_sintaxis_gbnf(&g).is_ok());
            assert!(!g.contains("ws ::= [ \\t\\n\\r]*"));
            // Ninguna línea sin `::=` puede quedar con paréntesis abierto en 0 salvo cierre `)`.
            let mut depth = 0i32;
            for line in g.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if !line.contains("::=") && line != ")" {
                    assert!(
                        depth > 0,
                        "línea multilínea fuera de paréntesis en GBNF: {line}"
                    );
                }
                depth += line.chars().filter(|&c| c == '(').count() as i32;
                depth -= line.chars().filter(|&c| c == ')').count() as i32;
            }
            assert_eq!(depth, 0);
        }
    }
}
