use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use tracing::{debug, info_span};
use uuid::Uuid;

use crate::error::{AuroraError, AuroraResult};
use crate::models::{GatilloVital, Prospecto, TipoGatilloVital};
use crate::validation::validar_prospecto;

/// Escala de peso topológico discreto compatible con `bob_betta/vector_db` (`AtomicU32`).
const PESO_4D_MIN: u32 = 1;
const PESO_4D_MAX: u32 = 100;

/// Isla por defecto recomendada para ingestar nodos de Aurora en `bob_betta/vector_db`.
pub const ISLA_PROSPECTOS_DEFECTO: &str = "aurora_prospectos";

/// Proyección serializada de un `GatilloVital` para alimentación del decaimiento temporal 4D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatilloProyectado {
    /// Tipo de gatillo vital.
    pub tipo: TipoGatilloVital,
    /// Descripción del evento.
    pub descripcion: String,
    /// Marca temporal UTC en segundos Unix (`u64` compatible con `vector_db::Timestamp`).
    pub ocurrido_en_unix: u64,
    /// Peso base de urgencia en `[0.0, 1.0]`.
    pub peso_urgencia: f32,
}

/// Carga útil estructurada de texto + metadatos lista para ser vectorizada e indexada
/// por `bob_betta/vector_db` en la Fase 1 sin acoplar dependencias circulares en Fase 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorPayload {
    /// UUID canónico del prospecto en Aurora.
    pub prospecto_id: Uuid,
    /// Proyección determinista a `u64` compatible con `vector_db::common::types::NodeId`.
    pub nodo_id_sugerido: u64,
    /// Texto semántico limpio preparado para el embedder en CPU.
    pub texto_embedding: String,
    /// Metadatos estructurados ordenados (incluye trazabilidad LFPDPPP).
    pub metadatos: BTreeMap<String, String>,
    /// Gatillos vitales proyectados con marcas temporales Unix.
    pub gatillos_proyectados: Vec<GatilloProyectado>,
    /// Marca temporal de captura en segundos Unix.
    pub timestamp_captura_unix: u64,
    /// Peso inicial entero para el motor de decaimiento 4D (`[1, 100]`).
    pub peso_inicial_4d: u32,
    /// Urgencia máxima detectada en los gatillos (`[0.0, 1.0]`).
    pub urgencia_maxima: f32,
    /// Identificador sugerido de isla en `DynamicIslandManager`.
    pub isla_sugerida: String,
}

/// Contrato de proyección desde entidades de dominio hacia el puente vectorial de `bob_betta`.
pub trait IntoVectorPayload: Sized {
    /// Valida las reglas de dominio/LFPDPPP y proyecta la entidad por referencia en `VectorPayload`.
    fn to_vector_payload(&self) -> AuroraResult<VectorPayload>;

    /// Valida las reglas de dominio/LFPDPPP y consume la entidad produciendo `VectorPayload`.
    fn into_vector_payload(self) -> AuroraResult<VectorPayload> {
        self.to_vector_payload()
    }
}

/// Convierte un `Uuid` de 128 bits en un identificador `u64` determinista mediante mezcla FNV-1a.
///
/// Sirve como adaptador provisional hacia `bob_betta/vector_db` (`type NodeId = u64`) mientras
/// se mantiene el `Uuid` completo en los metadatos para resolución inversa sin pérdida.
#[must_use]
pub fn uuid_a_node_id_u64(id: Uuid) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn datetime_a_unix_u64(segundos: i64, contexto: &str) -> AuroraResult<u64> {
    u64::try_from(segundos).map_err(|_| {
        AuroraError::PuenteMemoria(format!(
            "marca temporal anterior a época Unix en {contexto}: {segundos}"
        ))
    })
}

fn escalar_peso_4d(urgencia_maxima: f32) -> u32 {
    let clamped = urgencia_maxima.clamp(0.0, 1.0);
    let escalado = (clamped * (PESO_4D_MAX - PESO_4D_MIN) as f32).round() as u32 + PESO_4D_MIN;
    escalado.clamp(PESO_4D_MIN, PESO_4D_MAX)
}

fn construir_texto_embedding(prospecto: &Prospecto) -> String {
    let mut out = String::with_capacity(256);
    let _ = write!(
        out,
        "Prospecto: {}. Ubicacion: {}, {}",
        prospecto.nombre_normalizado.trim(),
        prospecto.ubicacion.ciudad.trim(),
        prospecto.ubicacion.estado.trim()
    );

    if let Some(zona) = prospecto
        .ubicacion
        .zona
        .as_deref()
        .map(str::trim)
        .filter(|z| !z.is_empty())
    {
        let _ = write!(out, " ({zona})");
    }

    let _ = write!(
        out,
        ". Perfil psicologico: {}. Ingreso anual estimado MXN: {}.",
        prospecto.perfil_psicologico.como_str(),
        prospecto.capacidad_adquisitiva.ingreso_anual_estimado_mxn
    );

    if !prospecto.aliases.is_empty() {
        let aliases_limpios: Vec<&str> = prospecto
            .aliases
            .iter()
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .collect();
        if !aliases_limpios.is_empty() {
            let _ = write!(out, " Aliases: {}.", aliases_limpios.join(", "));
        }
    }

    if !prospecto.gatillos.is_empty() {
        out.push_str(" Gatillos vitales:");
        for g in &prospecto.gatillos {
            let _ = write!(
                out,
                " [{} | urgencia={:.2} | {}] {};",
                g.tipo.como_str(),
                g.peso_urgencia,
                g.ocurrido_en.to_rfc3339(),
                g.descripcion.trim()
            );
        }
    }

    out
}

