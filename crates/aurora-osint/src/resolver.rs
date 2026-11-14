use std::collections::{BTreeSet, HashSet};

use aurora_domain::{
    validar_relacion, GatilloVital, Prospecto, RelacionTopologica, TipoVinculo,
};
use strsim::{jaro_winkler, levenshtein};
use tracing::{debug, info_span};

use crate::distiller::{
    normalizar_nombre_persona, normalizar_para_busqueda, ContactosExtraidos, ProspectoDestilado,
};
use crate::error::OsintResult;
use crate::sources::CoMencionCruda;

/// Entidad unificada tras deduplicación con su contexto auxiliar para inferir el Círculo Cercano.
#[derive(Debug, Clone)]
pub struct EntidadResuelta {
    /// Prospecto canónico consolidado.
    pub prospecto: Prospecto,
    /// Empresas asociadas acumuladas en las distintas fuentes.
    pub empresas: BTreeSet<String>,
    /// Contactos normalizados acumulados.
    pub contactos: ContactosExtraidos,
    /// Co-menciones acumuladas desde las fuentes públicas.
    pub co_menciones: Vec<CoMencionCruda>,
    /// Fuentes públicas distintas en las que apareció la entidad.
    pub fuentes_vistas: BTreeSet<String>,
}

/// Motor determinista de resolución de entidades (Entity Resolution) e inferencia relacional.
#[derive(Debug, Clone)]
pub struct EntityResolver {
    /// Umbral mínimo de similitud Jaro-Winkler para fusionar variantes nominales.
    pub umbral_jaro_winkler: f64,
    /// Umbral mínimo de similitud Levenshtein normalizada como respaldo.
    pub umbral_levenshtein: f64,
}

impl Default for EntityResolver {
    fn default() -> Self {
        Self {
            umbral_jaro_winkler: 0.87,
            umbral_levenshtein: 0.80,
        }
    }
}

impl EntityResolver {
    /// Construye un resolutor con umbrales personalizados.
    #[must_use]
    pub fn new(umbral_jaro_winkler: f64, umbral_levenshtein: f64) -> Self {
        Self {
            umbral_jaro_winkler,
            umbral_levenshtein,
        }
    }

    /// Deduplica registros destilados fusionando coincidencias exactas (correo/teléfono/handle)
    /// y similitud léxica normalizada (Jaro-Winkler / Levenshtein + iniciales con empresa/ciudad).
    #[tracing::instrument(skip(self, destilados), fields(entrada = destilados.len()))]
    pub fn resolver(&self, destilados: Vec<ProspectoDestilado>) -> Vec<EntidadResuelta> {
        let mut entidades: Vec<EntidadResuelta> = Vec::new();

        for item in destilados {
            if let Some(idx) = entidades
                .iter()
                .position(|existente| self.son_misma_entidad(existente, &item))
            {
                fusionar_entidad(&mut entidades[idx], item);
            } else {
                let mut empresas = BTreeSet::new();
                if let Some(emp) = item.empresa {
                    empresas.insert(emp);
                }
                let mut fuentes_vistas = BTreeSet::new();
                fuentes_vistas.insert(item.prospecto.lfpdppp.fuente_origen.clone());

                entidades.push(EntidadResuelta {
                    prospecto: item.prospecto,
                    empresas,
                    contactos: item.contactos,
                    co_menciones: item.co_menciones,
                    fuentes_vistas,
                });
            }
        }

        debug!(salida = entidades.len(), "Resolución de entidades completada");
        entidades
    }

    fn son_misma_entidad(
        &self,
        existente: &EntidadResuelta,
        candidato: &ProspectoDestilado,
    ) -> bool {
        if !existente
            .contactos
            .correos
            .is_disjoint(&candidato.contactos.correos)
            || !existente
                .contactos
                .telefonos
                .is_disjoint(&candidato.contactos.telefonos)
            || !existente
                .contactos
                .handles_sociales
                .is_disjoint(&candidato.contactos.handles_sociales)
        {
            return true;
        }

        let nombre_a = limpiar_puntuacion(&normalizar_para_busqueda(
            &existente.prospecto.nombre_normalizado,
        ));
        let nombre_b = limpiar_puntuacion(&normalizar_para_busqueda(
            &candidato.prospecto.nombre_normalizado,
        ));

        if nombre_a.is_empty() || nombre_b.is_empty() {
            return false;
        }

        if nombre_a == nombre_b {
            return true;
        }

        let jw = jaro_winkler(&nombre_a, &nombre_b);
        let max_len = nombre_a.chars().count().max(nombre_b.chars().count()).max(1) as f64;
        let lev_sim = 1.0 - (levenshtein(&nombre_a, &nombre_b) as f64 / max_len);

        if jw >= self.umbral_jaro_winkler || lev_sim >= self.umbral_levenshtein {
            return true;
        }

        // Caso común en México: "Ing. Carlos Mendoza" vs "Carlos A. Mendoza" vs "C. Mendoza - Empresa"
        if nombres_compatibles_con_inicial(&nombre_a, &nombre_b) {
            let comparte_empresa = candidato.empresa.as_ref().is_some_and(|emp_c| {
                let norm_c = normalizar_para_busqueda(emp_c);
                existente
                    .empresas
                    .iter()
                    .any(|emp_e| jaro_winkler(&normalizar_para_busqueda(emp_e), &norm_c) >= 0.85)
            });

            let comparte_dominio = !existente
                .contactos
                .dominios
                .is_disjoint(&candidato.contactos.dominios);

            let misma_ciudad = normalizar_para_busqueda(&existente.prospecto.ubicacion.ciudad)
                == normalizar_para_busqueda(&candidato.prospecto.ubicacion.ciudad);

            if comparte_empresa || comparte_dominio || misma_ciudad {
                return true;
            }
        }

        false
    }

