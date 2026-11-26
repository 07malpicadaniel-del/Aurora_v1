use std::cmp::Ordering;
use std::collections::HashMap;

use aurora_domain::{SubverticalInmobiliaria, SubverticalSeguros, VerticalComercial};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, info_span};
use uuid::Uuid;

use crate::embedder::{l2_normalize, simd_dot_product};
use crate::error::{MemoryError, MemoryResult};

/// Segmentación física del espacio vectorial por vertical comercial en Aurora.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IslaVertical {
    /// Proyectos inmobiliarios en preventa.
    InmobiliariaPreventa,
    /// Vivienda residencial y de lujo.
    InmobiliariaResidencial,
    /// Naves industriales, parques logísticos y comercial.
    InmobiliariaComercialIndustrial,
    /// Seguros de vida.
    SegurosVida,
    /// Gastos Médicos Mayores.
    SegurosGmm,
    /// Seguros de Hombre Clave corporativos.
    SegurosHombreClave,
    /// Seguros / fideicomisos educativos.
    SegurosEducativo,
    /// Protección patrimonial y retiro.
    SegurosPatrimonial,
    /// Isla general para prospectos sin campaña vertical asignada.
    GeneralAurora,
}

impl IslaVertical {
    /// Devuelve todas las islas disponibles en el gestor.
    #[must_use]
    pub const fn todas() -> &'static [Self] {
        &[
            Self::InmobiliariaPreventa,
            Self::InmobiliariaResidencial,
            Self::InmobiliariaComercialIndustrial,
            Self::SegurosVida,
            Self::SegurosGmm,
            Self::SegurosHombreClave,
            Self::SegurosEducativo,
            Self::SegurosPatrimonial,
            Self::GeneralAurora,
        ]
    }

    /// Etiqueta estable para observabilidad y serialización.
    #[must_use]
    pub const fn nombre(&self) -> &'static str {
        match self {
            Self::InmobiliariaPreventa => "inmobiliaria_preventa",
            Self::InmobiliariaResidencial => "inmobiliaria_residencial",
            Self::InmobiliariaComercialIndustrial => "inmobiliaria_comercial_industrial",
            Self::SegurosVida => "seguros_vida",
            Self::SegurosGmm => "seguros_gmm",
            Self::SegurosHombreClave => "seguros_hombre_clave",
            Self::SegurosEducativo => "seguros_educativo",
            Self::SegurosPatrimonial => "seguros_patrimonial",
            Self::GeneralAurora => "general_aurora",
        }
    }

    /// Convierte una isla vectorial específica de vuelta a su [`VerticalComercial`] canónica.
    /// Devuelve `None` únicamente para `GeneralAurora`.
    #[must_use]
    pub const fn a_vertical_comercial(&self) -> Option<VerticalComercial> {
        match self {
            Self::InmobiliariaPreventa => {
                Some(VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa))
            }
            Self::InmobiliariaResidencial => Some(VerticalComercial::Inmobiliaria(
                SubverticalInmobiliaria::Residencial,
            )),
            Self::InmobiliariaComercialIndustrial => Some(VerticalComercial::Inmobiliaria(
                SubverticalInmobiliaria::ComercialIndustrial,
            )),
            Self::SegurosVida => Some(VerticalComercial::Seguros(SubverticalSeguros::Vida)),
            Self::SegurosGmm => Some(VerticalComercial::Seguros(SubverticalSeguros::Gmm)),
            Self::SegurosHombreClave => {
                Some(VerticalComercial::Seguros(SubverticalSeguros::HombreClave))
            }
            Self::SegurosEducativo => {
                Some(VerticalComercial::Seguros(SubverticalSeguros::Educativo))
            }
            Self::SegurosPatrimonial => {
                Some(VerticalComercial::Seguros(SubverticalSeguros::Patrimonial))
            }
            Self::GeneralAurora => None,
        }
    }
}

impl From<VerticalComercial> for IslaVertical {
    fn from(vertical: VerticalComercial) -> Self {
        match vertical {
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa) => {
                Self::InmobiliariaPreventa
            }
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial) => {
                Self::InmobiliariaResidencial
            }
            VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial) => {
                Self::InmobiliariaComercialIndustrial
            }
            VerticalComercial::Seguros(SubverticalSeguros::Vida) => Self::SegurosVida,
            VerticalComercial::Seguros(SubverticalSeguros::Gmm) => Self::SegurosGmm,
            VerticalComercial::Seguros(SubverticalSeguros::HombreClave) => Self::SegurosHombreClave,
            VerticalComercial::Seguros(SubverticalSeguros::Educativo) => Self::SegurosEducativo,
            VerticalComercial::Seguros(SubverticalSeguros::Patrimonial) => Self::SegurosPatrimonial,
        }
    }
}

