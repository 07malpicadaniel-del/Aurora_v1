use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use aurora_cortex::{
    AjusteEstrategiaAuditor, BobInferenceBackend, OrquestadorComercialBob,
};
use aurora_memory::{IslaVertical, MotorMemoriaAurora};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{PulseError, PulseResult};
use crate::telemetry::{RecolectorTelemetria, ResumenGanchoTelemetria};

/// Parámetros de configuración para la consolidación circadiana y exportación DPO.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigConsolidacionNocturna {
    /// Directorio local en NVMe donde se escriben los lotes `.jsonl` de preferencias DPO.
    pub directorio_salida_dpo: PathBuf,
    /// Límite máximo de campañas auditadas por corrida nocturna.
    pub max_campanas_por_lote: usize,
    /// Umbral mínimo de retención `[0.0, 1.0]` para clasificar un gancho como `chosen`.
    pub umbral_retencion_chosen: f32,
    /// Umbral máximo de retención `[0.0, 1.0]` para clasificar un gancho como `rejected`.
    pub umbral_retencion_rejected: f32,
}

impl Default for ConfigConsolidacionNocturna {
    fn default() -> Self {
        Self {
            directorio_salida_dpo: PathBuf::from("data/dpo_nocturno"),
            max_campanas_por_lote: 32,
            umbral_retencion_chosen: 0.60,
            umbral_retencion_rejected: 0.25,
        }
    }
}

/// Par de preferencia DPO libre de PII compatible con entrenamiento local de BOB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntradaPreferenciaDpo {
    /// Marca temporal UTC de generación de la preferencia.
    pub timestamp: DateTime<Utc>,
    /// Campaña origen asociada al contraste creativo.
    pub campana_id: Uuid,
    /// Etiqueta estable de la isla vectorial vertical.
    pub isla: String,
    /// Prompt de diseño creativo sin datos personales ni PII.
    pub prompt: String,
    /// Gancho/guion ganador (`chosen`) validado por alta retención o respuesta.
    pub chosen: String,
    /// Gancho/guion perdedor (`rejected`) de baja retención o bajo engagement.
    pub rejected: String,
    /// Tasa promedio de retención del ejemplo `chosen`.
    pub tasa_retencion_chosen: f32,
    /// Tasa promedio de retención del ejemplo `rejected`.
    pub tasa_retencion_rejected: f32,
    /// Factor multiplicador emitido por `RolCognitivo::Auditor` durante la auditoría.
    pub ajuste_auditor_factor: f32,
}

/// Resultado integral de una ejecución nocturna del ciclo DPO y recalibración 4D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetPreferenciasDpo {
    /// Ruta absoluta o relativa del archivo `.jsonl` persistido en NVMe (o directorio DPO en modo no-op).
    pub ruta_jsonl: PathBuf,
    /// Entradas DPO escritas en el archivo.
    pub entradas: Vec<EntradaPreferenciaDpo>,
    /// Ajustes estratégicos devueltos por BOB en rol `Auditor`.
    pub campanas_auditadas: Vec<AjusteEstrategiaAuditor>,
    /// Cantidad total de recalibraciones aplicadas sobre nodos del `GrafoTemporal4D`.
    pub nodos_4d_recalibrados: usize,
    /// Cantidad total de recalibraciones aplicadas sobre hiperaristas del `GrafoTemporal4D`.
    pub aristas_4d_recalibradas: usize,
    /// Marca temporal UTC de cierre del lote nocturno.
    pub generado_en: DateTime<Utc>,
    /// Indica si la ejecución terminó en modo idempotente *no-op* por falta de eventos nuevos.
    #[serde(default)]
    pub modo_noop: bool,
    /// Mensaje resumen en español para operadores y API.
    #[serde(default)]
    pub mensaje: String,
}

/// Alias semántico del reporte de consolidación nocturna DPO.
pub type ReporteConsolidacionDpo = DatasetPreferenciasDpo;

/// Orquestador nocturno que conecta telemetría real, `RolCognitivo::Auditor` de BOB,
/// recalibración del `GrafoTemporal4D` y exportación de preferencias DPO en JSONL.
#[derive(Debug)]
pub struct MotorConsolidacionNocturna<B: BobInferenceBackend> {
    orquestador: OrquestadorComercialBob<B>,
    config: ConfigConsolidacionNocturna,
}

