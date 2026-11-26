use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{CouncilError, CouncilResult};

/// Orden canónico de cascada multi-agente, degradando siempre a `BobLocal`.
pub const ORDEN_CASCADA_CONSEJO: [MiembroConsejo; 5] = [
    MiembroConsejo::ClaudePro,
    MiembroConsejo::CodexPlus,
    MiembroConsejo::GeminiEstudiante,
    MiembroConsejo::OpenCodeLocal,
    MiembroConsejo::BobLocal,
];

/// Ventana móvil estándar de protección de suscripciones Pro/Plus (5 horas).
pub const VENTANA_MOVIL_HORAS: i64 = 5;

/// Ventana diaria estándar para el tope diario (24 horas).
pub const VENTANA_DIARIA_HORAS: i64 = 24;

/// Miembros reconocidos del Consejo Multi-Agente de Aurora.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MiembroConsejo {
    /// Suscripción Claude Pro vía CLI headless (`claude -p`).
    ClaudePro,
    /// Suscripción ChatGPT Plus / Codex vía CLI headless (`codex exec`).
    CodexPlus,
    /// Cuenta Gemini Estudiante vía CLI headless (`gemini -p`).
    GeminiEstudiante,
    /// Orquestador CLI local (`opencode run`).
    OpenCodeLocal,
    /// Respaldo soberano local en VRAM RTX 5060 Ti (`slot_id = 0`, nunca se agota).
    BobLocal,
}

impl MiembroConsejo {
    /// Identificador textual estable para logs, métricas y API HTTP.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::ClaudePro => "claude_pro",
            Self::CodexPlus => "codex_plus",
            Self::GeminiEstudiante => "gemini_estudiante",
            Self::OpenCodeLocal => "opencode_local",
            Self::BobLocal => "bob_local",
        }
    }

    /// Devuelve `true` si el miembro opera en nube externa (requiere PII Firewall obligatorio).
    #[must_use]
    pub const fn es_externo_nube(&self) -> bool {
        matches!(
            self,
            Self::ClaudePro | Self::CodexPlus | Self::GeminiEstudiante
        )
    }

    /// Devuelve `true` si toda entrada debe pasar obligatoriamente por `PiiFirewall`.
    ///
    /// Solo `BobLocal` procesa datos reales en VRAM soberana; todos los CLIs externos o
    /// subprocesos secundarios reciben exclusivamente texto sanitizado.
    #[must_use]
    pub const fn requiere_pii_firewall(&self) -> bool {
        !matches!(self, Self::BobLocal)
    }
}

/// Política conservadora de cuota y enfriamiento por proveedor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoliticaCuotaProveedor {
    /// Tope de invocaciones permitidas dentro de la ventana móvil de 5 horas.
    pub max_tareas_ventana_5h: u32,
    /// Tope de invocaciones permitidas dentro de una ventana diaria (24 horas).
    pub max_tareas_diarias: u32,
    /// Cooldown automático en segundos cuando el proveedor reporta rate-limit (429 / usage limit).
    pub cooldown_rate_limit_segundos: u64,
    /// Si es `true`, el proveedor nunca se agota (reservado para `BobLocal`).
    #[serde(default)]
    pub ilimitado: bool,
}

impl PoliticaCuotaProveedor {
    /// Política conservadora predeterminada según suscripción.
    #[must_use]
    pub const fn por_defecto_para(miembro: MiembroConsejo) -> Self {
        match miembro {
            MiembroConsejo::ClaudePro => Self {
                max_tareas_ventana_5h: 10,
                max_tareas_diarias: 30,
                cooldown_rate_limit_segundos: 1_800,
                ilimitado: false,
            },
            MiembroConsejo::CodexPlus => Self {
                max_tareas_ventana_5h: 12,
                max_tareas_diarias: 36,
                cooldown_rate_limit_segundos: 1_800,
                ilimitado: false,
            },
            MiembroConsejo::GeminiEstudiante => Self {
                max_tareas_ventana_5h: 25,
                max_tareas_diarias: 100,
                cooldown_rate_limit_segundos: 900,
                ilimitado: false,
            },
            MiembroConsejo::OpenCodeLocal => Self {
                max_tareas_ventana_5h: 60,
                max_tareas_diarias: 240,
                cooldown_rate_limit_segundos: 300,
                ilimitado: false,
            },
            MiembroConsejo::BobLocal => Self {
                max_tareas_ventana_5h: u32::MAX,
                max_tareas_diarias: u32::MAX,
                cooldown_rate_limit_segundos: 0,
                ilimitado: true,
            },
        }
    }
}

