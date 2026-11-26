use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::state::AuroraDaemonState;

/// Estado operativo del scheduler circadiano autónomo AFK (DPO + Carril Verde)
/// expuesto en `/api/v1/pulse/estado`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EstadoSchedulerDpo {
    /// Indica si el worker nocturno autónomo DPO está activo.
    pub habilitado: bool,
    /// Hora UTC preferida de anclaje diario (`0..=23`, por defecto `9` = `03:00 CST` MX).
    pub hora_utc_preferida: u8,
    /// Intervalo entre corridas DPO en minutos (por defecto `360` = cada 6 horas).
    pub intervalo_minutos: u64,
    /// Próxima marca temporal UTC planificada de ejecución DPO.
    pub proxima_ejecucion: Option<DateTime<Utc>>,
    /// Última marca temporal UTC en la que finalizó una corrida automática.
    pub ultima_ejecucion: Option<DateTime<Utc>>,
    /// Número acumulado de ciclos DPO completados por el scheduler desde el arranque.
    pub ciclos_completados: u64,
    /// Resumen textual del último ciclo ejecutado (o mensaje de error capturado).
    pub ultimo_resultado: Option<String>,
    /// Indica si la cacería OSINT periódica (Carril Verde) está habilitada.
    #[serde(default)]
    pub hunt_programado_habilitado: bool,
    /// Total de cacerías OSINT programadas completadas desde el arranque.
    #[serde(default)]
    pub hunts_ejecutados: u64,
    /// Indica si la revisión periódica de la cola orgánica (Carril Verde) está habilitada.
    #[serde(default)]
    pub publicacion_organica_habilitada: bool,
    /// Total de publicaciones orgánicas despachadas por el scheduler.
    #[serde(default)]
    pub publicaciones_organicas_procesadas: u64,
}

/// Manejador de ciclo de vida del worker DPO en segundo plano con cancelación limpia.
pub struct SchedulerDpoHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: Option<JoinHandle<()>>,
}

impl SchedulerDpoHandle {
    /// Señaliza el apagado ordenado del scheduler (`Ctrl+C` / `SIGTERM`) y espera su salida.
    pub async fn detener_y_esperar(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }
}

/// Calcula la próxima marca temporal UTC alineada a `hora_utc` y con cadencia `intervalo_minutos`.
#[must_use]
pub fn calcular_proxima_ejecucion(
    ahora: DateTime<Utc>,
    hora_utc: u8,
    intervalo_minutos: u64,
) -> DateTime<Utc> {
    let paso_min = intervalo_minutos.max(1) as i64;
    let paso = chrono::Duration::minutes(paso_min);
    let hora_segura = u32::from(hora_utc.min(23));

    let ancla_hoy = Utc
        .with_ymd_and_hms(
            ahora.format("%Y").to_string().parse::<i32>().unwrap_or(2026),
            ahora.format("%m").to_string().parse::<u32>().unwrap_or(1),
            ahora.format("%d").to_string().parse::<u32>().unwrap_or(1),
            hora_segura,
            0,
            0,
        )
        .single()
        .unwrap_or(ahora + paso);

    let mut candidato = ancla_hoy;
    if candidato <= ahora {
        while candidato <= ahora {
            candidato += paso;
        }
    } else if paso_min < 1440 {
        while candidato - paso > ahora {
            candidato -= paso;
        }
    }
    candidato
}

