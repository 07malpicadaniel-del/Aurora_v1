use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AuroraError, AuroraResult};

/// Subverticales soportadas dentro del sector Inmobiliario en México.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubverticalInmobiliaria {
    /// Proyectos en etapa de preventa con plusvalía proyectada.
    Preventa,
    /// Vivienda residencial terminada o de lujo.
    Residencial,
    /// Naves industriales, parques logísticos y locales comerciales.
    ComercialIndustrial,
}

/// Subverticales soportadas dentro del sector Asegurador en México.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubverticalSeguros {
    /// Seguro de vida individual o mancomunado.
    Vida,
    /// Gastos Médicos Mayores.
    Gmm,
    /// Seguro de Hombre Clave para socios y directivos.
    HombreClave,
    /// Fideicomiso o seguro educativo.
    Educativo,
    /// Estrategia de protección y retiro patrimonial.
    Patrimonial,
}

/// Vertical comercial objetivo para campañas y generación de activos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerticalComercial {
    /// Sector inmobiliario mexicano.
    Inmobiliaria(SubverticalInmobiliaria),
    /// Sector asegurador mexicano.
    Seguros(SubverticalSeguros),
}

impl VerticalComercial {
    /// Devuelve una etiqueta legible y estable de la vertical y subvertical.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::Inmobiliaria(SubverticalInmobiliaria::Preventa) => "inmobiliaria/preventa",
            Self::Inmobiliaria(SubverticalInmobiliaria::Residencial) => "inmobiliaria/residencial",
            Self::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial) => {
                "inmobiliaria/comercial_industrial"
            }
            Self::Seguros(SubverticalSeguros::Vida) => "seguros/vida",
            Self::Seguros(SubverticalSeguros::Gmm) => "seguros/gmm",
            Self::Seguros(SubverticalSeguros::HombreClave) => "seguros/hombre_clave",
            Self::Seguros(SubverticalSeguros::Educativo) => "seguros/educativo",
            Self::Seguros(SubverticalSeguros::Patrimonial) => "seguros/patrimonial",
        }
    }
}

/// Perfil psicológico y estilo comunicativo inferido para personalizar el ángulo comercial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PerfilPsicologico {
    /// Prioriza ROI, métricas financieras, deducibilidad fiscal y tablas comparativas.
    AnaliticoFinanciero,
    /// Responde mejor a narrativa visual, estatus, renders, video corto y urgencia táctica.
    DinamicoVisual,
    /// Prioriza certeza jurídica, legado familiar, preservación de capital y bajo riesgo.
    ConservadorPatrimonial,
}

impl PerfilPsicologico {
    /// Devuelve la representación textual estable del perfil.
    #[must_use]
    pub const fn como_str(&self) -> &'static str {
        match self {
            Self::AnaliticoFinanciero => "AnaliticoFinanciero",
            Self::DinamicoVisual => "DinamicoVisual",
            Self::ConservadorPatrimonial => "ConservadorPatrimonial",
        }
    }
}

/// Clasificación de eventos de vida o negocio detectados en fuentes públicas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoGatilloVital {
    /// Constitución de nueva sociedad mercantil o razón social.
    AperturaEmpresa,
    /// Ascenso directivo, nombramiento o cambio de puesto relevante.
    CambioPuesto,
    /// Matrimonio o cambio de estado civil reportado públicamente.
    Matrimonio,
    /// Nacimiento de hijos o crecimiento del núcleo familiar.
    Nacimiento,
    /// Apertura de sucursales, bodegas o expansión logística/operativa.
    ExpansionLogistica,
    /// Adquisición pública de inmueble, flotilla o activo productivo.
    CompraActivo,
}

impl TipoGatilloVital {
    /// Devuelve la etiqueta estable del tipo de gatillo.
    #[must_use]
    pub const fn como_str(&self) -> &'static str {
        match self {
            Self::AperturaEmpresa => "AperturaEmpresa",
            Self::CambioPuesto => "CambioPuesto",
            Self::Matrimonio => "Matrimonio",
            Self::Nacimiento => "Nacimiento",
            Self::ExpansionLogistica => "ExpansionLogistica",
            Self::CompraActivo => "CompraActivo",
        }
    }
}

/// Evento temporal que detona relevancia comercial y alimenta el decaimiento 4D en `vector_db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatilloVital {
    /// Categoría del evento detectado.
    pub tipo: TipoGatilloVital,
    /// Descripción normalizada del suceso observado.
    pub descripcion: String,
    /// Marca temporal UTC en la que ocurrió o se publicó el evento.
    pub ocurrido_en: DateTime<Utc>,
    /// Peso base de urgencia en el rango `[0.0, 1.0]`.
    pub peso_urgencia: f32,
}

/// Metadatos obligatorios de trazabilidad y cumplimiento bajo LFPDPPP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadatosLfpdppp {
    /// Nombre o identificador de la fuente pública de origen (ej. DENUE, RPC, LinkedIn público).
    pub fuente_origen: String,
    /// URL pública verificable donde consta la evidencia capturada.
    pub url_evidencia: String,
    /// Marca temporal UTC de la captura original.
    pub capturado_en: DateTime<Utc>,
    /// Bandera de exclusión ARCO/LFPDPPP; si es `true`, el nodo debe ser bloqueado.
    pub opt_out: bool,
}