    /// Infiere aristas del Círculo Cercano (`RelacionTopologica`) entre entidades resueltas
    /// mediante co-menciones, empresas/dominios compartidos o contextos de club/interacción.
    #[tracing::instrument(skip(self, entidades), fields(nodos = entidades.len()))]
    pub fn inferir_relaciones(
        &self,
        entidades: &[EntidadResuelta],
    ) -> OsintResult<Vec<RelacionTopologica>> {
        let _span = info_span!("inferir_relaciones", nodos = entidades.len()).entered();
        let mut relaciones = Vec::new();
        let mut pares_vistos: HashSet<(uuid::Uuid, uuid::Uuid, TipoVinculo)> = HashSet::new();

        for i in 0..entidades.len() {
            for j in (i + 1)..entidades.len() {
                let a = &entidades[i];
                let b = &entidades[j];

                if let Some((tipo, fuerza, contexto)) = evaluar_vinculo(a, b) {
                    let clave = (a.prospecto.id, b.prospecto.id, tipo);
                    if pares_vistos.insert(clave) {
                        let rel = RelacionTopologica {
                            origen_id: a.prospecto.id,
                            destino_id: b.prospecto.id,
                            tipo_vinculo: tipo,
                            fuerza,
                            contexto_compartido: contexto,
                        };
                        validar_relacion(&rel)?;
                        relaciones.push(rel);
                    }
                }
            }
        }

        debug!(aristas = relaciones.len(), "Relaciones topológicas inferidas");
        Ok(relaciones)
    }
}

fn fusionar_entidad(base: &mut EntidadResuelta, nuevo: ProspectoDestilado) {
    // Preservar cualquier señal opt_out bajo LFPDPPP de manera conservadora.
    if nuevo.prospecto.lfpdppp.opt_out {
        base.prospecto.lfpdppp.opt_out = true;
    }

    let candidato_nombre = nuevo.prospecto.nombre_normalizado.clone();
    if candidato_nombre != base.prospecto.nombre_normalizado {
        if candidato_nombre.len() > base.prospecto.nombre_normalizado.len()
            && !candidato_nombre.contains('.')
        {
            let previo = std::mem::replace(
                &mut base.prospecto.nombre_normalizado,
                candidato_nombre,
            );
            agregar_alias_unico(&mut base.prospecto.aliases, previo);
        } else {
            agregar_alias_unico(&mut base.prospecto.aliases, candidato_nombre);
        }
    }

    for alias in nuevo.prospecto.aliases {
        agregar_alias_unico(&mut base.prospecto.aliases, alias);
    }

    if let Some(emp) = nuevo.empresa {
        agregar_alias_unico(&mut base.prospecto.aliases, emp.clone());
        base.empresas.insert(emp);
    }

    if nuevo.prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn
        > base.prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn
    {
        base.prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn =
            nuevo.prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn;
    }
    if base.prospecto.capacidad_adquisitiva.patrimonio_estimado_mxn.is_none() {
        base.prospecto.capacidad_adquisitiva.patrimonio_estimado_mxn =
            nuevo.prospecto.capacidad_adquisitiva.patrimonio_estimado_mxn;
    }

    for gatillo in nuevo.prospecto.gatillos {
        agregar_gatillo_unico(&mut base.prospecto.gatillos, gatillo);
    }

    base.contactos.correos.extend(nuevo.contactos.correos);
    base.contactos.telefonos.extend(nuevo.contactos.telefonos);
    base.contactos.dominios.extend(nuevo.contactos.dominios);
    base.contactos
        .handles_sociales
        .extend(nuevo.contactos.handles_sociales);

    base.co_menciones.extend(nuevo.co_menciones);
    base.fuentes_vistas
        .insert(nuevo.prospecto.lfpdppp.fuente_origen);
}

fn agregar_alias_unico(aliases: &mut Vec<String>, candidato: String) {
    let norm = normalizar_para_busqueda(&candidato);
    if norm.is_empty() {
        return;
    }
    if !aliases
        .iter()
        .any(|a| normalizar_para_busqueda(a) == norm)
    {
        aliases.push(candidato);
    }
}

fn agregar_gatillo_unico(gatillos: &mut Vec<GatilloVital>, nuevo: GatilloVital) {
    if let Some(existente) = gatillos.iter_mut().find(|g| g.tipo == nuevo.tipo) {
        if nuevo.peso_urgencia > existente.peso_urgencia {
            existente.peso_urgencia = nuevo.peso_urgencia;
        }
        if nuevo.ocurrido_en > existente.ocurrido_en {
            existente.ocurrido_en = nuevo.ocurrido_en;
        }
    } else {
        gatillos.push(nuevo);
    }
}