/// Configuración de inicialización del [`QuotaGovernor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCuotasConsejo {
    /// Archivo JSON en NVMe donde se persiste el libro de uso de forma atómica.
    pub ruta_persistencia: PathBuf,
    /// Políticas personalizadas por miembro del consejo.
    pub politicas: BTreeMap<MiembroConsejo, PoliticaCuotaProveedor>,
}

impl ConfigCuotasConsejo {
    /// Crea configuración con las políticas conservadoras por defecto y la ruta indicada.
    #[must_use]
    pub fn con_ruta(ruta_persistencia: impl Into<PathBuf>) -> Self {
        let mut politicas = BTreeMap::new();
        for miembro in ORDEN_CASCADA_CONSEJO {
            politicas.insert(miembro, PoliticaCuotaProveedor::por_defecto_para(miembro));
        }
        Self {
            ruta_persistencia: ruta_persistencia.into(),
            politicas,
        }
    }

    /// Sobrescribe la política de cuota para un miembro específico (`BobLocal` sigue siendo ilimitado).
    #[must_use]
    pub fn con_politica(
        mut self,
        miembro: MiembroConsejo,
        mut politica: PoliticaCuotaProveedor,
    ) -> Self {
        if matches!(miembro, MiembroConsejo::BobLocal) {
            politica.ilimitado = true;
            politica.cooldown_rate_limit_segundos = 0;
        }
        self.politicas.insert(miembro, politica);
        self
    }
}

/// Instantánea serializable del estado de cuotas y enfriamiento de un miembro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstadoCuotaMiembro {
    /// Miembro evaluado.
    pub miembro: MiembroConsejo,
    /// `true` si el miembro puede aceptar una nueva tarea ahora mismo.
    pub disponible: bool,
    /// `true` si no posee límite de cuota (`BobLocal`).
    pub ilimitado: bool,
    /// Invocaciones contabilizadas en la ventana móvil de 5 horas.
    pub usado_ventana_5h: u32,
    /// Límite configurado de la ventana móvil de 5 horas.
    pub limite_ventana_5h: u32,
    /// Invocaciones contabilizadas en las últimas 24 horas.
    pub usado_diario: u32,
    /// Límite diario configurado.
    pub limite_diario: u32,
    /// Marca UTC hasta la cual el proveedor permanece en enfriamiento por rate-limit.
    pub cooldown_hasta: Option<DateTime<Utc>>,
    /// Razón de bloqueo actual cuando `disponible == false`.
    pub motivo_bloqueo: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LibroCuotasPersistido {
    #[serde(default)]
    usos_por_miembro: BTreeMap<MiembroConsejo, Vec<DateTime<Utc>>>,
    #[serde(default)]
    cooldown_hasta: BTreeMap<MiembroConsejo, DateTime<Utc>>,
    #[serde(default)]
    ultimo_motivo_rate_limit: BTreeMap<MiembroConsejo, String>,
    actualizado_en: Option<DateTime<Utc>>,
}

/// Gobernador thread-safe de cuotas móviles de 5 horas, topes diarios y cooldowns en NVMe.
pub struct QuotaGovernor {
    ruta_persistencia: PathBuf,
    politicas: BTreeMap<MiembroConsejo, PoliticaCuotaProveedor>,
    libro: Mutex<LibroCuotasPersistido>,
}

