use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::time::Instant;

use aurora_domain::{GatilloVital, Prospecto, TipoVinculo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info_span};
use uuid::Uuid;

use crate::error::MemoryResult;
use crate::islands::IslaVertical;
use crate::storage::MotorMemoriaAurora;

/// Gatillo vital evaluado con su peso vigente tras decaimiento temporal 4D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatilloVigente {
    /// Gatillo original capturado.
    pub gatillo: GatilloVital,
    /// Peso vigente en `ahora` calculado mediante $w(t) = w_0 \cdot e^{-\lambda \Delta t}$.
    pub peso_vigente_4d: f32,
    /// Antigüedad del evento en horas.
    pub antiguedad_horas: f64,
}

/// Vecino dentro del Círculo Cercano (1 o 2 saltos) incluido en el contexto de BOB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VecinoCirculoCercano {
    /// UUID del prospecto conectado.
    pub prospecto_id: Uuid,
    /// Nombre normalizado del vecino.
    pub nombre_normalizado: String,
    /// Tipo de vínculo topológico.
    pub tipo_vinculo: TipoVinculo,
    /// Fuerza efectiva de la conexión en `[0.0, 1.0]`.
    pub fuerza_vinculo: f32,
    /// Contexto compartido que respalda la arista.
    pub contexto_compartido: String,
    /// Distancia topológica en saltos (`1` o `2`).
    pub salto: u8,
    /// Relevancia temporal 4D propia del vecino.
    pub relevancia_propia_4d: f32,
}

/// Paquete enriquecido listo para ser consumido por BOB (`cortex` / `router`) en la Fase 3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubgrafoProspecto {
    /// Prospecto objetivo validado bajo LFPDPPP.
    pub prospecto: Prospecto,
    /// Isla vertical en la que reside.
    pub isla: IslaVertical,
    /// Puntuación compuesta híbrida (similitud semántica + vigencia 4D + señal de círculo cercano).
    pub score_compuesto: f32,
    /// Similitud coseno en `[0.0, 1.0]` respecto a la consulta.
    pub similitud_vectorial: f32,
    /// Relevancia temporal 4D propia del prospecto.
    pub score_temporal_4d: f32,
    /// Impulso heredado por gatillos recientes en su Círculo Cercano (1–2 saltos).
    pub boost_circulo_cercano: f32,
    /// Gatillos ordenados descendentemente por vigencia 4D.
    pub gatillos_vigentes: Vec<GatilloVigente>,
    /// Vecinos de 1 y 2 saltos con contexto relacional.
    pub vecinos_circulo_cercano: Vec<VecinoCirculoCercano>,
    /// Clústeres afines inferidos (aliases/contextos compartidos).
    pub clusteres_afines: Vec<String>,
}

/// Parámetros de consulta híbrida (Broad-Phase vectorial + Narrow-Phase topológica 4D).
#[derive(Debug, Clone)]
pub struct ConsultaHibrida {
    /// Texto de intención comercial o perfil buscado en español.
    pub texto_consulta: String,
    /// Isla objetivo opcional (`None` consulta todas las islas en paralelo).
    pub isla_objetivo: Option<IslaVertical>,
    /// Candidatos recuperados en la fase ancha vectorial.
    pub top_k_broad: usize,
    /// Subgrafos finales emitidos tras re-ranking 4D.
    pub top_k_final: usize,
    /// Profundidad máxima de expansión en el grafo (1 o 2 saltos).
    pub max_saltos_grafo: usize,
    /// Marca temporal de referencia para el decaimiento 4D.
    pub ahora: DateTime<Utc>,
}

impl ConsultaHibrida {
    /// Crea una consulta híbrida con valores recomendados para latencia <15 ms.
    #[must_use]
    pub fn nueva(texto_consulta: impl Into<String>, isla_objetivo: Option<IslaVertical>) -> Self {
        Self {
            texto_consulta: texto_consulta.into(),
            isla_objetivo,
            top_k_broad: 32,
            top_k_final: 10,
            max_saltos_grafo: 2,
            ahora: Utc::now(),
        }
    }
}

/// Motor de recuperación híbrida en dos fases para alimentar a BOB en Fase 3.
#[derive(Debug, Clone)]
pub struct MotorRecuperacion {
    /// Peso de la similitud semántica en el score compuesto.
    pub peso_similitud: f32,
    /// Peso de la vigencia temporal 4D propia del prospecto.
    pub peso_temporal: f32,
    /// Peso de la propagación de oportunidades del Círculo Cercano.
    pub peso_circulo: f32,
}

impl Default for MotorRecuperacion {
    fn default() -> Self {
        Self {
            peso_similitud: 0.40,
            peso_temporal: 0.40,
            peso_circulo: 0.20,
        }
    }
}