/// Entrada almacenada en una isla vectorial (layout contiguo amigable con caché CPU).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntradaVectorial {
    /// Identificador `u64` determinista compatible con `bob_betta/vector_db`.
    pub node_id: u64,
    /// UUID canónico del prospecto en Aurora.
    pub prospecto_id: Uuid,
    /// Vector L2-normalizado.
    pub vector: Vec<f32>,
    /// Peso base 4D inicial `[1, 100]`.
    pub peso_inicial_4d: u32,
    /// Marca temporal de captura en segundos Unix.
    pub timestamp_captura_unix: u64,
}

/// Coincidencia devuelta por la búsqueda K-NN paralela sobre una isla.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoSimilitud {
    /// Identificador `u64` del nodo.
    pub node_id: u64,
    /// UUID canónico del prospecto.
    pub prospecto_id: Uuid,
    /// Isla donde reside el vector.
    pub isla: IslaVertical,
    /// Similitud coseno en `[-1.0, 1.0]`.
    pub similitud_coseno: f32,
}

/// Isla vectorial independiente con almacenamiento contiguo y eliminación física O(1).
#[derive(Debug, Clone)]
pub struct IslaVectorial {
    isla: IslaVertical,
    dimension: usize,
    entradas: Vec<EntradaVectorial>,
    indice_por_node_id: HashMap<u64, usize>,
}

impl IslaVectorial {
    /// Crea una nueva isla para la vertical y dimensión indicadas.
    #[must_use]
    pub fn new(isla: IslaVertical, dimension: usize) -> Self {
        Self {
            isla,
            dimension,
            entradas: Vec::new(),
            indice_por_node_id: HashMap::new(),
        }
    }

    /// Inserta o actualiza un vector normalizado L2 en O(1) amortizado.
    pub fn upsert(&mut self, mut entrada: EntradaVectorial) -> MemoryResult<()> {
        if entrada.vector.len() != self.dimension {
            return Err(MemoryError::DimensionMismatch {
                esperada: self.dimension,
                recibida: entrada.vector.len(),
            });
        }
        l2_normalize(&mut entrada.vector);

        if let Some(&pos) = self.indice_por_node_id.get(&entrada.node_id) {
            self.entradas[pos] = entrada;
        } else {
            let pos = self.entradas.len();
            self.indice_por_node_id.insert(entrada.node_id, pos);
            self.entradas.push(entrada);
        }
        Ok(())
    }

    /// Elimina físicamente un nodo mediante `swap_remove` sin dejar huecos ni tombstones en RAM.
    pub fn eliminar(&mut self, node_id: u64) -> bool {
        let Some(pos) = self.indice_por_node_id.remove(&node_id) else {
            return false;
        };

        let last_idx = self.entradas.len().saturating_sub(1);
        self.entradas.swap_remove(pos);

        if pos < last_idx {
            let movido_id = self.entradas[pos].node_id;
            self.indice_por_node_id.insert(movido_id, pos);
        }
        true
    }

    /// Número de vectores activos en esta isla.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entradas.len()
    }

    /// Indica si la isla no contiene vectores.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entradas.is_empty()
    }

    /// Devuelve el slice de entradas activas (útil para snapshots compactados).
    #[must_use]
    pub fn entradas(&self) -> &[EntradaVectorial] {
        &self.entradas
    }

    /// Búsqueda K-NN paralela con `rayon` y producto punto SIMD sobre todos los vectores de la isla.
    pub fn buscar_knn(&self, query_l2: &[f32], top_k: usize) -> MemoryResult<Vec<ResultadoSimilitud>> {
        if query_l2.len() != self.dimension {
            return Err(MemoryError::DimensionMismatch {
                esperada: self.dimension,
                recibida: query_l2.len(),
            });
        }
        if top_k == 0 || self.entradas.is_empty() {
            return Ok(Vec::new());
        }

        let mut puntuados: Vec<ResultadoSimilitud> = self
            .entradas
            .par_iter()
            .map(|e| {
                let sim = simd_dot_product(&e.vector, query_l2)?;
                Ok(ResultadoSimilitud {
                    node_id: e.node_id,
                    prospecto_id: e.prospecto_id,
                    isla: self.isla,
                    similitud_coseno: sim,
                })
            })
            .collect::<MemoryResult<Vec<_>>>()?;

        puntuados.sort_by(|a, b| {
            b.similitud_coseno
                .partial_cmp(&a.similitud_coseno)
                .unwrap_or(Ordering::Equal)
        });
        puntuados.truncate(top_k);
        Ok(puntuados)
    }
}

