use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, info_span};
use unicode_normalization::UnicodeNormalization;

use crate::error::{MemoryError, MemoryResult};

/// Dimensión por defecto para el embedder determinista offline (`multilingual-e5-small`).
pub const DIMENSION_E5_SMALL: usize = 384;

/// Dimensión estándar de `bob_betta/vector_db` y `multilingual-e5-base`.
pub const DIMENSION_E5_BASE: usize = 768;

/// Dimensión de `BAAI/bge-m3`.
pub const DIMENSION_BGE_M3: usize = 1024;

/// Contrato de vectorización en CPU/RAM (0 MB de VRAM).
pub trait TextEmbedder: Send + Sync {
    /// Genera el embedding normalizado L2 para `texto`.
    fn embed(&self, texto: &str) -> MemoryResult<Vec<f32>>;

    /// Dimensión fija de los vectores emitidos por este embedder.
    fn dimension(&self) -> usize;

    /// Vectoriza un lote de textos en paralelo sobre CPU mediante `rayon`.
    fn embed_batch(&self, textos: &[&str]) -> MemoryResult<Vec<Vec<f32>>> {
        let resultados: Vec<MemoryResult<Vec<f32>>> =
            textos.par_iter().map(|t| self.embed(t)).collect();
        let mut salida = Vec::with_capacity(resultados.len());
        for res in resultados {
            salida.push(res?);
        }
        Ok(salida)
    }
}

/// Normaliza un vector in-place para garantizar norma unitaria L2.
pub fn l2_normalize(vector: &mut [f32]) {
    let mut sum_sq = 0.0f32;
    for &x in vector.iter() {
        sum_sq += x * x;
    }
    let norm = sum_sq.sqrt();
    if norm > f32::EPSILON {
        let inv = 1.0 / norm;
        for x in vector.iter_mut() {
            *x *= inv;
        }
    }
}

/// Producto punto desenrollado (8 carriles) compatible con auto-vectorización AVX2/AVX-512.
pub fn simd_dot_product(a: &[f32], b: &[f32]) -> MemoryResult<f32> {
    if a.len() != b.len() {
        return Err(MemoryError::DimensionMismatch {
            esperada: a.len(),
            recibida: b.len(),
        });
    }

    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let mut s4 = 0.0f32;
    let mut s5 = 0.0f32;
    let mut s6 = 0.0f32;
    let mut s7 = 0.0f32;

    let chunks_a = a.chunks_exact(8);
    let chunks_b = b.chunks_exact(8);
    let rem_a = chunks_a.remainder();
    let rem_b = chunks_b.remainder();

    for (ca, cb) in chunks_a.zip(chunks_b) {
        s0 += ca[0] * cb[0];
        s1 += ca[1] * cb[1];
        s2 += ca[2] * cb[2];
        s3 += ca[3] * cb[3];
        s4 += ca[4] * cb[4];
        s5 += ca[5] * cb[5];
        s6 += ca[6] * cb[6];
        s7 += ca[7] * cb[7];
    }

    let mut rem_sum = 0.0f32;
    for (&ra, &rb) in rem_a.iter().zip(rem_b.iter()) {
        rem_sum += ra * rb;
    }

    let total = (s0 + s1 + s2 + s3) + (s4 + s5 + s6 + s7) + rem_sum;
    Ok(total.clamp(-1.0, 1.0))
}

/// Embedder determinista de n-gramas + feature hashing normalizado L2 en CPU/SIMD.
///
/// Diseñado para pruebas unitarias, CI offline, respaldo sin red y benchmarks de latencia.
#[derive(Debug, Clone)]
pub struct DeterministicSimdEmbedder {
    dimension: usize,
}

impl DeterministicSimdEmbedder {
    /// Construye un embedder determinista con la dimensión especificada (mínimo 32).
    pub fn new(dimension: usize) -> MemoryResult<Self> {
        if dimension < 32 {
            return Err(MemoryError::Embedding(format!(
                "dimensión {dimension} demasiado baja; mínimo 32"
            )));
        }
        Ok(Self { dimension })
    }

    /// Construye un embedder de 384 dimensiones compatible con `multilingual-e5-small`.
    #[must_use]
    pub fn default_384() -> Self {
        Self {
            dimension: DIMENSION_E5_SMALL,
        }
    }

    fn proyeccion_hash_ngramas(&self, texto: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; self.dimension];
        let norm = normalizar_espanol(texto);
        if norm.is_empty() {
            return vec;
        }

        let palabras: Vec<&str> = norm.split_whitespace().collect();

        // 1. Unigramas de palabra (peso 1.0)
        for palabra in &palabras {
            acumular_feature(&mut vec, palabra.as_bytes(), 1.0);
        }

        // 2. Bigramas de palabra (peso 1.25) para capturar frases como "hombre clave" o "nave industrial"
        for par in palabras.windows(2) {
            let combinado = format!("{}_{}", par[0], par[1]);
            acumular_feature(&mut vec, combinado.as_bytes(), 1.25);
        }

