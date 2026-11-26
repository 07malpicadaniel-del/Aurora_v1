use std::collections::{HashMap, VecDeque};

use aurora_cortex::{CanalContacto, PlataformaCalentamiento};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{PulseError, PulseResult};

/// Canal o red pública gobernada por higiene de cuentas y limitación de tasa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlataformaPulso {
    /// Correo electrónico ejecutivo SMTP/API.
    CorreoEjecutivo,
    /// Mensajería WhatsApp Business.
    WhatsApp,
    /// Interacción y mensajes en LinkedIn.
    LinkedIn,
    /// Interacción en Instagram / Meta Reels.
    Instagram,
    /// Publicación orgánica vertical en YouTube Shorts.
    YouTubeShorts,
    /// Publicación orgánica y monetización vertical en TikTok (Content Posting API v2).
    TikTok,
    /// Interacción pública en X.
    X,
    /// Llamada directa asistida.
    LlamadaDirecta,
    /// Foros o directorios de cámaras empresariales.
    CamaraEmpresarial,
}

impl PlataformaPulso {
    /// Etiqueta estable de plataforma para métricas y observabilidad.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::CorreoEjecutivo => "CorreoEjecutivo",
            Self::WhatsApp => "WhatsApp",
            Self::LinkedIn => "LinkedIn",
            Self::Instagram => "Instagram",
            Self::YouTubeShorts => "YouTubeShorts",
            Self::TikTok => "TikTok",
            Self::X => "X",
            Self::LlamadaDirecta => "LlamadaDirecta",
            Self::CamaraEmpresarial => "CamaraEmpresarial",
        }
    }

    /// Todas las plataformas soportadas por `aurora-pulse`.
    #[must_use]
    pub const fn todas() -> &'static [Self] {
        &[
            Self::CorreoEjecutivo,
            Self::WhatsApp,
            Self::LinkedIn,
            Self::Instagram,
            Self::YouTubeShorts,
            Self::TikTok,
            Self::X,
            Self::LlamadaDirecta,
            Self::CamaraEmpresarial,
        ]
    }
}

impl From<CanalContacto> for PlataformaPulso {
    fn from(canal: CanalContacto) -> Self {
        match canal {
            CanalContacto::CorreoEjecutivo => Self::CorreoEjecutivo,
            CanalContacto::WhatsApp => Self::WhatsApp,
            CanalContacto::LinkedIn => Self::LinkedIn,
            CanalContacto::LlamadaDirecta => Self::LlamadaDirecta,
        }
    }
}

impl From<PlataformaCalentamiento> for PlataformaPulso {
    fn from(plat: PlataformaCalentamiento) -> Self {
        match plat {
            PlataformaCalentamiento::LinkedIn => Self::LinkedIn,
            PlataformaCalentamiento::X => Self::X,
            PlataformaCalentamiento::Instagram => Self::Instagram,
            PlataformaCalentamiento::CamaraEmpresarial => Self::CamaraEmpresarial,
        }
    }
}

/// Política conservadora de tasa y circuit breaker por plataforma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoliticaTasaPlataforma {
    /// Máximo de acciones permitidas en ventana móvil de 1 hora.
    pub cuota_max_hora: usize,
    /// Máximo de acciones permitidas en ventana móvil de 24 horas.
    pub cuota_max_dia: usize,
    /// Separación mínima estricta entre acciones consecutivas (ms).
    pub intervalo_minimo_ms: u64,
    /// Amplitud máxima de jitter aleatorio/determinista añadido (ms).
    pub jitter_max_ms: u64,
    /// Fallos consecutivos tolerados antes de abrir el circuit breaker.
    pub umbral_fallos_circuit_breaker: u32,
    /// Segundos de enfriamiento con el circuit breaker abierto.
    pub enfriamiento_circuit_breaker_s: i64,
}

