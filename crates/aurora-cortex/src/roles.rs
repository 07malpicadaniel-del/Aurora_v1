use std::fmt::Write as _;

use aurora_domain::{PerfilPsicologico, VerticalComercial};
use aurora_memory::SubgrafoProspecto;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Roles cognitivos ejecutados sobre el **único modelo LLM residente en VRAM** (16 GB RTX 5060 Ti).
///
/// En lugar de cargar múltiples modelos, BOB conmuta dinámicamente el contexto de sistema,
/// la temperatura y la gramática GBNF de salida según el rol activo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RolCognitivo {
    /// Evalúa el subgrafo 4D para diagnosticar producto óptimo, dolor principal y ángulo de entrada.
    Perfilador,
    /// Diseña gancho de 3 segundos, tono, guion TTS, escenas de Reel y estructura del Dossier PDF.
    DirectorCreativo,
    /// Analiza métricas de campaña para ajustar pesos y mutar ganchos de bajo rendimiento.
    Auditor,
}

impl RolCognitivo {
    /// Etiqueta estable del rol para observabilidad `tracing` y logs de auditoría.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::Perfilador => "Perfilador",
            Self::DirectorCreativo => "DirectorCreativo",
            Self::Auditor => "Auditor",
        }
    }

    /// Temperatura de muestreo recomendada para mantener determinismo en el modelo único.
    #[must_use]
    pub const fn temperatura_sugerida(&self) -> f32 {
        match self {
            Self::Perfilador => 0.15,
            Self::DirectorCreativo => 0.35,
            Self::Auditor => 0.10,
        }
    }

    /// Prompt de sistema determinista en español (México) para el rol seleccionado.
    #[must_use]
    pub const fn system_prompt(&self) -> &'static str {
        match self {
            Self::Perfilador => {
                "Eres BOB en rol PERFILADOR COMERCIAL de Aurora v2 para México (sectores Inmobiliario y Asegurador). \
                 Analiza exclusivamente el SubgrafoProspecto 4D proporcionado: capacidad adquisitiva en MXN, \
                 gatillos vitales ordenados por decaimiento temporal y vecinos del Círculo Cercano. \
                 Selecciona la VerticalComercial óptima (Inmobiliaria: Preventa/Residencial/ComercialIndustrial; \
                 Seguros: Vida/Gmm/HombreClave/Educativo/Patrimonial), identifica el dolor principal real y formula \
                 el ángulo de entrada respetando estrictamente la LFPDPPP. Responde ÚNICAMENTE con JSON válido bajo la gramática GBNF."
            }
            Self::DirectorCreativo => {
                "Eres BOB en rol DIRECTOR CREATIVO de Aurora v2 para México. \
                 A partir del DiagnosticoComercial y del PerfilPsicologico del prospecto: \
                 - AnaliticoFinanciero: abre con cifras concretas, ROI/TIR, deducibilidad fiscal y comparativas cuantitativas. \
                 - DinamicoVisual: abre con impacto visual inmediato, cortes rápidos cada 2-3s, dinamismo y exclusividad. \
                 - ConservadorPatrimonial: abre con certeza jurídica, protección familiar/patrimonial y continuidad de legado. \
                 Diseña un gancho de los primeros 3 segundos, tono narrativo, guion TTS en español de México, \
                 escenas de Reel vertical y secciones del Dossier PDF. Responde ÚNICAMENTE con JSON válido bajo la gramática GBNF."
            }
            Self::Auditor => {
                "Eres BOB en rol AUDITOR ESTRATÉGICO de Aurora v2. \
                 Evalúa las tasas de apertura, retención a 3 segundos y respuesta de la campaña para recalibrar \
                 el factor de urgencia en [0.5, 1.5] y proponer un nuevo gancho cuando existan piezas de bajo rendimiento. \
                 Responde ÚNICAMENTE con JSON válido bajo la gramática GBNF."
            }
        }
    }
}

/// Diagnóstico comercial estructurado emitido por el rol `Perfilador`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticoComercial {
    /// UUID del prospecto diagnosticado.
    pub prospecto_id: Uuid,
    /// Vertical y subvertical óptima recomendada por BOB.
    pub vertical_optima: VerticalComercial,
    /// Producto o solución concreta propuesta como gancho inicial.
    pub producto_gancho: String,
    /// Dolor financiero, operativo o patrimonial principal identificado.
    pub dolor_principal: String,
    /// Ángulo de conversación inicial no invasivo.
    pub angulo_entrada: String,
    /// Probabilidad estimada de cierre en `[0.0, 1.0]`.
    pub probabilidad_cierre: f32,
    /// Justificación explícita basada en los gatillos 4D y el Círculo Cercano.
    pub justificacion_topologica: String,
}