/// Ubicación geográfica normalizada dentro del territorio mexicano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ubicacion {
    /// Ciudad o municipio principal.
    pub ciudad: String,
    /// Entidad federativa.
    pub estado: String,
    /// Colonia, corredor industrial o zona de influencia opcional.
    pub zona: Option<String>,
}

/// Estimación económica normalizada del prospecto en pesos mexicanos (MXN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacidadAdquisitiva {
    /// Ingreso anual estimado en MXN.
    pub ingreso_anual_estimado_mxn: u64,
    /// Patrimonio neto estimado opcional en MXN.
    pub patrimonio_estimado_mxn: Option<u64>,
}

/// Nodo canónico de prospecto dentro del ecosistema Aurora.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prospecto {
    /// Identificador único universal del prospecto.
    pub id: Uuid,
    /// Nombre completo normalizado (sin ruido tipográfico).
    pub nombre_normalizado: String,
    /// Aliases, nombres comerciales o variantes conocidas.
    pub aliases: Vec<String>,
    /// Ubicación geográfica principal.
    pub ubicacion: Ubicacion,
    /// Capacidad adquisitiva estimada.
    pub capacidad_adquisitiva: CapacidadAdquisitiva,
    /// Perfil psicológico/comunicativo para adaptar propuestas.
    pub perfil_psicologico: PerfilPsicologico,
    /// Trazabilidad legal obligatoria LFPDPPP.
    pub lfpdppp: MetadatosLfpdppp,
    /// Lista de gatillos vitales asociados al prospecto.
    pub gatillos: Vec<GatilloVital>,
}

impl Prospecto {
    /// Serializa el prospecto a JSON compacto.
    pub fn to_json(&self) -> AuroraResult<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Deserializa un prospecto desde JSON.
    pub fn from_json(raw: &str) -> AuroraResult<Self> {
        Ok(serde_json::from_str(raw)?)
    }

    /// Serializa el prospecto a representación binaria compacta con `bincode`.
    pub fn to_bincode(&self) -> AuroraResult<Vec<u8>> {
        bincode::serialize(self).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))
    }

    /// Deserializa un prospecto desde bytes generados por `bincode`.
    pub fn from_bincode(bytes: &[u8]) -> AuroraResult<Self> {
        bincode::deserialize(bytes).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))
    }
}

/// Tipo de relación interpersonal o corporativa dentro del Círculo Cercano.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoVinculo {
    /// Co-fundadores, accionistas, directivos o socios de negocio.
    SocioComercial,
    /// Vínculo familiar directo o político.
    Familiar,
    /// Pertenencia común a club deportivo, cámara empresarial o asociación.
    MismoClubCirculo,
    /// Interacción recurrente verificable en fuentes públicas.
    InteraccionFrecuente,
}

/// Arista dirigida o ponderada entre dos prospectos dentro de la telaraña topológica.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelacionTopologica {
    /// Nodo origen de la relación.
    pub origen_id: Uuid,
    /// Nodo destino de la relación.
    pub destino_id: Uuid,
    /// Naturaleza del vínculo.
    pub tipo_vinculo: TipoVinculo,
    /// Fuerza de la arista en el rango `[0.0, 1.0]`.
    pub fuerza: f32,
    /// Contexto compartido que respalda la conexión (ej. consejo de administración, torneo de golf).
    pub contexto_compartido: String,
}

/// Campaña comercial estructurada por vertical y territorio objetivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampanaVertical {
    /// Identificador único de la campaña.
    pub id: Uuid,
    /// Nombre descriptivo de la campaña.
    pub nombre: String,
    /// Vertical y subvertical comercial asignada.
    pub vertical: VerticalComercial,
    /// Segmento o zona geográfica objetivo (ej. "Puerto de Veracruz / Corredor Bajío").
    pub objetivo_geografico: String,
    /// Fecha de creación UTC.
    pub creada_en: DateTime<Utc>,
    /// Indica si la campaña está activa para procesamiento.
    pub activa: bool,
}

/// Modalidad de entregable generado en Fase 4 para un prospecto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoActivoGenerado {
    /// Dossier ejecutivo en PDF maquetado desde HTML/CSS local.
    DossierPdf,
    /// Nota de voz personalizada sintetizada mediante TTS local en CPU.
    NotaVozTts,
    /// Video vertical corto MP4 renderizado con apoyo de `ffmpeg`/NVENC.
    ReelVerticalMp4,
    /// Mensaje de texto directo adaptado al canal de salida.
    MensajeDirecto,
}

/// Ciclo de aprobación obligatorio supervisado por el Gatekeeper humano/táctico.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstadoAprobacion {
    /// Generado y en espera de revisión en el Gatekeeper.
    PendienteGatekeeper,
    /// Validado para despacho.
    Aprobado,
    /// Entregado al canal correspondiente.
    Despachado,
    /// Descartado por calidad, tono o cumplimiento.
    Rechazado,
}