impl PoliticaTasaPlataforma {
    /// Política recomendada por plataforma para proteger reputación de cuentas y evitar bans.
    #[must_use]
    pub const fn recomendada(plataforma: PlataformaPulso) -> Self {
        match plataforma {
            PlataformaPulso::CorreoEjecutivo => Self {
                cuota_max_hora: 35,
                cuota_max_dia: 180,
                intervalo_minimo_ms: 10_000,
                jitter_max_ms: 4_500,
                umbral_fallos_circuit_breaker: 3,
                enfriamiento_circuit_breaker_s: 1_800,
            },
            PlataformaPulso::WhatsApp => Self {
                cuota_max_hora: 20,
                cuota_max_dia: 90,
                intervalo_minimo_ms: 25_000,
                jitter_max_ms: 8_000,
                umbral_fallos_circuit_breaker: 3,
                enfriamiento_circuit_breaker_s: 3_600,
            },
            PlataformaPulso::LinkedIn => Self {
                cuota_max_hora: 12,
                cuota_max_dia: 45,
                intervalo_minimo_ms: 45_000,
                jitter_max_ms: 15_000,
                umbral_fallos_circuit_breaker: 2,
                enfriamiento_circuit_breaker_s: 3_600,
            },
            PlataformaPulso::Instagram | PlataformaPulso::X => Self {
                cuota_max_hora: 15,
                cuota_max_dia: 60,
                intervalo_minimo_ms: 30_000,
                jitter_max_ms: 10_000,
                umbral_fallos_circuit_breaker: 3,
                enfriamiento_circuit_breaker_s: 2_400,
            },
            PlataformaPulso::YouTubeShorts => Self {
                cuota_max_hora: 4,
                cuota_max_dia: 12,
                intervalo_minimo_ms: 60_000,
                jitter_max_ms: 15_000,
                umbral_fallos_circuit_breaker: 3,
                enfriamiento_circuit_breaker_s: 3_600,
            },
            PlataformaPulso::TikTok => Self {
                cuota_max_hora: 3,
                cuota_max_dia: 10,
                intervalo_minimo_ms: 90_000,
                jitter_max_ms: 20_000,
                umbral_fallos_circuit_breaker: 2,
                enfriamiento_circuit_breaker_s: 3_600,
            },
            PlataformaPulso::LlamadaDirecta | PlataformaPulso::CamaraEmpresarial => Self {
                cuota_max_hora: 10,
                cuota_max_dia: 40,
                intervalo_minimo_ms: 60_000,
                jitter_max_ms: 10_000,
                umbral_fallos_circuit_breaker: 3,
                enfriamiento_circuit_breaker_s: 1_800,
            },
        }
    }
}

/// Instantánea observable del estado actual del limitador para monitoreo y `axis_v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstadoTasaPlataforma {
    /// Plataforma evaluada.
    pub plataforma: PlataformaPulso,
    /// Acciones ejecutadas en los últimos 60 minutos.
    pub usadas_ultima_hora: usize,
    /// Cuota horaria configurada.
    pub cuota_max_hora: usize,
    /// Acciones ejecutadas en las últimas 24 horas.
    pub usadas_ultimo_dia: usize,
    /// Cuota diaria configurada.
    pub cuota_max_dia: usize,
    /// Fallos consecutivos vigentes.
    pub fallos_consecutivos: u32,
    /// Instante hasta el que permanece abierto el circuit breaker (`None` si cerrado).
    pub circuit_abierto_hasta: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct VentanaPlataforma {
    politica: PoliticaTasaPlataforma,
    timestamps_24h: VecDeque<DateTime<Utc>>,
    ultimo_envio: Option<DateTime<Utc>>,
    fallos_consecutivos: u32,
    circuit_abierto_hasta: Option<DateTime<Utc>>,
    prng_estado: u64,
}