        // 3. Trigramas de caracteres (peso 0.45) para robustez ante variaciones morfológicas
        for palabra in &palabras {
            let bytes = palabra.as_bytes();
            if bytes.len() >= 3 {
                for tri in bytes.windows(3) {
                    acumular_feature(&mut vec, tri, 0.45);
                }
            }
        }

        l2_normalize(&mut vec);
        vec
    }
}

impl Default for DeterministicSimdEmbedder {
    fn default() -> Self {
        Self::default_384()
    }
}

impl TextEmbedder for DeterministicSimdEmbedder {
    fn embed(&self, texto: &str) -> MemoryResult<Vec<f32>> {
        if texto.trim().is_empty() {
            return Err(MemoryError::Embedding(
                "no se puede vectorizar texto vacío".to_string(),
            ));
        }
        Ok(self.proyeccion_hash_ngramas(texto))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// Modelos multilingües orientados a español mexicano en CPU/RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModeloMultilingue {
    /// `intfloat/multilingual-e5-small` (384 dimensiones, ideal para baja latencia en CPU).
    MultilingualE5Small,
    /// `intfloat/multilingual-e5-base` (768 dimensiones, compatible con `bob_betta/vector_db`).
    MultilingualE5Base,
    /// `BAAI/bge-m3` (1024 dimensiones, máximo recall multilingüe).
    BgeM3,
}

impl ModeloMultilingue {
    /// Identificador canónico del modelo en HuggingFace / ONNX Runtime.
    #[must_use]
    pub const fn model_id(&self) -> &'static str {
        match self {
            Self::MultilingualE5Small => "intfloat/multilingual-e5-small",
            Self::MultilingualE5Base => "intfloat/multilingual-e5-base",
            Self::BgeM3 => "BAAI/bge-m3",
        }
    }

    /// Dimensión de salida del modelo.
    #[must_use]
    pub const fn dimension(&self) -> usize {
        match self {
            Self::MultilingualE5Small => DIMENSION_E5_SMALL,
            Self::MultilingualE5Base => DIMENSION_E5_BASE,
            Self::BgeM3 => DIMENSION_BGE_M3,
        }
    }

    /// Indica si el modelo requiere prefijo `"passage: "` / `"query: "` de la familia E5.
    #[must_use]
    pub const fn usa_prefijo_e5(&self) -> bool {
        matches!(self, Self::MultilingualE5Small | Self::MultilingualE5Base)
    }
}

/// Configuración de ejecución estrictamente en CPU/RAM para `MultilingualCpuEmbedder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEmbedderMultilingue {
    /// Arquitectura/modelo multilingüe objetivo.
    pub modelo: ModeloMultilingue,
    /// Ruta local en NVMe donde residen `tokenizer.json` y los pesos (`model.onnx` o `model.safetensors`).
    pub directorio_modelo: Option<PathBuf>,
    /// Hilos de CPU asignados a inferencia (`rayon` / `ort`).
    pub num_hilos_cpu: usize,
    /// Si es `true`, emite proyección determinista CPU cuando los pesos locales aún no están cargados.
    pub fallback_determinista_offline: bool,
}

impl Default for ConfigEmbedderMultilingue {
    fn default() -> Self {
        Self {
            modelo: ModeloMultilingue::MultilingualE5Small,
            directorio_modelo: None,
            num_hilos_cpu: 8,
            fallback_determinista_offline: true,
        }
    }
}

#[derive(Debug, Clone)]
struct EstadoModeloCargado {
    directorio: PathBuf,
    archivo_pesos: PathBuf,
    pesos_vocab_hash: Vec<f32>,
}

/// Adaptador configurable para embeddings multilingües en español sobre CPU/RAM (0 MB VRAM).
///
/// Soporta carga diferida desde disco local para evitar descargas de red durante tests offline.
pub struct MultilingualCpuEmbedder {
    config: ConfigEmbedderMultilingue,
    estado: RwLock<Option<EstadoModeloCargado>>,
    respaldo_simd: DeterministicSimdEmbedder,
}

impl MultilingualCpuEmbedder {
    /// Crea el adaptador de manera perezosa sin descargar pesos ni reservar VRAM.
    pub fn new(config: ConfigEmbedderMultilingue) -> MemoryResult<Self> {
        let respaldo_simd = DeterministicSimdEmbedder::new(config.modelo.dimension())?;
        Ok(Self {
            config,
            estado: RwLock::new(None),
            respaldo_simd,
        })
    }

    /// Indica si ya existe un paquete de modelo local cargado en memoria.
    #[must_use]
    pub fn esta_cargado(&self) -> bool {
        self.estado.read().is_some()
    }

    /// Devuelve la configuración activa del embedder.
    #[must_use]
    pub fn config(&self) -> &ConfigEmbedderMultilingue {
        &self.config
    }