impl QuotaGovernor {
    /// Abre o inicializa el gobernador de cuotas desde disco NVMe.
    pub fn abrir(mut config: ConfigCuotasConsejo) -> CouncilResult<Self> {
        for miembro in ORDEN_CASCADA_CONSEJO {
            config
                .politicas
                .entry(miembro)
                .or_insert_with(|| PoliticaCuotaProveedor::por_defecto_para(miembro));
        }
        if let Some(pol_bob) = config.politicas.get_mut(&MiembroConsejo::BobLocal) {
            pol_bob.ilimitado = true;
            pol_bob.cooldown_rate_limit_segundos = 0;
        }

        let libro = if config.ruta_persistencia.is_file() {
            let contenido = fs::read_to_string(&config.ruta_persistencia)?;
            if contenido.trim().is_empty() {
                LibroCuotasPersistido::default()
            } else {
                serde_json::from_str::<LibroCuotasPersistido>(&contenido).map_err(|e| {
                    CouncilError::SalidaInvalida {
                        miembro: "quota_governor".to_string(),
                        detalle: format!(
                            "estado de cuotas corrupto en '{}': {e}",
                            config.ruta_persistencia.display()
                        ),
                    }
                })?
            }
        } else {
            LibroCuotasPersistido::default()
        };

        Ok(Self {
            ruta_persistencia: config.ruta_persistencia,
            politicas: config.politicas,
            libro: Mutex::new(libro),
        })
    }

    /// Devuelve la ruta NVMe donde se persiste el libro de uso.
    #[must_use]
    pub fn ruta_persistencia(&self) -> &Path {
        &self.ruta_persistencia
    }

    /// Consulta si un miembro puede ejecutar una tarea en el instante actual.
    pub async fn puede_ejecutar(&self, miembro: MiembroConsejo) -> bool {
        self.puede_ejecutar_en(miembro, Utc::now()).await
    }