/// Gestor de todas las islas vectoriales comerciales de Aurora.
#[derive(Debug, Clone)]
pub struct GestorIslas {
    dimension: usize,
    islas: HashMap<IslaVertical, IslaVectorial>,
}

impl GestorIslas {
    /// Inicializa todas las islas de `IslaVertical` con la dimensión de embeddings configurada.
    #[must_use]
    pub fn new(dimension: usize) -> Self {
        let mut islas = HashMap::new();
        for &isla in IslaVertical::todas() {
            islas.insert(isla, IslaVectorial::new(isla, dimension));
        }
        Self { dimension, islas }
    }

    /// Dimensión vectorial exigida por todas las islas del gestor.
    #[must_use]
    pub const fn dimension(&self) -> usize {
        self.dimension
    }

    /// Inserta o actualiza un vector en la isla asignada.
    pub fn upsert(&mut self, isla: IslaVertical, entrada: EntradaVectorial) -> MemoryResult<()> {
        let target = self
            .islas
            .get_mut(&isla)
            .ok_or_else(|| MemoryError::IslaNoEncontrada(isla.nombre().to_string()))?;
        target.upsert(entrada)
    }

    /// Elimina físicamente un `node_id` de todas las islas donde pudiera estar indexado.
    pub fn eliminar_de_todas(&mut self, node_id: u64) -> usize {
        let mut eliminados = 0;
        for isla in self.islas.values_mut() {
            if isla.eliminar(node_id) {
                eliminados += 1;
            }
        }
        eliminados
    }

    /// Busca los `top_k` vecinos más cercanos en una isla específica o en todo el espacio vectorial.
    #[tracing::instrument(skip(self, query), fields(isla = ?isla_opt, top_k))]
    pub fn buscar(
        &self,
        isla_opt: Option<IslaVertical>,
        query: &[f32],
        top_k: usize,
    ) -> MemoryResult<Vec<ResultadoSimilitud>> {
        let _span = info_span!("gestor_islas_buscar", top_k).entered();
        if query.len() != self.dimension {
            return Err(MemoryError::DimensionMismatch {
                esperada: self.dimension,
                recibida: query.len(),
            });
        }

        let mut query_norm = query.to_vec();
        l2_normalize(&mut query_norm);

        let mut resultados = match isla_opt {
            Some(isla_key) => {
                let isla = self
                    .islas
                    .get(&isla_key)
                    .ok_or_else(|| MemoryError::IslaNoEncontrada(isla_key.nombre().to_string()))?;
                isla.buscar_knn(&query_norm, top_k)?
            }
            None => {
                let parciales = self
                    .islas
                    .par_iter()
                    .map(|(_, isla)| isla.buscar_knn(&query_norm, top_k))
                    .collect::<MemoryResult<Vec<_>>>()?;

                let mut todos: Vec<ResultadoSimilitud> = parciales.into_iter().flatten().collect();
                todos.sort_by(|a, b| {
                    b.similitud_coseno
                        .partial_cmp(&a.similitud_coseno)
                        .unwrap_or(Ordering::Equal)
                });
                todos.truncate(top_k);
                todos
            }
        };

        resultados.truncate(top_k);
        debug!(encontrados = resultados.len(), "Búsqueda K-NN completada");
        Ok(resultados)
    }

    /// Recuenta el número total de nodos indexados en todas las islas.
    #[must_use]
    pub fn total_vectores(&self) -> usize {
        self.islas.values().map(IslaVectorial::len).sum()
    }

    /// Itera sobre las entradas almacenadas por cada isla (usado en compactación de snapshots).
    pub fn iter_entradas(&self) -> impl Iterator<Item = (IslaVertical, &EntradaVectorial)> {
        self.islas
            .iter()
            .flat_map(|(&isla, vec_isla)| vec_isla.entradas().iter().map(move |e| (isla, e)))
    }
}