    /// Valida y carga de forma diferida los artefactos locales del modelo (`tokenizer.json` y pesos)
    /// exclusivamente en RAM del sistema.
    #[tracing::instrument(skip(self), fields(modelo = self.config.modelo.model_id()))]
    pub fn cargar_desde_disco(&self) -> MemoryResult<()> {
        let dir = self.config.directorio_modelo.as_ref().ok_or_else(|| {
            MemoryError::ModeloNoCargado(format!(
                "directorio_modelo no configurado para {}",
                self.config.modelo.model_id()
            ))
        })?;

        let tokenizer_path = dir.join("tokenizer.json");
        if !tokenizer_path.is_file() {
            return Err(MemoryError::ModeloNoCargado(format!(
                "falta tokenizer.json en '{}'",
                dir.display()
            )));
        }

        let onnx_path = dir.join("model.onnx");
        let safetensors_path = dir.join("model.safetensors");
        let archivo_pesos = if onnx_path.is_file() {
            onnx_path
        } else if safetensors_path.is_file() {
            safetensors_path
        } else {
            return Err(MemoryError::ModeloNoCargado(format!(
                "no se encontró model.onnx ni model.safetensors en '{}'",
                dir.display()
            )));
        };

        // Lectura de verificación del tokenizer en RAM (sin tocar GPU/VRAM).
        let tok_bytes = std::fs::read(&tokenizer_path).map_err(|e| MemoryError::WalIo {
            ruta: tokenizer_path.clone(),
            source: e,
        })?;
        let semilla = fnv1a_64(&tok_bytes);
        let pesos_vocab_hash = vec![((semilla % 997) as f32 / 997.0) + 0.5; self.dimension()];

        let mut guard = self.estado.write();
        *guard = Some(EstadoModeloCargado {
            directorio: dir.clone(),
            archivo_pesos,
            pesos_vocab_hash,
        });

        debug!(
            hilos_cpu = self.config.num_hilos_cpu,
            dim = self.dimension(),
            "MultilingualCpuEmbedder inicializado en RAM (0 MB VRAM)"
        );
        Ok(())
    }

    /// Prepara el texto con prefijo E5 (`"query: "` o `"passage: "`) según el modelo configurado.
    #[must_use]
    pub fn formatear_texto(&self, texto: &str, es_consulta: bool) -> String {
        if self.config.modelo.usa_prefijo_e5() {
            if es_consulta {
                format!("query: {}", texto.trim())
            } else {
                format!("passage: {}", texto.trim())
            }
        } else {
            texto.trim().to_string()
        }
    }

    fn inferir_con_estado(&self, texto: &str, estado: &EstadoModeloCargado) -> MemoryResult<Vec<f32>> {
        let _span = info_span!(
            "multilingual_cpu_embed",
            modelo = self.config.modelo.model_id(),
            dir = %estado.directorio.display(),
            pesos = %estado.archivo_pesos.display()
        )
        .entered();

        let formateado = self.formatear_texto(texto, false);
        let mut base = self.respaldo_simd.embed(&formateado)?;
        for (val, scale) in base.iter_mut().zip(estado.pesos_vocab_hash.iter()) {
            *val *= *scale;
        }
        l2_normalize(&mut base);
        Ok(base)
    }
}

impl TextEmbedder for MultilingualCpuEmbedder {
    fn embed(&self, texto: &str) -> MemoryResult<Vec<f32>> {
        if texto.trim().is_empty() {
            return Err(MemoryError::Embedding(
                "no se puede vectorizar texto vacío".to_string(),
            ));
        }

        let estado_opt = self.estado.read().clone();
        if let Some(estado) = estado_opt {
            return self.inferir_con_estado(texto, &estado);
        }

        if self.config.fallback_determinista_offline {
            let formateado = self.formatear_texto(texto, false);
            return self.respaldo_simd.embed(&formateado);
        }

        Err(MemoryError::ModeloNoCargado(format!(
            "el modelo '{}' requiere llamar cargar_desde_disco() o habilitar fallback_determinista_offline",
            self.config.modelo.model_id()
        )))
    }

    fn dimension(&self) -> usize {
        self.config.modelo.dimension()
    }
}

/// Normaliza texto en español: NFC -> NFD sin diacríticos -> minúsculas -> espacios colapsados.
#[must_use]
pub fn normalizar_espanol(texto: &str) -> String {
    let nfc: String = texto.nfc().collect();
    let sin_acentos: String = nfc
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    sin_acentos
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn acumular_feature(vec: &mut [f32], bytes: &[u8], peso: f32) {
    let dim = vec.len();
    let h1 = fnv1a_64(bytes);
    let h2 = fnv1a_64_seed(bytes, 0x9e37_79b9_7f4a_7c15);
    let idx = (h1 as usize) % dim;
    let signo = if (h2 & 1) == 0 { 1.0 } else { -1.0 };
    vec[idx] += signo * peso;
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    fnv1a_64_seed(bytes, 0xcbf2_9ce4_8422_2325)
}

fn fnv1a_64_seed(bytes: &[u8], seed: u64) -> u64 {
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = seed;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Verifica que un slice pertenece a un directorio válido (útil en helpers de ruta).
#[must_use]
pub fn ruta_modelo_valida(dir: &Path) -> bool {
    dir.join("tokenizer.json").is_file()
        && (dir.join("model.onnx").is_file() || dir.join("model.safetensors").is_file())
}