    /// Consulta determinista de disponibilidad en un instante `ahora` específico.
    pub async fn puede_ejecutar_en(&self, miembro: MiembroConsejo, ahora: DateTime<Utc>) -> bool {
        if matches!(miembro, MiembroConsejo::BobLocal) {
            return true;
        }
        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);
        let estado = self.evaluar_miembro(&libro, miembro, ahora);
        estado.disponible
    }

    /// Registra una invocación exitosa (o consumida) y persiste atómicamente en NVMe.
    pub async fn registrar_uso(
        &self,
        miembro: MiembroConsejo,
    ) -> CouncilResult<EstadoCuotaMiembro> {
        self.registrar_uso_en(miembro, Utc::now()).await
    }

    /// Variante determinista de [`Self::registrar_uso`] con reloj explícito.
    pub async fn registrar_uso_en(
        &self,
        miembro: MiembroConsejo,
        ahora: DateTime<Utc>,
    ) -> CouncilResult<EstadoCuotaMiembro> {
        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);

        if !matches!(miembro, MiembroConsejo::BobLocal) {
            let previo = self.evaluar_miembro(&libro, miembro, ahora);
            if !previo.disponible {
                return Err(CouncilError::CuotaAgotada {
                    miembro: miembro.etiqueta().to_string(),
                    detalle: previo
                        .motivo_bloqueo
                        .unwrap_or_else(|| "cuota temporalmente agotada".to_string()),
                });
            }
        }

        libro.usos_por_miembro.entry(miembro).or_default().push(ahora);
        libro.actualizado_en = Some(ahora);
        persistir_atomico(&self.ruta_persistencia, &libro)?;

        let estado = self.evaluar_miembro(&libro, miembro, ahora);
        debug!(
            miembro = miembro.etiqueta(),
            usado_5h = estado.usado_ventana_5h,
            usado_diario = estado.usado_diario,
            "Consumo de cuota registrado y persistido en NVMe"
        );
        Ok(estado)
    }

    /// Aplica penalización de enfriamiento tras recibir rate-limit externo y persiste en NVMe.
    pub async fn registrar_rate_limit(
        &self,
        miembro: MiembroConsejo,
        motivo: impl Into<String>,
    ) -> CouncilResult<DateTime<Utc>> {
        self.registrar_rate_limit_en(miembro, motivo, Utc::now())
            .await
    }

    /// Variante determinista de [`Self::registrar_rate_limit`] con reloj explícito.
    pub async fn registrar_rate_limit_en(
        &self,
        miembro: MiembroConsejo,
        motivo: impl Into<String>,
        ahora: DateTime<Utc>,
    ) -> CouncilResult<DateTime<Utc>> {
        if matches!(miembro, MiembroConsejo::BobLocal) {
            return Ok(ahora);
        }

        let politica = self.politica_de(miembro);
        let cooldown_seg = politica.cooldown_rate_limit_segundos.max(60);
        let hasta = ahora + Duration::seconds(cooldown_seg.min(i64::MAX as u64) as i64);
        let motivo_str = motivo.into();

        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);
        libro.cooldown_hasta.insert(miembro, hasta);
        libro
            .ultimo_motivo_rate_limit
            .insert(miembro, motivo_str.clone());
        libro.actualizado_en = Some(ahora);
        persistir_atomico(&self.ruta_persistencia, &libro)?;

        warn!(
            miembro = miembro.etiqueta(),
            cooldown_hasta = %hasta,
            motivo = %motivo_str,
            "Penalización de enfriamiento aplicada por rate-limit externo"
        );
        Ok(hasta)
    }

    /// Devuelve el orden de cascada vigente filtrando miembros agotados o en cooldown.
    ///
    /// Garantiza que `BobLocal` siempre esté presente al final como respaldo inagotable.
    pub async fn orden_cascada_vigente(&self) -> Vec<MiembroConsejo> {
        self.orden_cascada_vigente_en(Utc::now()).await
    }

    /// Variante determinista de [`Self::orden_cascada_vigente`] con reloj explícito.
    pub async fn orden_cascada_vigente_en(&self, ahora: DateTime<Utc>) -> Vec<MiembroConsejo> {
        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);

        let mut cascada = Vec::with_capacity(ORDEN_CASCADA_CONSEJO.len());
        for miembro in ORDEN_CASCADA_CONSEJO {
            if matches!(miembro, MiembroConsejo::BobLocal)
                || self.evaluar_miembro(&libro, miembro, ahora).disponible
            {
                cascada.push(miembro);
            }
        }
        cascada
    }

    /// Devuelve la telemetría de cuotas de un miembro específico.
    pub async fn estado_miembro(&self, miembro: MiembroConsejo) -> EstadoCuotaMiembro {
        let ahora = Utc::now();
        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);
        self.evaluar_miembro(&libro, miembro, ahora)
    }

    /// Devuelve el estado completo de ventanas de 5 h, uso diario y cooldowns de todo el consejo.
    pub async fn estado_todas_las_cuotas(&self) -> Vec<EstadoCuotaMiembro> {
        let ahora = Utc::now();
        let mut libro = self.libro.lock().await;
        podar_libro(&mut libro, ahora);

        ORDEN_CASCADA_CONSEJO
            .iter()
            .map(|&miembro| self.evaluar_miembro(&libro, miembro, ahora))
            .collect()
    }

    fn politica_de(&self, miembro: MiembroConsejo) -> PoliticaCuotaProveedor {
        self.politicas
            .get(&miembro)
            .cloned()
            .unwrap_or_else(|| PoliticaCuotaProveedor::por_defecto_para(miembro))
    }

    fn evaluar_miembro(
        &self,
        libro: &LibroCuotasPersistido,
        miembro: MiembroConsejo,
        ahora: DateTime<Utc>,
    ) -> EstadoCuotaMiembro {
        let politica = self.politica_de(miembro);
        let limite_5h_inicio = ahora - Duration::hours(VENTANA_MOVIL_HORAS);
        let limite_24h_inicio = ahora - Duration::hours(VENTANA_DIARIA_HORAS);

        let registros = libro.usos_por_miembro.get(&miembro);
        let usado_5h = registros.map_or(0, |v| {
            v.iter().filter(|&&ts| ts >= limite_5h_inicio).count() as u32
        });
        let usado_24h = registros.map_or(0, |v| {
            v.iter().filter(|&&ts| ts >= limite_24h_inicio).count() as u32
        });

        if matches!(miembro, MiembroConsejo::BobLocal) || politica.ilimitado {
            return EstadoCuotaMiembro {
                miembro,
                disponible: true,
                ilimitado: true,
                usado_ventana_5h: usado_5h,
                limite_ventana_5h: u32::MAX,
                usado_diario: usado_24h,
                limite_diario: u32::MAX,
                cooldown_hasta: None,
                motivo_bloqueo: None,
            };
        }

        let cooldown_activo = libro
            .cooldown_hasta
            .get(&miembro)
            .copied()
            .filter(|&hasta| hasta > ahora);

        let (disponible, motivo_bloqueo) = if let Some(hasta) = cooldown_activo {
            let motivo_rl = libro
                .ultimo_motivo_rate_limit
                .get(&miembro)
                .map_or("rate-limit externo", String::as_str);
            (
                false,
                Some(format!(
                    "en cooldown hasta {} ({motivo_rl})",
                    hasta.to_rfc3339()
                )),
            )
        } else if usado_5h >= politica.max_tareas_ventana_5h {
            (
                false,
                Some(format!(
                    "cuota móvil 5h alcanzada ({usado_5h}/{})",
                    politica.max_tareas_ventana_5h
                )),
            )
        } else if usado_24h >= politica.max_tareas_diarias {
            (
                false,
                Some(format!(
                    "tope diario alcanzado ({usado_24h}/{})",
                    politica.max_tareas_diarias
                )),
            )
        } else {
            (true, None)
        };

        EstadoCuotaMiembro {
            miembro,
            disponible,
            ilimitado: false,
            usado_ventana_5h: usado_5h,
            limite_ventana_5h: politica.max_tareas_ventana_5h,
            usado_diario: usado_24h,
            limite_diario: politica.max_tareas_diarias,
            cooldown_hasta: cooldown_activo,
            motivo_bloqueo,
        }
    }
}