impl VentanaPlataforma {
    fn nueva(politica: PoliticaTasaPlataforma, semilla: u64) -> Self {
        Self {
            politica,
            timestamps_24h: VecDeque::new(),
            ultimo_envio: None,
            fallos_consecutivos: 0,
            circuit_abierto_hasta: None,
            prng_estado: semilla ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn podar_antiguos(&mut self, ahora: DateTime<Utc>) {
        let limite_24h = ahora - Duration::hours(24);
        while let Some(&front) = self.timestamps_24h.front() {
            if front <= limite_24h {
                self.timestamps_24h.pop_front();
            } else {
                break;
            }
        }
    }

    fn siguiente_jitter_ms(&mut self) -> u64 {
        let max_j = self.politica.jitter_max_ms;
        if max_j == 0 {
            return 0;
        }
        // Xorshift64 determinista libre de dependencias externas.
        let mut x = self.prng_estado.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.prng_estado = x;
        x % (max_j + 1)
    }
}

/// Controlador thread-safe de tasas por plataforma con jitter acotado y circuit breaker.
#[derive(Debug)]
pub struct ControladorTasaPlataforma {
    estados: Mutex<HashMap<PlataformaPulso, VentanaPlataforma>>,
}

impl Default for ControladorTasaPlataforma {
    fn default() -> Self {
        let mut mapa = HashMap::new();
        for (idx, &plat) in PlataformaPulso::todas().iter().enumerate() {
            mapa.insert(
                plat,
                VentanaPlataforma::nueva(
                    PoliticaTasaPlataforma::recomendada(plat),
                    (idx as u64) + 1,
                ),
            );
        }
        Self {
            estados: Mutex::new(mapa),
        }
    }
}

impl ControladorTasaPlataforma {
    /// Crea un controlador con políticas personalizadas para cada plataforma.
    #[must_use]
    pub fn con_politicas(politicas: HashMap<PlataformaPulso, PoliticaTasaPlataforma>) -> Self {
        let ctrl = Self::default();
        for (plat, pol) in politicas {
            ctrl.configurar_politica(plat, pol);
        }
        ctrl
    }

    /// Reemplaza en caliente la política de tasa y reinicia límites de una plataforma.
    pub fn configurar_politica(&self, plataforma: PlataformaPulso, politica: PoliticaTasaPlataforma) {
        let mut guard = self.estados.lock();
        if let Some(ventana) = guard.get_mut(&plataforma) {
            ventana.politica = politica;
        } else {
            guard.insert(
                plataforma,
                VentanaPlataforma::nueva(politica, 0x517c_c1b7_2722_0a95),
            );
        }
    }

    /// Valida cuotas (hora/día), intervalo mínimo y estado de circuit breaker antes de emitir.
    ///
    /// Si está autorizado, reserva la ranura en la ventana temporal y devuelve el `jitter_ms`
    /// acotado asignado para humanizar la cadencia de salida.
    #[tracing::instrument(skip(self), fields(plataforma = plataforma.etiqueta()))]
    pub fn verificar_y_reservar(
        &self,
        plataforma: PlataformaPulso,
        ahora: DateTime<Utc>,
    ) -> PulseResult<u64> {
        let mut guard = self.estados.lock();
        let ventana = guard
            .entry(plataforma)
            .or_insert_with(|| {
                VentanaPlataforma::nueva(PoliticaTasaPlataforma::recomendada(plataforma), 17)
            });

        // 1. Verificar circuit breaker abierto
        if let Some(reapertura_en) = ventana.circuit_abierto_hasta {
            if ahora < reapertura_en {
                warn!(
                    plataforma = plataforma.etiqueta(),
                    fallos = ventana.fallos_consecutivos,
                    %reapertura_en,
                    "Acceso rechazado: circuit breaker abierto"
                );
                return Err(PulseError::CircuitBreakerAbierto {
                    plataforma: plataforma.etiqueta().to_string(),
                    fallos_consecutivos: ventana.fallos_consecutivos,
                    reapertura_en,
                });
            }
            // Expiró enfriamiento: transición limpia a estado cerrado
            ventana.circuit_abierto_hasta = None;
            ventana.fallos_consecutivos = 0;
        }

        // 2. Ventanas móviles de 24h y 1h
        ventana.podar_antiguos(ahora);
        let total_dia = ventana.timestamps_24h.len();
        if total_dia >= ventana.politica.cuota_max_dia {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: plataforma.etiqueta().to_string(),
                motivo: format!(
                    "cuota diaria ({}/{}) alcanzada",
                    total_dia, ventana.politica.cuota_max_dia
                ),
                esperar_ms: 3_600_000,
            });
        }

        let limite_hora = ahora - Duration::hours(1);
        let en_ultima_hora = ventana
            .timestamps_24h
            .iter()
            .filter(|&&ts| ts > limite_hora)
            .count();
        if en_ultima_hora >= ventana.politica.cuota_max_hora {
            return Err(PulseError::RateLimitPlataforma {
                plataforma: plataforma.etiqueta().to_string(),
                motivo: format!(
                    "cuota horaria ({}/{}) alcanzada",
                    en_ultima_hora, ventana.politica.cuota_max_hora
                ),
                esperar_ms: 60_000,
            });
        }

        // 3. Intervalo mínimo entre acciones consecutivas
        if let Some(ultimo) = ventana.ultimo_envio {
            let transcurrido_ms = (ahora - ultimo).num_milliseconds().max(0) as u64;
            if transcurrido_ms < ventana.politica.intervalo_minimo_ms {
                let esperar_ms = ventana.politica.intervalo_minimo_ms - transcurrido_ms;
                return Err(PulseError::RateLimitPlataforma {
                    plataforma: plataforma.etiqueta().to_string(),
                    motivo: "intervalo mínimo entre acciones no cumplido".to_string(),
                    esperar_ms,
                });
            }
        }

        // 4. Reservar ranura y emitir jitter determinista acotado
        ventana.timestamps_24h.push_back(ahora);
        ventana.ultimo_envio = Some(ahora);
        let jitter_ms = ventana.siguiente_jitter_ms();

        debug!(
            plataforma = plataforma.etiqueta(),
            en_ultima_hora = en_ultima_hora + 1,
            total_dia = total_dia + 1,
            jitter_ms,
            "Ranura de plataforma autorizada"
        );
        Ok(jitter_ms)
    }