/// Inicia el worker circadiano AFK (DPO nocturno + cacerías OSINT y cola orgánica de Carril Verde)
/// respetando `AuroraDaemonConfig` y exclusión mutua sobre `slot_id = 0` de BOB.
#[must_use]
pub fn iniciar_scheduler_dpo(state: Arc<AuroraDaemonState>) -> SchedulerDpoHandle {
    let cfg = state.config().clone();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let hunt_activo =
        cfg.scheduler.hunt_programado_habilitado && !cfg.scheduler.plazas_osint.is_empty();
    let organico_activo = cfg.scheduler.publicacion_organica_habilitada;

    state.actualizar_estado_scheduler(|est| {
        est.hunt_programado_habilitado = hunt_activo;
        est.publicacion_organica_habilitada = organico_activo;
    });

    if !cfg.dpo_scheduler_habilitado && !hunt_activo && !organico_activo {
        state.actualizar_estado_scheduler(|est| {
            est.habilitado = false;
            est.hora_utc_preferida = cfg.dpo_hora_utc;
            est.intervalo_minutos = cfg.dpo_intervalo_minutos;
            est.proxima_ejecucion = None;
        });
        info!("Scheduler circadiano AFK deshabilitado por configuración");
        return SchedulerDpoHandle {
            shutdown_tx,
            join_handle: None,
        };
    }

    let hora_utc = cfg.dpo_hora_utc;
    let intervalo_min = cfg.dpo_intervalo_minutos.max(1);
    let primera_ejecucion = calcular_proxima_ejecucion(Utc::now(), hora_utc, intervalo_min);

    state.actualizar_estado_scheduler(|est| {
        est.habilitado = cfg.dpo_scheduler_habilitado;
        est.hora_utc_preferida = hora_utc;
        est.intervalo_minutos = intervalo_min;
        est.proxima_ejecucion = cfg.dpo_scheduler_habilitado.then_some(primera_ejecucion);
    });

    info!(
        dpo = cfg.dpo_scheduler_habilitado,
        hunt_afk = hunt_activo,
        organico_afk = organico_activo,
        hora_utc,
        intervalo_minutos = intervalo_min,
        "Scheduler circadiano AFK (Carril Verde) iniciado en segundo plano"
    );

    let hunt_intervalo = Duration::from_secs(cfg.scheduler.hunt_intervalo_minutos.max(1) * 60);
    let organico_intervalo =
        Duration::from_secs(cfg.scheduler.organico_intervalo_minutos.max(1) * 60);
    let dpo_habilitado = cfg.dpo_scheduler_habilitado;

    let join_handle = tokio::spawn(async move {
        let mut proxima_dpo = primera_ejecucion;
        let mut timer_hunt = tokio::time::interval(hunt_intervalo);
        timer_hunt.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        timer_hunt.tick().await; // consume tick inmediato inicial

        let mut timer_organico = tokio::time::interval(organico_intervalo);
        timer_organico.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        timer_organico.tick().await; // consume tick inmediato inicial

        loop {
            let espera_dpo = if dpo_habilitado {
                let ahora = Utc::now();
                let espera_ms = (proxima_dpo - ahora).num_milliseconds().max(10) as u64;
                Duration::from_millis(espera_ms)
            } else {
                Duration::from_secs(86_400)
            };

            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        info!("Scheduler circadiano AFK detenido limpiamente por señal de apagado");
                        break;
                    }
                }
                () = tokio::time::sleep(espera_dpo), if dpo_habilitado => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    ejecutar_y_registrar_ciclo(&state).await;
                    proxima_dpo = calcular_proxima_ejecucion(Utc::now(), hora_utc, intervalo_min);
                    state.actualizar_estado_scheduler(|est| {
                        est.proxima_ejecucion = Some(proxima_dpo);
                    });
                }
                _ = timer_hunt.tick(), if hunt_activo => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let ejecutadas = state.ejecutar_hunts_programados_afk().await;
                    if ejecutadas > 0 {
                        state.actualizar_estado_scheduler(|est| {
                            est.hunts_ejecutados = est.hunts_ejecutados.saturating_add(ejecutadas);
                        });
                    }
                }
                _ = timer_organico.tick(), if organico_activo => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let publicadas = state.procesar_cola_organica_pendiente().await;
                    if publicadas > 0 {
                        state.actualizar_estado_scheduler(|est| {
                            est.publicaciones_organicas_procesadas = est
                                .publicaciones_organicas_procesadas
                                .saturating_add(publicadas);
                        });
                    }
                }
            }
        }
    });

    SchedulerDpoHandle {
        shutdown_tx,
        join_handle: Some(join_handle),
    }
}

/// Inicia el worker DPO con intervalo corto en `Duration` (útil para pruebas de integración deterministas).
#[must_use]
pub fn iniciar_scheduler_dpo_con_intervalo(
    state: Arc<AuroraDaemonState>,
    intervalo: Duration,
) -> SchedulerDpoHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let intervalo_seguro = intervalo.max(Duration::from_millis(10));
    let delta_chrono =
        chrono::Duration::from_std(intervalo_seguro).unwrap_or_else(|_| chrono::Duration::seconds(1));

    state.actualizar_estado_scheduler(|est| {
        est.habilitado = true;
        est.proxima_ejecucion = Some(Utc::now() + delta_chrono);
    });

    let join_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                () = tokio::time::sleep(intervalo_seguro) => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    ejecutar_y_registrar_ciclo(&state).await;
                    state.actualizar_estado_scheduler(|est| {
                        est.proxima_ejecucion = Some(Utc::now() + delta_chrono);
                    });
                }
            }
        }
    });

    SchedulerDpoHandle {
        shutdown_tx,
        join_handle: Some(join_handle),
    }
}

async fn ejecutar_y_registrar_ciclo(state: &AuroraDaemonState) {
    match state.ejecutar_ciclo_scheduler_dpo().await {
        Ok(resp) => {
            info!(
                campanas_auditadas = resp.campanas_auditadas,
                modo_noop = resp.modo_noop,
                mensaje = %resp.mensaje,
                "Ciclo autónomo DPO completado"
            );
        }
        Err(err) => {
            warn!(error = %err, "Error controlado en ciclo autónomo DPO; el scheduler continuará");
            state.actualizar_estado_scheduler(|est| {
                est.ultima_ejecucion = Some(Utc::now());
                est.ultimo_resultado = Some(format!("Error controlado: {err}"));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calcula_proxima_ejecucion_alineada_a_hora_utc_e_intervalo() {
        let base = Utc
            .with_ymd_and_hms(2026, 11, 21, 10, 15, 0)
            .single()
            .unwrap_or_else(Utc::now);

        // Anclado a 09:00 UTC con intervalo de 360 min (6h) -> 09:00, 15:00, 21:00, 03:00
        let prox = calcular_proxima_ejecucion(base, 9, 360);
        assert_eq!(prox.format("%Y-%m-%d %H:%M").to_string(), "2026-11-21 15:00");

        // Con intervalo diario (1440 min) y base 10:15 -> día siguiente 09:00 UTC
        let prox_diario = calcular_proxima_ejecucion(base, 9, 1440);
        assert_eq!(
            prox_diario.format("%Y-%m-%d %H:%M").to_string(),
            "2026-11-22 09:00"
        );
    }
}