fn podar_libro(libro: &mut LibroCuotasPersistido, ahora: DateTime<Utc>) {
    let corte_24h = ahora - Duration::hours(VENTANA_DIARIA_HORAS);
    for marcas in libro.usos_por_miembro.values_mut() {
        marcas.retain(|&ts| ts >= corte_24h);
    }
    libro.cooldown_hasta.retain(|_, &mut hasta| hasta > ahora);
}

fn persistir_atomico(ruta: &Path, libro: &LibroCuotasPersistido) -> CouncilResult<()> {
    if let Some(padre) = ruta.parent() {
        if !padre.as_os_str().is_empty() {
            fs::create_dir_all(padre)?;
        }
    }

    let nombre_base = ruta
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("quota_state.json");
    let ruta_tmp = ruta.with_file_name(format!(".{nombre_base}.tmp.{}", Uuid::new_v4()));

    let bytes = serde_json::to_vec_pretty(libro).map_err(|e| CouncilError::SalidaInvalida {
        miembro: "quota_governor".to_string(),
        detalle: format!("fallo serializando estado de cuotas: {e}"),
    })?;

    {
        let mut archivo = File::create(&ruta_tmp)?;
        archivo.write_all(&bytes)?;
        archivo.sync_all()?;
    }

    fs::rename(&ruta_tmp, ruta)?;
    info!(ruta = %ruta.display(), "Libro de cuotas persistido atómicamente en NVMe");
    Ok(())
}
