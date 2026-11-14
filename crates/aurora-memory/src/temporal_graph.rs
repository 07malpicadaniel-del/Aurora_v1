use std::collections::{HashMap, HashSet, VecDeque};

use aurora_domain::{
    uuid_a_node_id_u64, validar_prospecto, validar_relacion, GatilloVital, Prospecto,
    RelacionTopologica, TipoVinculo,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info_span};
use uuid::Uuid;

use crate::error::{MemoryError, MemoryResult};
use crate::islands::IslaVertical;

/// Parámetros calibrados del decaimiento temporal 4D: $w(t) = w_0 \cdot e^{-\lambda_{\text{eff}} \Delta t}$.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfigDecaimiento4D {
    /// Tasa base de decaimiento por día ($\lambda$).
    pub lambda_por_dia: f32,
    /// Factor de atenuación para señales propagadas a 1 salto (Círculo Cercano directo).
    pub atenuacion_salto_1: f32,
    /// Factor de atenuación para señales propagadas a 2 saltos.
    pub atenuacion_salto_2: f32,
}

impl Default for ConfigDecaimiento4D {
    fn default() -> Self {
        Self {
            lambda_por_dia: 0.035,
            atenuacion_salto_1: 0.55,
            atenuacion_salto_2: 0.25,
        }
    }
}

/// Nodo de prospecto indexado dentro del hipergrafo temporal 4D.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistroNodoProspecto {
    /// Identificador `u64` determinista compatible con `bob_betta/vector_db`.
    pub node_id: u64,
    /// Prospecto canónico validado.
    pub prospecto: Prospecto,
    /// Isla vertical asignada.
    pub isla: IslaVertical,
}

/// Arista de vecindad en el grafo para propagación rápida de 1 y 2 saltos.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AristaTelarana {
    /// Nodo conectado (`u64`).
    pub vecino_node_id: u64,
    /// UUID del vecino conectado.
    pub vecino_prospecto_id: Uuid,
    /// Tipo de vínculo del Círculo Cercano.
    pub tipo_vinculo: TipoVinculo,
    /// Fuerza de arista en `[0.0, 1.0]`.
    pub fuerza: f32,
    /// Contexto compartido.
    pub contexto_compartido: String,
}

/// Vecino alcanzado durante una propagación BFS de 1 o 2 saltos.
#[derive(Debug, Clone)]
pub struct VecinoPropagado {
    /// Nodo del vecino alcanzado.
    pub node_id: u64,
    /// UUID del vecino.
    pub prospecto_id: Uuid,
    /// Nombre normalizado del vecino.
    pub nombre_normalizado: String,
    /// Tipo de vínculo por el que se alcanzó al vecino.
    pub tipo_vinculo: TipoVinculo,
    /// Fuerza efectiva acumulada en la ruta.
    pub fuerza_ruta: f32,
    /// Contexto compartido.
    pub contexto_compartido: String,
    /// Distancia en saltos (`1` o `2`).
    pub salto: u8,
    /// Relevancia temporal 4D propia del vecino.
    pub relevancia_propia_4d: f32,
    /// Contribución de oportunidad propagada hacia el nodo origen.
    pub senal_aportada: f32,
}

/// Hipergrafo causal 4D ("La Telaraña") en RAM.
#[derive(Debug, Clone)]
pub struct GrafoTemporal4D {
    config: ConfigDecaimiento4D,
    nodos: HashMap<u64, RegistroNodoProspecto>,
    vecinos: HashMap<u64, Vec<AristaTelarana>>,
    relaciones_unicas: HashMap<(u64, u64, TipoVinculo), RelacionTopologica>,
}

impl GrafoTemporal4D {
    /// Crea un grafo temporal vacío con la configuración de decaimiento 4D dada.
    #[must_use]
    pub fn new(config: ConfigDecaimiento4D) -> Self {
        Self {
            config,
            nodos: HashMap::new(),
            vecinos: HashMap::new(),
            relaciones_unicas: HashMap::new(),
        }
    }

    /// Devuelve la configuración activa de decaimiento 4D.
    #[must_use]
    pub const fn config(&self) -> &ConfigDecaimiento4D {
        &self.config
    }

    /// Inserta o actualiza un `Prospecto` tras validar cumplimiento LFPDPPP.
    pub fn upsert_prospecto(
        &mut self,
        prospecto: Prospecto,
        isla: IslaVertical,
    ) -> MemoryResult<u64> {
        validar_prospecto(&prospecto)?;
        let node_id = uuid_a_node_id_u64(prospecto.id);

        if let Some(existente) = self.nodos.get(&node_id) {
            if existente.prospecto.id != prospecto.id {
                return Err(MemoryError::GrafoCausal(format!(
                    "colisión de NodeId {node_id} entre {} y {}",
                    existente.prospecto.id, prospecto.id
                )));
            }
        }

        self.nodos.insert(
            node_id,
            RegistroNodoProspecto {
                node_id,
                prospecto,
                isla,
            },
        );
        self.vecinos.entry(node_id).or_default();
        Ok(node_id)
    }