impl MotorRecuperacion {
    /// Ejecuta Broad-Phase vectorial + Narrow-Phase topológica 4D devolviendo `SubgrafoProspecto`
    /// ordenados de mayor a menor relevancia estratégica.
    #[tracing::instrument(skip(self, motor, consulta), fields(isla = ?consulta.isla_objetivo, top_k_broad = consulta.top_k_broad, top_k_final = consulta.top_k_final))]
    pub fn consultar(
        &self,
        motor: &MotorMemoriaAurora,
        consulta: &ConsultaHibrida,
    ) -> MemoryResult<Vec<SubgrafoProspecto>> {
        let inicio = Instant::now();
        let _span = info_span!("motor_recuperacion_consultar").entered();

        if consulta.top_k_final == 0 || motor.islas().total_vectores() == 0 {
            return Ok(Vec::new());
        }

        // 1. Broad-Phase vectorial en CPU/SIMD
        let query_vec = motor.embedder().embed(&consulta.texto_consulta)?;
        let candidatos = motor.islas().buscar(
            consulta.isla_objetivo,
            &query_vec,
            consulta.top_k_broad.max(consulta.top_k_final),
        )?;

        // 2. Narrow-Phase: decaimiento 4D + propagación de Círculo Cercano 1–2 saltos
        let mut subgrafos = Vec::with_capacity(candidatos.len());
        let grafo = motor.grafo();

        for cand in candidatos {
            let Some(reg_nodo) = grafo.obtener_nodo(cand.node_id) else {
                continue;
            };
            if motor.esta_en_blacklist(reg_nodo.prospecto.id) {
                continue;
            }

            let similitud_norm = ((cand.similitud_coseno + 1.0) * 0.5).clamp(0.0, 1.0);
            let score_temporal_4d = grafo.relevancia_temporal_nodo(cand.node_id, consulta.ahora);
            let (boost_circulo_cercano, vecinos_prop) = grafo.propagar_senal_circulo_cercano(
                cand.node_id,
                consulta.ahora,
                consulta.max_saltos_grafo,
            );

            let mut gatillos_vigentes: Vec<GatilloVigente> = reg_nodo
                .prospecto
                .gatillos
                .iter()
                .map(|g| {
                    let peso_vigente_4d = grafo.peso_gatillo_vigente(g, consulta.ahora);
                    let antiguedad_horas =
                        ((consulta.ahora - g.ocurrido_en).num_seconds().max(0) as f64) / 3600.0;
                    GatilloVigente {
                        gatillo: g.clone(),
                        peso_vigente_4d,
                        antiguedad_horas,
                    }
                })
                .collect();

            gatillos_vigentes.sort_by(|a, b| {
                b.peso_vigente_4d
                    .partial_cmp(&a.peso_vigente_4d)
                    .unwrap_or(Ordering::Equal)
            });

            let mut clusteres_set = BTreeSet::new();
            for alias in &reg_nodo.prospecto.aliases {
                clusteres_set.insert(alias.clone());
            }

            let vecinos_circulo_cercano: Vec<VecinoCirculoCercano> = vecinos_prop
                .into_iter()
                .map(|v| {
                    clusteres_set.insert(v.contexto_compartido.clone());
                    VecinoCirculoCercano {
                        prospecto_id: v.prospecto_id,
                        nombre_normalizado: v.nombre_normalizado,
                        tipo_vinculo: v.tipo_vinculo,
                        fuerza_vinculo: v.fuerza_ruta,
                        contexto_compartido: v.contexto_compartido,
                        salto: v.salto,
                        relevancia_propia_4d: v.relevancia_propia_4d,
                    }
                })
                .collect();

            let score_compuesto = self.peso_similitud * similitud_norm
                + self.peso_temporal * score_temporal_4d
                + self.peso_circulo * boost_circulo_cercano;

            subgrafos.push(SubgrafoProspecto {
                prospecto: reg_nodo.prospecto.clone(),
                isla: cand.isla,
                score_compuesto,
                similitud_vectorial: similitud_norm,
                score_temporal_4d,
                boost_circulo_cercano,
                gatillos_vigentes,
                vecinos_circulo_cercano,
                clusteres_afines: clusteres_set.into_iter().collect(),
            });
        }

        subgrafos.sort_by(|a, b| {
            b.score_compuesto
                .partial_cmp(&a.score_compuesto)
                .unwrap_or(Ordering::Equal)
        });
        subgrafos.truncate(consulta.top_k_final);

        let elapsed_ms = inicio.elapsed().as_secs_f64() * 1000.0;
        debug!(
            resultados = subgrafos.len(),
            elapsed_ms,
            "Recuperación híbrida 4D completada"
        );
        Ok(subgrafos)
    }
}