/// Entregable comercial vinculado a una campaña y a un prospecto específico.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivoGenerado {
    /// Identificador único del activo.
    pub id: Uuid,
    /// Campaña a la que pertenece el activo.
    pub campana_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Tipo de activo multimodal.
    pub tipo: TipoActivoGenerado,
    /// Estado actual en el flujo de Gatekeeper.
    pub estado: EstadoAprobacion,
    /// Ruta local al artefacto generado o cuerpo textual del mensaje directo.
    pub contenido_o_ruta: String,
    /// Fecha de creación UTC.
    pub creado_en: DateTime<Utc>,
    /// Última actualización de estado UTC.
    pub actualizado_en: DateTime<Utc>,
    /// Motivo obligatorio cuando el estado es `EstadoAprobacion::Rechazado`.
    pub motivo_rechazo: Option<String>,
}

/// Nivel discreto de probabilidad de compra calculado por BOB o heurística 4D.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NivelProbabilidadCompra {
    /// Probabilidad `>= 0.72`: alta intención y ajuste topológico inmediato.
    Alta,
    /// Probabilidad en `[0.45, 0.72)`: oportunidad viable en maduración.
    Media,
    /// Probabilidad `< 0.45`: baja urgencia o baja afinidad actual.
    Baja,
}

impl NivelProbabilidadCompra {
    /// Clasifica una probabilidad en `[0.0, 1.0]` bajo los umbrales canónicos de Aurora v2.
    #[must_use]
    pub fn desde_probabilidad(probabilidad: f32) -> Self {
        if !probabilidad.is_finite() {
            return Self::Baja;
        }
        if probabilidad >= 0.72 {
            Self::Alta
        } else if probabilidad >= 0.45 {
            Self::Media
        } else {
            Self::Baja
        }
    }

    /// Etiqueta estable en español para filtros HTTP, UI y observabilidad.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::Alta => "Alta",
            Self::Media => "Media",
            Self::Baja => "Baja",
        }
    }

    /// Interpreta el nivel desde una cadena flexible (`"alta"`, `"media"`, `"baja"`).
    #[must_use]
    pub fn desde_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "alta" => Some(Self::Alta),
            "media" => Some(Self::Media),
            "baja" => Some(Self::Baja),
            _ => None,
        }
    }
}

/// Calificación comercial persistente emitida por BOB sobre un prospecto de la Telaraña 4D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogacionCompraBob {
    /// Identificador canónico del prospecto evaluado.
    pub prospecto_id: Uuid,
    /// Probabilidad de compra estimada en el rango `[0.0, 1.0]`.
    pub probabilidad_compra: f32,
    /// Clasificación discreta derivada de `probabilidad_compra`.
    pub nivel: NivelProbabilidadCompra,
    /// Producto o solución concreta sugerida para el primer contacto.
    pub producto_recomendado: String,
    /// Vertical y subvertical comercial óptima.
    pub vertical_optima: VerticalComercial,
    /// Explicación breve (1–2 líneas en español) para decisión rápida del operador.
    pub justificacion_corta: String,
    /// Peso topológico 4D combinado (gatillos vigentes + propagación de círculo cercano).
    pub score_topologico_4d: f32,
    /// Motor que produjo la catalogación (`"BOB GBNF"` o `"Heurística 4D"`).
    pub evaluado_por: String,
    /// Fecha UTC en que se emitió la calificación.
    pub evaluado_en: DateTime<Utc>,
}

/// Objetivo estratégico dual del motor de producción audiovisual vertical (`1080x1920`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ObjetivoContenidoVideo {
    /// Optimizado para retención, alcance orgánico y elegibilidad de monetización
    /// (duración objetivo >= 61 s para Creator Rewards en TikTok + CTA inbound sin PII).
    #[default]
    MonetizacionAlcance,
    /// Optimizado para conversión comercial directa de un prospecto o clúster (30–55 s).
    VentaDirectaProspecto,
}

impl ObjetivoContenidoVideo {
    /// Duración mínima recomendada en segundos según el objetivo del video.
    #[must_use]
    pub const fn duracion_objetivo_min_seg(&self) -> u32 {
        match self {
            Self::MonetizacionAlcance => 61,
            Self::VentaDirectaProspecto => 30,
        }
    }

    /// Duración máxima recomendada en segundos según el objetivo del video.
    #[must_use]
    pub const fn duracion_objetivo_max_seg(&self) -> u32 {
        match self {
            Self::MonetizacionAlcance => 90,
            Self::VentaDirectaProspecto => 55,
        }
    }

    /// Indica si este objetivo exige contenido público 100% libre de PII (Carril Verde).
    #[must_use]
    pub const fn exige_cero_pii(&self) -> bool {
        matches!(self, Self::MonetizacionAlcance)
    }

    /// Etiqueta legible para logs y UI.
    #[must_use]
    pub const fn etiqueta(&self) -> &'static str {
        match self {
            Self::MonetizacionAlcance => "monetizacion_alcance",
            Self::VentaDirectaProspecto => "venta_directa_prospecto",
        }
    }
}