    /// Inserta una `RelacionTopologica` validada y actualiza las listas de adyacencia para 1–2 saltos.
    pub fn insertar_relacion(&mut self, relacion: RelacionTopologica) -> MemoryResult<()> {
        validar_relacion(&relacion)?;

        let origen_id = uuid_a_node_id_u64(relacion.origen_id);
        let destino_id = uuid_a_node_id_u64(relacion.destino_id);

        if !self.nodos.contains_key(&origen_id) {
            return Err(MemoryError::ProspectoNoEncontrado(relacion.origen_id));
        }
        if !self.nodos.contains_key(&destino_id) {
            return Err(MemoryError::ProspectoNoEncontrado(relacion.destino_id));
        }

        let clave_canonica = if origen_id <= destino_id {
            (origen_id, destino_id, relacion.tipo_vinculo)
        } else {
            (destino_id, origen_id, relacion.tipo_vinculo)
        };

        if let Some(prev) = self.relaciones_unicas.get(&clave_canonica) {
            if prev.fuerza >= relacion.fuerza {
                return Ok(());
            }
        }
        self.relaciones_unicas
            .insert(clave_canonica, relacion.clone());

        upsert_arista(
            self.vecinos.entry(origen_id).or_default(),
            AristaTelarana {
                vecino_node_id: destino_id,
                vecino_prospecto_id: relacion.destino_id,
                tipo_vinculo: relacion.tipo_vinculo,
                fuerza: relacion.fuerza,
                contexto_compartido: relacion.contexto_compartido.clone(),
            },
        );
        upsert_arista(
            self.vecinos.entry(destino_id).or_default(),
            AristaTelarana {
                vecino_node_id: origen_id,
                vecino_prospecto_id: relacion.origen_id,
                tipo_vinculo: relacion.tipo_vinculo,
                fuerza: relacion.fuerza,
                contexto_compartido: relacion.contexto_compartido,
            },
        );

        Ok(())
    }

    /// Calcula el peso vigente de un `GatilloVital` en `ahora` según $w(t) = w_0 \cdot e^{-\lambda_{\text{eff}} \Delta t}$.
    #[must_use]
    pub fn peso_gatillo_vigente(&self, gatillo: &GatilloVital, ahora: DateTime<Utc>) -> f32 {
        let delta_secs = (ahora - gatillo.ocurrido_en).num_seconds().max(0) as f32;
        let delta_dias = delta_secs / 86_400.0;
        let urgencia = gatillo.peso_urgencia.clamp(0.0, 1.0);
        // Mayor urgencia inicial reduce levemente el decaimiento efectivo para sostener ventanas críticas.
        let lambda_eff = self.config.lambda_por_dia * (1.25 - 0.50 * urgencia);
        (urgencia * (-lambda_eff * delta_dias).exp()).clamp(0.0, 1.0)
    }

    /// Calcula la relevancia temporal 4D acumulada de un nodo combinando su gatillo más fuerte
    /// y la contribución marginal de gatillos adicionales vigentes.
    #[must_use]
    pub fn relevancia_temporal_nodo(&self, node_id: u64, ahora: DateTime<Utc>) -> f32 {
        let Some(nodo) = self.nodos.get(&node_id) else {
            return 0.0;
        };
        if nodo.prospecto.gatillos.is_empty() {
            return 0.0;
        }

        let mut pesos: Vec<f32> = nodo
            .prospecto
            .gatillos
            .iter()
            .map(|g| self.peso_gatillo_vigente(g, ahora))
            .collect();
        pesos.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        let principal = pesos[0];
        let bonus: f32 = pesos.iter().skip(1).take(3).map(|p| p * 0.20).sum();
        (principal + bonus).clamp(0.0, 1.25)
    }