/// Escena estructurada para ensamblaje de Reel vertical MP4 en `aurora-factory`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscenaReel {
    /// Orden secuencial de la escena (`1`, `2`, ...).
    pub indice: u8,
    /// Duración objetivo en segundos (`1..=60`).
    pub duracion_segundos: u8,
    /// Descripción visual o indicación de plano/b-roll.
    pub descripcion_visual: String,
    /// Subtítulo o texto superpuesto de alto impacto.
    pub texto_overlay: String,
}

/// Canal de despacho sugerido por el `DirectorCreativo` (sujeto a Gatekeeper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanalContacto {
    /// Mensaje directo / nota de voz en WhatsApp Business.
    WhatsApp,
    /// Mensaje ejecutivo en LinkedIn.
    LinkedIn,
    /// Correo electrónico con dossier PDF adjunto.
    CorreoEjecutivo,
    /// Llamada telefónica directa / buzón personalizado.
    LlamadaDirecta,
}

/// Brief creativo estructurado emitido por el rol `DirectorCreativo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefCreativo {
    /// UUID del prospecto objetivo.
    pub prospecto_id: Uuid,
    /// Perfil psicológico aplicado al guion.
    pub perfil_psicologico: PerfilPsicologico,
    /// Gancho verbal/visual para los primeros 3 segundos.
    pub gancho_3s: String,
    /// Tono narrativo de la locución y redacción.
    pub tono_narrativo: String,
    /// Guion completo en español de México listo para TTS en CPU.
    pub guion_tts: String,
    /// Secuencia de escenas del Reel vertical MP4.
    pub escenas_reel: Vec<EscenaReel>,
    /// Secciones obligatorias del Dossier PDF personalizado.
    pub secciones_dossier_pdf: Vec<String>,
    /// Canal sugerido de primer contacto.
    pub canal_sugerido: CanalContacto,
}

/// Métricas de desempeño de campaña evaluadas por el rol `Auditor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricasCampana {
    /// Identificador de la campaña auditada.
    pub campana_id: Uuid,
    /// Tasa de apertura en `[0.0, 1.0]`.
    pub tasa_apertura: f32,
    /// Tasa de retención en los primeros 3 segundos en `[0.0, 1.0]`.
    pub tasa_retencion_3s: f32,
    /// Tasa de respuesta positiva en `[0.0, 1.0]`.
    pub tasa_respuesta: f32,
    /// Lista de ganchos con desempeño insuficiente para mutación.
    pub ganchos_bajo_rendimiento: Vec<String>,
}

/// Ajuste estratégico emitido por el rol `Auditor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AjusteEstrategiaAuditor {
    /// Identificador de la campaña auditada.
    pub campana_id: Uuid,
    /// Factor multiplicador de urgencia en `[0.5, 1.5]`.
    pub factor_ajuste_urgencia: f32,
    /// Nuevo gancho alternativo de reemplazo.
    pub nuevo_gancho_sugerido: String,
    /// Diagnóstico sintético de métricas observadas.
    pub diagnostico_rendimiento: String,
    /// Indica si conviene pausar temporalmente la vertical para recalibración.
    pub requiere_pausa_vertical: bool,
}