fn evaluar_vinculo(
    a: &EntidadResuelta,
    b: &EntidadResuelta,
) -> Option<(TipoVinculo, f32, String)> {
    // 1. Co-mención explícita en documento público
    for cm in &a.co_menciones {
        if coincide_con_entidad(&cm.nombre, b) {
            return Some(clasificar_contexto_comencion(&cm.contexto, cm.empresa.as_deref()));
        }
    }
    for cm in &b.co_menciones {
        if coincide_con_entidad(&cm.nombre, a) {
            return Some(clasificar_contexto_comencion(&cm.contexto, cm.empresa.as_deref()));
        }
    }

    // 2. Empresa compartida
    for emp_a in &a.empresas {
        let norm_a = normalizar_para_busqueda(emp_a);
        for emp_b in &b.empresas {
            if jaro_winkler(&norm_a, &normalizar_para_busqueda(emp_b)) >= 0.88 {
                return Some((
                    TipoVinculo::SocioComercial,
                    0.85,
                    format!("Coincidencia societaria o directiva en {emp_a}"),
                ));
            }
        }
    }

    // 3. Dominio corporativo compartido
    if let Some(dom) = a.contactos.dominios.intersection(&b.contactos.dominios).next() {
        return Some((
            TipoVinculo::SocioComercial,
            0.80,
            format!("Dominio corporativo compartido: {dom}"),
        ));
    }

    None
}

fn clasificar_contexto_comencion(
    contexto: &str,
    empresa: Option<&str>,
) -> (TipoVinculo, f32, String) {
    let norm = limpiar_puntuacion(&normalizar_para_busqueda(contexto));
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    let tiene_palabra = |objetivo: &str| tokens.contains(&objetivo);

    let detalle = match empresa {
        Some(emp) if !norm.contains(&normalizar_para_busqueda(emp)) => {
            format!("{contexto} ({emp})")
        }
        _ => contexto.to_string(),
    };

    if tiene_palabra("familiar")
        || tiene_palabra("hermano")
        || tiene_palabra("hermana")
        || tiene_palabra("esposo")
        || tiene_palabra("esposa")
        || tiene_palabra("hijo")
        || tiene_palabra("hija")
    {
        (TipoVinculo::Familiar, 0.88, detalle)
    } else if tiene_palabra("socio")
        || tiene_palabra("socia")
        || tiene_palabra("director")
        || tiene_palabra("directora")
        || tiene_palabra("consejo")
        || tiene_palabra("fundador")
        || tiene_palabra("fundadora")
    {
        (TipoVinculo::SocioComercial, 0.86, detalle)
    } else if tiene_palabra("club")
        || tiene_palabra("golf")
        || tiene_palabra("camara")
        || tiene_palabra("circulo")
        || tiene_palabra("asociacion")
    {
        (TipoVinculo::MismoClubCirculo, 0.78, detalle)
    } else if empresa.is_some() {
        (TipoVinculo::SocioComercial, 0.84, detalle)
    } else {
        (TipoVinculo::InteraccionFrecuente, 0.65, detalle)
    }
}

fn coincide_con_entidad(nombre_mencion: &str, entidad: &EntidadResuelta) -> bool {
    let sin_titulo = normalizar_nombre_persona(nombre_mencion);
    let cand = limpiar_puntuacion(&normalizar_para_busqueda(&sin_titulo));
    let target = limpiar_puntuacion(&normalizar_para_busqueda(
        &entidad.prospecto.nombre_normalizado,
    ));

    if cand.is_empty() || target.is_empty() {
        return false;
    }

    if jaro_winkler(&cand, &target) >= 0.87 || nombres_compatibles_con_inicial(&cand, &target) {
        return true;
    }

    entidad.prospecto.aliases.iter().any(|alias| {
        let al = limpiar_puntuacion(&normalizar_para_busqueda(alias));
        jaro_winkler(&cand, &al) >= 0.88
    })
}

fn limpiar_puntuacion(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn nombres_compatibles_con_inicial(a: &str, b: &str) -> bool {
    let ta: Vec<&str> = a.split_whitespace().collect();
    let tb: Vec<&str> = b.split_whitespace().collect();

    if ta.len() < 2 || tb.len() < 2 {
        return false;
    }

    let first_a = ta[0];
    let first_b = tb[0];
    let primer_nombre_compatible = first_a == first_b
        || (first_a.len() == 1 && first_b.starts_with(first_a))
        || (first_b.len() == 1 && first_a.starts_with(first_b));

    if !primer_nombre_compatible {
        return false;
    }

    // El último token (o penúltimo si hay segundo apellido) debe coincidir con el apellido principal.
    let apellidos_a = &ta[1..];
    let apellidos_b = &tb[1..];

    apellidos_a
        .iter()
        .any(|ap_a| ap_a.len() > 1 && apellidos_b.contains(ap_a))
}