    /// Propaga señales de oportunidad del Círculo Cercano a 1 y 2 saltos alrededor de `node_id`.
    #[tracing::instrument(skip(self), fields(node_id, max_saltos))]
    pub fn propagar_senal_circulo_cercano(
        &self,
        node_id: u64,
        ahora: DateTime<Utc>,
        max_saltos: usize,
    ) -> (f32, Vec<VecinoPropagado>) {
        let _span = info_span!("propagar_circulo_cercano", node_id).entered();
        if max_saltos == 0 || !self.nodos.contains_key(&node_id) {
            return (0.0, Vec::new());
        }

        let mut visitados = HashSet::new();
        visitados.insert(node_id);

        let mut cola: VecDeque<(u64, u8, f32)> = VecDeque::new();
        cola.push_back((node_id, 0, 1.0));

        let mut propagados = Vec::new();
        let mut boost_total = 0.0f32;

        while let Some((actual, salto_actual, fuerza_acum)) = cola.pop_front() {
            if usize::from(salto_actual) >= max_saltos {
                continue;
            }
            let Some(aristas) = self.vecinos.get(&actual) else {
                continue;
            };

            let siguiente_salto = salto_actual + 1;
            let atenuacion = match siguiente_salto {
                1 => self.config.atenuacion_salto_1,
                _ => self.config.atenuacion_salto_2,
            };

            for arista in aristas {
                if !visitados.insert(arista.vecino_node_id) {
                    continue;
                }
                let Some(nodo_vecino) = self.nodos.get(&arista.vecino_node_id) else {
                    continue;
                };

                let fuerza_ruta = (fuerza_acum * arista.fuerza).clamp(0.0, 1.0);
                let rel_propia = self.relevancia_temporal_nodo(arista.vecino_node_id, ahora);
                let senal = rel_propia * fuerza_ruta * atenuacion;
                boost_total += senal;

                propagados.push(VecinoPropagado {
                    node_id: arista.vecino_node_id,
                    prospecto_id: nodo_vecino.prospecto.id,
                    nombre_normalizado: nodo_vecino.prospecto.nombre_normalizado.clone(),
                    tipo_vinculo: arista.tipo_vinculo,
                    fuerza_ruta,
                    contexto_compartido: arista.contexto_compartido.clone(),
                    salto: siguiente_salto,
                    relevancia_propia_4d: rel_propia,
                    senal_aportada: senal,
                });

                if usize::from(siguiente_salto) < max_saltos {
                    cola.push_back((arista.vecino_node_id, siguiente_salto, fuerza_ruta));
                }
            }
        }

        propagados.sort_by(|a, b| {
            b.senal_aportada
                .partial_cmp(&a.senal_aportada)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        debug!(vecinos = propagados.len(), boost_total, "Propagación 4D completada");
        (boost_total.clamp(0.0, 1.5), propagados)
    }

    /// Elimina físicamente un nodo y todas sus aristas entrantes/salientes del grafo.
    pub fn eliminar_nodo_y_aristas(&mut self, prospecto_id: Uuid) -> Option<u64> {
        let node_id = uuid_a_node_id_u64(prospecto_id);
        let eliminado = self.nodos.remove(&node_id)?;

        if let Some(aristas) = self.vecinos.remove(&node_id) {
            for arista in aristas {
                if let Some(lista_vecino) = self.vecinos.get_mut(&arista.vecino_node_id) {
                    lista_vecino.retain(|a| a.vecino_node_id != node_id);
                }
            }
        }

        self.relaciones_unicas
            .retain(|(a, b, _), _| *a != node_id && *b != node_id);

        Some(eliminado.node_id)
    }

    /// Obtiene una referencia al registro de nodo por `node_id`.
    #[must_use]
    pub fn obtener_nodo(&self, node_id: u64) -> Option<&RegistroNodoProspecto> {
        self.nodos.get(&node_id)
    }

    /// Obtiene una referencia al registro de nodo por `Uuid`.
    #[must_use]
    pub fn obtener_por_uuid(&self, prospecto_id: Uuid) -> Option<&RegistroNodoProspecto> {
        self.nodos.get(&uuid_a_node_id_u64(prospecto_id))
    }

    /// Número de prospectos activos en el grafo.
    #[must_use]
    pub fn len_nodos(&self) -> usize {
        self.nodos.len()
    }

    /// Número de relaciones únicas activas en el grafo.
    #[must_use]
    pub fn len_relaciones(&self) -> usize {
        self.relaciones_unicas.len()
    }

    /// Itera sobre las relaciones canónicas activas (para compactación de snapshots).
    pub fn iter_relaciones(&self) -> impl Iterator<Item = &RelacionTopologica> {
        self.relaciones_unicas.values()
    }
}

fn upsert_arista(lista: &mut Vec<AristaTelarana>, nueva: AristaTelarana) {
    if let Some(prev) = lista.iter_mut().find(|a| {
        a.vecino_node_id == nueva.vecino_node_id && a.tipo_vinculo == nueva.tipo_vinculo
    }) {
        if nueva.fuerza > prev.fuerza {
            prev.fuerza = nueva.fuerza;
            prev.contexto_compartido = nueva.contexto_compartido;
        }
    } else {
        lista.push(nueva);
    }
}