impl<B: BobInferenceBackend> MotorConsolidacionNocturna<B> {
    /// Construye el motor de consolidación nocturna sobre un backend de inferencia de BOB.
    #[must_use]
    pub const fn new(
        orquestador: OrquestadorComercialBob<B>,
        config: ConfigConsolidacionNocturna,
    ) -> Self {
        Self {
            orquestador,
            config,
        }
    }

    /// Devuelve la configuración activa de consolidación DPO.
    #[must_use]
    pub const fn config(&self) -> &ConfigConsolidacionNocturna {
        &self.config
    }

    /// Alias idempotente para ejecutar la consolidación nocturna de telemetría DPO.
    pub async fn consolidar(
        &self,
        telemetria: &RecolectorTelemetria,
        memoria: &mut MotorMemoriaAurora,
    ) -> PulseResult<ReporteConsolidacionDpo> {
        self.ejecutar_consolidacion_nocturna(telemetria, memoria)
            .await
    }

    /// Ejecuta el ciclo circadiano completo de manera idempotente:
    /// 1. Si no hay campañas ni eventos nuevos de telemetría, retorna un reporte *no-op* sin error.
    /// 2. Consolida métricas por campaña desde `telemetria`.
    /// 3. Invoca a BOB (`RolCognitivo::Auditor`) por gramática GBNF estricta.
    /// 4. Recalibra decaimiento $\lambda$ y pesos/hiperaristas del `GrafoTemporal4D`.
    /// 5. Contrasta ganchos exitosos (`chosen`) vs de baja retención (`rejected`) y escribe JSONL DPO.
    #[tracing::instrument(skip(self, telemetria, memoria))]
    pub async fn ejecutar_consolidacion_nocturna(
        &self,
        telemetria: &RecolectorTelemetria,
        memoria: &mut MotorMemoriaAurora,
    ) -> PulseResult<DatasetPreferenciasDpo> {
        let campanas = telemetria.campanas_registradas();
        if campanas.is_empty() || telemetria.total_eventos() == 0 {
            let ahora = Utc::now();
            info!("Consolidación DPO en modo no-op: sin eventos de telemetría nuevos");
            return Ok(DatasetPreferenciasDpo {
                ruta_jsonl: self.config.directorio_salida_dpo.clone(),
                entradas: Vec::new(),
                campanas_auditadas: Vec::new(),
                nodos_4d_recalibrados: 0,
                aristas_4d_recalibradas: 0,
                generado_en: ahora,
                modo_noop: true,
                mensaje: "Sin eventos de telemetría nuevos para consolidar".to_string(),
            });
        }

        let limite = self.config.max_campanas_por_lote.max(1);
        let lote_campanas: Vec<Uuid> = campanas.into_iter().take(limite).collect();

        let mut campanas_auditadas = Vec::with_capacity(lote_campanas.len());
        let mut ajustes_por_campana: HashMap<Uuid, AjusteEstrategiaAuditor> =
            HashMap::with_capacity(lote_campanas.len());
        let mut nodos_4d_recalibrados = 0_usize;
        let mut aristas_4d_recalibradas = 0_usize;

        for campana_id in &lote_campanas {
            let metricas = telemetria.generar_metricas_campana(*campana_id)?;
            let ajuste = self.orquestador.auditar_campana(&metricas).await?;
            let isla_campana = telemetria.isla_campana(*campana_id);

            let (lambda_prev, lambda_nueva) = memoria
                .grafo_mut()
                .recalibrar_decaimiento_nocturno(ajuste.factor_ajuste_urgencia);
            let (nodos_isla, aristas_isla) = memoria
                .grafo_mut()
                .ajustar_pesos_isla_y_relaciones(isla_campana, ajuste.factor_ajuste_urgencia);

            debug!(
                %campana_id,
                ?isla_campana,
                factor = ajuste.factor_ajuste_urgencia,
                lambda_prev,
                lambda_nueva,
                nodos_isla,
                aristas_isla,
                "Campaña auditada y pesos 4D recalibrados en ciclo nocturno"
            );

            nodos_4d_recalibrados += nodos_isla;
            aristas_4d_recalibradas += aristas_isla;
            ajustes_por_campana.insert(*campana_id, ajuste.clone());
            campanas_auditadas.push(ajuste);
        }

        let resumenes_ganchos = telemetria.exportar_estadisticas_ganchos();
        for resumen in &resumenes_ganchos {
            let multiplicador_prospecto =
                if resumen.tasa_retencion_promedio >= self.config.umbral_retencion_chosen
                    || resumen.es_alto_rendimiento
                {
                    Some(1.20_f32)
                } else if resumen.tasa_retencion_promedio <= self.config.umbral_retencion_rejected {
                    Some(0.75_f32)
                } else {
                    None
                };

            if let Some(mult) = multiplicador_prospecto {
                for prospecto_id in &resumen.prospectos_asociados {
                    if !telemetria.esta_prospecto_detenido(*prospecto_id) {
                        aristas_4d_recalibradas += memoria
                            .grafo_mut()
                            .ajustar_peso_prospecto(*prospecto_id, mult);
                    }
                }
            }
        }

        let ahora = Utc::now();
        let entradas = self.construir_pares_dpo(
            &resumenes_ganchos,
            &ajustes_por_campana,
            ahora,
        );

        if entradas.is_empty() {
            let mensaje = "Campañas auditadas en memoria 4D; sin pares contrastantes suficientes para nuevo lote DPO".to_string();
            info!(
                campanas_auditadas = campanas_auditadas.len(),
                nodos_4d_recalibrados,
                aristas_4d_recalibradas,
                "Consolidación DPO finalizada sin nuevo JSONL por falta de contraste"
            );
            return Ok(DatasetPreferenciasDpo {
                ruta_jsonl: self.config.directorio_salida_dpo.clone(),
                entradas: Vec::new(),
                campanas_auditadas,
                nodos_4d_recalibrados,
                aristas_4d_recalibradas,
                generado_en: ahora,
                modo_noop: true,
                mensaje,
            });
        }

        let ruta_jsonl =
            persistir_dataset_jsonl(&self.config.directorio_salida_dpo, &entradas, ahora)?;

        let mensaje = format!(
            "Consolidación nocturna DPO completada: {} preferencias exportadas",
            entradas.len()
        );

        info!(
            ruta_jsonl = %ruta_jsonl.display(),
            pares_dpo = entradas.len(),
            campanas_auditadas = campanas_auditadas.len(),
            nodos_4d_recalibrados,
            aristas_4d_recalibradas,
            "Consolidación nocturna DPO y recalibración 4D completadas"
        );

        Ok(DatasetPreferenciasDpo {
            ruta_jsonl,
            entradas,
            campanas_auditadas,
            nodos_4d_recalibrados,
            aristas_4d_recalibradas,
            generado_en: ahora,
            modo_noop: false,
            mensaje,
        })
    }

