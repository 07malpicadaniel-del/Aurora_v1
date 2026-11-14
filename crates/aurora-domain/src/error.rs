use thiserror::Error;
use uuid::Uuid;

/// Alias estándar de resultado para todo el ecosistema Aurora.
pub type AuroraResult<T> = Result<T, AuroraError>;

/// Errores canónicos del dominio Aurora, cumplimiento LFPDPPP y puente de memoria.
#[derive(Debug, Error)]
pub enum AuroraError {
    /// El prospecto ejerció su derecho de exclusión (`opt_out = true`) bajo LFPDPPP.
    #[error("cumplimiento LFPDPPP: el prospecto {prospecto_id} tiene opt_out activo")]
    OptOutActive { prospecto_id: Uuid },

    /// Falta una fuente pública verificable para justificar la captura.
    #[error("cumplimiento LFPDPPP: el prospecto {prospecto_id} carece de fuente_origen pública")]
    FuentePublicaFaltante { prospecto_id: Uuid },

    /// La URL de evidencia no cumple con formato HTTP/HTTPS válido.
    #[error("cumplimiento LFPDPPP: url_evidencia inválida para prospecto {prospecto_id}: '{url}'")]
    UrlEvidenciaInvalida { prospecto_id: Uuid, url: String },

    /// Error general de validación de reglas de dominio.
    #[error("validación de dominio fallida: {0}")]
    Validacion(String),

    /// Un peso o puntuación normalizada se encuentra fuera de `[0.0, 1.0]` o no es finito.
    #[error("peso '{campo}' ({valor}) fuera del rango permitido [0.0, 1.0]")]
    PesoFueraDeRango { campo: &'static str, valor: f32 },

    /// Se intentó crear una arista topológica de un nodo hacia sí mismo.
    #[error("relación topológica inválida: auto-referencia sobre el nodo {nodo_id}")]
    AutoRelacion { nodo_id: Uuid },

    /// Fallo al serializar o deserializar en formato JSON.
    #[error("error de serialización JSON: {0}")]
    SerializacionJson(#[from] serde_json::Error),

    /// Fallo al serializar o deserializar en formato binario.
    #[error("error de serialización binaria: {0}")]
    SerializacionBinaria(String),

    /// Fallo al proyectar entidades de dominio hacia el puente vectorial de `bob_betta`.
    #[error("error en puente de memoria vectorial: {0}")]
    PuenteMemoria(String),
}