/// Construye el contexto estructurado en español para que el rol `Perfilador` evalúe `SubgrafoProspecto`.
#[must_use]
pub fn construir_contexto_perfilador(subgrafo: &SubgrafoProspecto) -> String {
    let p = &subgrafo.prospecto;
    let mut out = String::with_capacity(512);

    let _ = writeln!(out, "=== SUBGRAFO TOPOLOGICO 4D ===");
    let _ = writeln!(out, "prospecto_id: {}", p.id);
    let _ = writeln!(out, "nombre_normalizado: {}", p.nombre_normalizado);
    let _ = writeln!(out, "isla_actual: {}", subgrafo.isla.nombre());
    let _ = writeln!(
        out,
        "ubicacion: {}, {} ({})",
        p.ubicacion.ciudad,
        p.ubicacion.estado,
        p.ubicacion.zona.as_deref().unwrap_or("general")
    );
    let _ = writeln!(
        out,
        "perfil_psicologico: {}",
        p.perfil_psicologico.como_str()
    );
    let _ = writeln!(
        out,
        "ingreso_anual_mxn: {}",
        p.capacidad_adquisitiva.ingreso_anual_estimado_mxn
    );
    if let Some(pat) = p.capacidad_adquisitiva.patrimonio_estimado_mxn {
        let _ = writeln!(out, "patrimonio_estimado_mxn: {pat}");
    }
    let _ = writeln!(
        out,
        "scores_4d: compuesto={:.3}, temporal={:.3}, boost_circulo={:.3}",
        subgrafo.score_compuesto, subgrafo.score_temporal_4d, subgrafo.boost_circulo_cercano
    );
    let _ = writeln!(
        out,
        "lfpdppp_fuente: {} | opt_out: {}",
        p.lfpdppp.fuente_origen, p.lfpdppp.opt_out
    );

    out.push_str("gatillos_vigentes:\n");
    for gv in &subgrafo.gatillos_vigentes {
        let _ = writeln!(
            out,
            "- [{} | peso_4d={:.3} | horas={:.1}] {}",
            gv.gatillo.tipo.como_str(),
            gv.peso_vigente_4d,
            gv.antiguedad_horas,
            gv.gatillo.descripcion
        );
    }

    out.push_str("circulo_cercano:\n");
    for v in &subgrafo.vecinos_circulo_cercano {
        let _ = writeln!(
            out,
            "- [salto={} | {:?} | fuerza={:.2} | rel_4d={:.2}] {} ({})",
            v.salto,
            v.tipo_vinculo,
            v.fuerza_vinculo,
            v.relevancia_propia_4d,
            v.nombre_normalizado,
            v.contexto_compartido
        );
    }

    if !subgrafo.clusteres_afines.is_empty() {
        let _ = writeln!(out, "clusteres_afines: {}", subgrafo.clusteres_afines.join(", "));
    }

    out
}

/// Construye el contexto de entrada para el rol `DirectorCreativo` combinando subgrafo y diagnóstico.
#[must_use]
pub fn construir_contexto_director_creativo(
    subgrafo: &SubgrafoProspecto,
    diagnostico: &DiagnosticoComercial,
) -> String {
    let p = &subgrafo.prospecto;
    let mut out = String::with_capacity(512);

    let _ = writeln!(out, "=== BRIEFING PARA DIRECTOR CREATIVO ===");
    let _ = writeln!(out, "prospecto_id: {}", p.id);
    let _ = writeln!(out, "nombre_normalizado: {}", p.nombre_normalizado);
    let _ = writeln!(
        out,
        "perfil_psicologico: {}",
        p.perfil_psicologico.como_str()
    );
    let _ = writeln!(
        out,
        "ubicacion: {}, {}",
        p.ubicacion.ciudad, p.ubicacion.estado
    );
    let _ = writeln!(
        out,
        "vertical_optima: {}",
        diagnostico.vertical_optima.etiqueta()
    );
    let _ = writeln!(out, "producto_gancho: {}", diagnostico.producto_gancho);
    let _ = writeln!(out, "dolor_principal: {}", diagnostico.dolor_principal);
    let _ = writeln!(out, "angulo_entrada: {}", diagnostico.angulo_entrada);
    let _ = writeln!(
        out,
        "justificacion_topologica: {}",
        diagnostico.justificacion_topologica
    );

    if let Some(top_g) = subgrafo.gatillos_vigentes.first() {
        let _ = writeln!(
            out,
            "gatillo_principal: [{}] {}",
            top_g.gatillo.tipo.como_str(),
            top_g.gatillo.descripcion
        );
    }

    out
}

/// Construye el contexto de métricas para el rol `Auditor`.
#[must_use]
pub fn construir_contexto_auditor(metricas: &MetricasCampana) -> String {
    let mut out = String::with_capacity(256);
    let _ = writeln!(out, "=== AUDITORIA DE CAMPAÑA ===");
    let _ = writeln!(out, "campana_id: {}", metricas.campana_id);
    let _ = writeln!(out, "tasa_apertura: {:.3}", metricas.tasa_apertura);
    let _ = writeln!(out, "tasa_retencion_3s: {:.3}", metricas.tasa_retencion_3s);
    let _ = writeln!(out, "tasa_respuesta: {:.3}", metricas.tasa_respuesta);
    if !metricas.ganchos_bajo_rendimiento.is_empty() {
        let _ = writeln!(
            out,
            "ganchos_bajo_rendimiento: {}",
            metricas.ganchos_bajo_rendimiento.join(" | ")
        );
    }
    out
}