fn proyectar_gatillo(gatillo: &GatilloVital) -> AuroraResult<GatilloProyectado> {
    let ocurrido_en_unix = datetime_a_unix_u64(
        gatillo.ocurrido_en.timestamp(),
        gatillo.tipo.como_str(),
    )?;
    Ok(GatilloProyectado {
        tipo: gatillo.tipo,
        descripcion: gatillo.descripcion.trim().to_string(),
        ocurrido_en_unix,
        peso_urgencia: gatillo.peso_urgencia,
    })
}

impl IntoVectorPayload for Prospecto {
    fn to_vector_payload(&self) -> AuroraResult<VectorPayload> {
        let _span = info_span!("into_vector_payload", prospecto_id = %self.id).entered();

        // Garantiza cumplimiento LFPDPPP y rangos antes de emitir cualquier payload.
        validar_prospecto(self)?;

        let texto_embedding = construir_texto_embedding(self);
        if texto_embedding.trim().is_empty() {
            return Err(AuroraError::PuenteMemoria(format!(
                "el texto de embedding generado para {} quedó vacío",
                self.id
            )));
        }

        let timestamp_captura_unix =
            datetime_a_unix_u64(self.lfpdppp.capturado_en.timestamp(), "capturado_en")?;

        let mut gatillos_proyectados = Vec::with_capacity(self.gatillos.len());
        let mut urgencia_maxima: f32 = 0.0;
        for gatillo in &self.gatillos {
            if gatillo.peso_urgencia > urgencia_maxima {
                urgencia_maxima = gatillo.peso_urgencia;
            }
            gatillos_proyectados.push(proyectar_gatillo(gatillo)?);
        }

        let peso_inicial_4d = escalar_peso_4d(urgencia_maxima);

        let mut metadatos = BTreeMap::new();
        metadatos.insert("prospecto_id".to_string(), self.id.to_string());
        metadatos.insert(
            "nombre_normalizado".to_string(),
            self.nombre_normalizado.trim().to_string(),
        );
        metadatos.insert(
            "perfil_psicologico".to_string(),
            self.perfil_psicologico.como_str().to_string(),
        );
        metadatos.insert(
            "ubicacion_ciudad".to_string(),
            self.ubicacion.ciudad.trim().to_string(),
        );
        metadatos.insert(
            "ubicacion_estado".to_string(),
            self.ubicacion.estado.trim().to_string(),
        );
        metadatos.insert(
            "ingreso_anual_mxn".to_string(),
            self.capacidad_adquisitiva
                .ingreso_anual_estimado_mxn
                .to_string(),
        );
        if let Some(patrimonio) = self.capacidad_adquisitiva.patrimonio_estimado_mxn {
            metadatos.insert("patrimonio_estimado_mxn".to_string(), patrimonio.to_string());
        }
        metadatos.insert(
            "lfpdppp_fuente_origen".to_string(),
            self.lfpdppp.fuente_origen.trim().to_string(),
        );
        metadatos.insert(
            "lfpdppp_url_evidencia".to_string(),
            self.lfpdppp.url_evidencia.trim().to_string(),
        );
        metadatos.insert(
            "lfpdppp_capturado_en".to_string(),
            self.lfpdppp.capturado_en.to_rfc3339(),
        );
        metadatos.insert(
            "lfpdppp_opt_out".to_string(),
            self.lfpdppp.opt_out.to_string(),
        );
        metadatos.insert(
            "gatillos_count".to_string(),
            gatillos_proyectados.len().to_string(),
        );

        let payload = VectorPayload {
            prospecto_id: self.id,
            nodo_id_sugerido: uuid_a_node_id_u64(self.id),
            texto_embedding,
            metadatos,
            gatillos_proyectados,
            timestamp_captura_unix,
            peso_inicial_4d,
            urgencia_maxima,
            isla_sugerida: ISLA_PROSPECTOS_DEFECTO.to_string(),
        };

        debug!(
            prospecto_id = %self.id,
            nodo_id_sugerido = payload.nodo_id_sugerido,
            peso_inicial_4d = payload.peso_inicial_4d,
            "VectorPayload generado para puente bob_betta/vector_db"
        );

        Ok(payload)
    }
}