    /// Reinicia el contador de fallos consecutivos tras una operación exitosa.
    pub fn registrar_exito(&self, plataforma: PlataformaPulso) {
        let mut guard = self.estados.lock();
        if let Some(ventana) = guard.get_mut(&plataforma) {
            ventana.fallos_consecutivos = 0;
            ventana.circuit_abierto_hasta = None;
        }
    }

    /// Registra un fallo (transporte o HTTP 429) y abre el circuit breaker al alcanzar el umbral.
    pub fn registrar_fallo(
        &self,
        plataforma: PlataformaPulso,
        ahora: DateTime<Utc>,
        es_rate_limit_remoto: bool,
    ) -> u32 {
        let mut guard = self.estados.lock();
        let ventana = guard
            .entry(plataforma)
            .or_insert_with(|| {
                VentanaPlataforma::nueva(PoliticaTasaPlataforma::recomendada(plataforma), 31)
            });

        ventana.fallos_consecutivos = ventana.fallos_consecutivos.saturating_add(1);
        let umbral = ventana.politica.umbral_fallos_circuit_breaker.max(1);
        if ventana.fallos_consecutivos >= umbral {
            let reapertura =
                ahora + Duration::seconds(ventana.politica.enfriamiento_circuit_breaker_s.max(60));
            ventana.circuit_abierto_hasta = Some(reapertura);
            warn!(
                plataforma = plataforma.etiqueta(),
                fallos = ventana.fallos_consecutivos,
                es_rate_limit_remoto,
                %reapertura,
                "Circuit breaker ABIERTO por fallos consecutivos"
            );
        }
        ventana.fallos_consecutivos
    }

    /// Devuelve el estado actual observable de cuotas y circuit breaker para una plataforma.
    #[must_use]
    pub fn estado_plataforma(
        &self,
        plataforma: PlataformaPulso,
        ahora: DateTime<Utc>,
    ) -> EstadoTasaPlataforma {
        let mut guard = self.estados.lock();
        let ventana = guard
            .entry(plataforma)
            .or_insert_with(|| {
                VentanaPlataforma::nueva(PoliticaTasaPlataforma::recomendada(plataforma), 47)
            });
        ventana.podar_antiguos(ahora);
        let limite_hora = ahora - Duration::hours(1);
        let usadas_ultima_hora = ventana
            .timestamps_24h
            .iter()
            .filter(|&&ts| ts > limite_hora)
            .count();

        let circuit_vigente = ventana.circuit_abierto_hasta.filter(|&t| ahora < t);
        EstadoTasaPlataforma {
            plataforma,
            usadas_ultima_hora,
            cuota_max_hora: ventana.politica.cuota_max_hora,
            usadas_ultimo_dia: ventana.timestamps_24h.len(),
            cuota_max_dia: ventana.politica.cuota_max_dia,
            fallos_consecutivos: ventana.fallos_consecutivos,
            circuit_abierto_hasta: circuit_vigente,
        }
    }
}
