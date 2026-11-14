use std::collections::HashSet;

use aurora_domain::{
    validar_prospecto, AuroraError, IntoVectorPayload, Prospecto, RelacionTopologica,
    VectorPayload,
};
use tracing::{debug, info, info_span, warn};
use uuid::Uuid;

use crate::distiller::DestiladorOsint;
use crate::error::OsintResult;
use crate::resolver::{EntidadResuelta, EntityResolver};
use crate::sources::{ConsultaObjetivo, EnjambreOsint};

/// Resultado completo y validado bajo LFPDPPP listo para ingesta en `bob_betta/vector_db` (Fase 2).
#[derive(Debug, Clone)]
pub struct ResultadoIngestaOsint {
    /// Prospectos deduplicados y validados por `aurora-domain`.
    pub prospectos: Vec<Prospecto>,
    /// Proyecciones vectoriales preparadas vía `IntoVectorPayload`.
    pub payloads: Vec<VectorPayload>,
    /// Aristas topológicas del Círculo Cercano validadas.
    pub relaciones: Vec<RelacionTopologica>,
    /// IDs bloqueados por cumplimiento LFPDPPP (`opt_out = true`).
    pub excluidos_opt_out: Vec<Uuid>,
    /// Total de registros crudos recolectados antes de deduplicación.
    pub registros_crudos_totales: usize,
}

/// Orquestador end-to-end de la Fase 1:
/// `EnjambreOsint` -> `DestiladorOsint` -> `EntityResolver` -> `aurora_domain::validation` -> `VectorPayload`.
pub struct PipelineOsint {
    enjambre: EnjambreOsint,
    destilador: DestiladorOsint,
    resolver: EntityResolver,
}

impl PipelineOsint {
    /// Construye el pipeline completo con sus etapas configuradas.
    #[must_use]
    pub fn new(
        enjambre: EnjambreOsint,
        destilador: DestiladorOsint,
        resolver: EntityResolver,
    ) -> Self {
        Self {
            enjambre,
            destilador,
            resolver,
        }
    }

    /// Ejecuta la extracción concurrente, destilación CPU, deduplicación, validación LFPDPPP
    /// y proyección hacia `VectorPayload` + `RelacionTopologica`.
    #[tracing::instrument(skip(self, consulta), fields(ciudad = %consulta.ubicacion.ciudad, vertical = consulta.vertical.etiqueta()))]
    pub async fn ejecutar(&self, consulta: &ConsultaObjetivo) -> OsintResult<ResultadoIngestaOsint> {
        let crudos = self.enjambre.ejecutar(consulta).await?;
        let registros_crudos_totales = crudos.len();

        let destilados = self
            .destilador
            .destilar_lote(&crudos, &consulta.ubicacion)?;

        let entidades = self.resolver.resolver(destilados);
        self.validar_y_proyectar_entidades(entidades, registros_crudos_totales)
    }

    fn validar_y_proyectar_entidades(
        &self,
        entidades: Vec<EntidadResuelta>,
        registros_crudos_totales: usize,
    ) -> OsintResult<ResultadoIngestaOsint> {
        let _span = info_span!("validar_y_proyectar_lote", entidades = entidades.len()).entered();

        let mut entidades_validas = Vec::with_capacity(entidades.len());
        let mut prospectos = Vec::with_capacity(entidades.len());
        let mut payloads = Vec::with_capacity(entidades.len());
        let mut excluidos_opt_out = Vec::new();
        let mut ids_excluidos = HashSet::new();

        for entidad in entidades {
            match validar_prospecto(&entidad.prospecto) {
                Ok(()) => {
                    let payload = entidad.prospecto.to_vector_payload()?;
                    prospectos.push(entidad.prospecto.clone());
                    payloads.push(payload);
                    entidades_validas.push(entidad);
                }
                Err(AuroraError::OptOutActive { prospecto_id }) => {
                    warn!(
                        prospecto_id = %prospecto_id,
                        "Prospecto excluido del lote OSINT por cumplimiento LFPDPPP (opt_out)"
                    );
                    ids_excluidos.insert(prospecto_id);
                    excluidos_opt_out.push(prospecto_id);
                }
                Err(err) => {
                    return Err(err.into());
                }
            }
        }

        let relaciones_brutas = self.resolver.inferir_relaciones(&entidades_validas)?;
        let relaciones: Vec<RelacionTopologica> = relaciones_brutas
            .into_iter()
            .filter(|r| {
                !ids_excluidos.contains(&r.origen_id) && !ids_excluidos.contains(&r.destino_id)
            })
            .collect();

        info!(
            registros_crudos = registros_crudos_totales,
            prospectos_validos = prospectos.len(),
            relaciones = relaciones.len(),
            excluidos_opt_out = excluidos_opt_out.len(),
            "Pipeline OSINT finalizado"
        );
        debug!("Todos los VectorPayload y RelacionTopologica pasaron validación LFPDPPP");

        Ok(ResultadoIngestaOsint {
            prospectos,
            payloads,
            relaciones,
            excluidos_opt_out,
            registros_crudos_totales,
        })
    }
}