    fn construir_pares_dpo(
        &self,
        resumenes: &[ResumenGanchoTelemetria],
        ajustes: &HashMap<Uuid, AjusteEstrategiaAuditor>,
        ahora: DateTime<Utc>,
    ) -> Vec<EntradaPreferenciaDpo> {
        let mut ganadores: Vec<&ResumenGanchoTelemetria> = resumenes
            .iter()
            .filter(|r| {
                r.tasa_retencion_promedio >= self.config.umbral_retencion_chosen
                    || (r.es_alto_rendimiento
                        && r.tasa_retencion_promedio > self.config.umbral_retencion_rejected)
            })
            .collect();

        let mut perdedores: Vec<&ResumenGanchoTelemetria> = resumenes
            .iter()
            .filter(|r| r.tasa_retencion_promedio <= self.config.umbral_retencion_rejected)
            .collect();

        // Respaldo cuando hay contraste marcado aun sin cruzar ambos umbrales exactos.
        if ganadores.is_empty() || perdedores.is_empty() {
            if let (Some(mejor), Some(peor)) = (resumenes.first(), resumenes.last()) {
                if mejor.gancho != peor.gancho
                    && (mejor.tasa_retencion_promedio - peor.tasa_retencion_promedio) >= 0.15
                {
                    if ganadores.is_empty() {
                        ganadores.push(mejor);
                    }
                    if perdedores.is_empty() {
                        perdedores.push(peor);
                    }
                }
            }
        }

        let mut entradas = Vec::new();
        for perdedor in &perdedores {
            let mejor_compat = ganadores
                .iter()
                .copied()
                .find(|g| g.isla == perdedor.isla && g.gancho != perdedor.gancho)
                .or_else(|| {
                    ganadores
                        .iter()
                        .copied()
                        .find(|g| g.gancho != perdedor.gancho)
                });

            let isla_str = perdedor
                .isla
                .unwrap_or(IslaVertical::GeneralAurora)
                .nombre()
                .to_string();

            let ajuste_factor = ajustes
                .get(&perdedor.campana_id)
                .map_or(1.0, |a| a.factor_ajuste_urgencia);

            let prompt = format!(
                "Diseña el gancho de los primeros 3 segundos y enfoque comercial para la isla '{}' en México maximizando retención y respuesta ejecutiva.",
                isla_str
            );

            let rejected_texto = formatear_respuesta_dpo(perdedor);

            if let Some(ganador) = mejor_compat {
                let chosen_texto = formatear_respuesta_dpo(ganador);
                entradas.push(EntradaPreferenciaDpo {
                    timestamp: ahora,
                    campana_id: ganador.campana_id,
                    isla: isla_str,
                    prompt,
                    chosen: chosen_texto,
                    rejected: rejected_texto,
                    tasa_retencion_chosen: ganador.tasa_retencion_promedio,
                    tasa_retencion_rejected: perdedor.tasa_retencion_promedio,
                    ajuste_auditor_factor: ajuste_factor,
                });
            } else if let Some(ajuste) = ajustes.get(&perdedor.campana_id) {
                // Si todavía no hay pieza ganadora real, usamos la mutación propuesta por BOB Auditor.
                let chosen_texto = format!(
                    "Gancho mutado (BOB Auditor): {} | Diagnóstico: {}",
                    ajuste.nuevo_gancho_sugerido, ajuste.diagnostico_rendimiento
                );
                entradas.push(EntradaPreferenciaDpo {
                    timestamp: ahora,
                    campana_id: perdedor.campana_id,
                    isla: isla_str,
                    prompt,
                    chosen: chosen_texto,
                    rejected: rejected_texto,
                    tasa_retencion_chosen: 0.55,
                    tasa_retencion_rejected: perdedor.tasa_retencion_promedio,
                    ajuste_auditor_factor: ajuste.factor_ajuste_urgencia,
                });
            }
        }

        entradas
    }
}

fn formatear_respuesta_dpo(resumen: &ResumenGanchoTelemetria) -> String {
    if resumen.guion_resumen.trim().is_empty() {
        format!(
            "Gancho: {} (retención {:.1}%, apertura/respuesta {:.1}%)",
            resumen.gancho,
            resumen.tasa_retencion_promedio * 100.0,
            resumen.tasa_apertura_o_respuesta * 100.0
        )
    } else {
        format!(
            "Gancho: {} | Guion: {}",
            resumen.gancho, resumen.guion_resumen
        )
    }
}

fn persistir_dataset_jsonl(
    directorio: &PathBuf,
    entradas: &[EntradaPreferenciaDpo],
    ahora: DateTime<Utc>,
) -> PulseResult<PathBuf> {
    fs::create_dir_all(directorio).map_err(|e| {
        PulseError::ConsolidacionDpoFallida(format!(
            "no fue posible crear el directorio DPO '{}': {e}",
            directorio.display()
        ))
    })?;

    let nombre_archivo = format!(
        "aurora_dpo_preferences_{}.jsonl",
        ahora.format("%Y%m%dT%H%M%SZ")
    );
    let ruta = directorio.join(nombre_archivo);
    let file = File::create(&ruta).map_err(|e| {
        PulseError::ConsolidacionDpoFallida(format!(
            "no fue posible crear archivo JSONL '{}': {e}",
            ruta.display()
        ))
    })?;

    {
        let mut writer = BufWriter::new(&file);
        for entrada in entradas {
            let linea = serde_json::to_string(entrada).map_err(|e| {
                PulseError::ConsolidacionDpoFallida(format!(
                    "error serializando entrada DPO: {e}"
                ))
            })?;
            writeln!(writer, "{linea}").map_err(|e| {
                PulseError::ConsolidacionDpoFallida(format!(
                    "error escribiendo archivo JSONL '{}': {e}",
                    ruta.display()
                ))
            })?;
        }
        writer.flush().map_err(|e| {
            PulseError::ConsolidacionDpoFallida(format!(
                "error vaciando buffer JSONL '{}': {e}",
                ruta.display()
            ))
        })?;
    }

    file.sync_all().map_err(|e| {
        PulseError::ConsolidacionDpoFallida(format!(
            "error sincronizando JSONL DPO '{}' a disco NVMe: {e}",
            ruta.display()
        ))
    })?;

    Ok(ruta)
}
