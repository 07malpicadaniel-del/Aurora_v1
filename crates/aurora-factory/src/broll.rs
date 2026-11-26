use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use aurora_domain::{
    PerfilPsicologico, SubverticalInmobiliaria, SubverticalSeguros, VerticalComercial,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{FactoryError, FactoryResult};
use crate::manifest::sha256_bytes;

/// Cantidad mínima recomendada de clips maestros B-Roll por vertical para evitar repetición visual.
pub const MIN_CLIPS_COBERTURA_VERTICAL: usize = 3;

/// Catálogo completo de las 8 verticales comerciales soportadas por Aurora v2 en México.
pub const TODAS_LAS_VERTICALES: [VerticalComercial; 8] = [
    VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa),
    VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial),
    VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
    VerticalComercial::Seguros(SubverticalSeguros::Vida),
    VerticalComercial::Seguros(SubverticalSeguros::Gmm),
    VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
    VerticalComercial::Seguros(SubverticalSeguros::Educativo),
    VerticalComercial::Seguros(SubverticalSeguros::Patrimonial),
];

/// Clip maestro B-Roll (sin texto ni locución) almacenado en NVMe y listo para composición NVENC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipBroll {
    /// Identificador único del clip maestro.
    pub id: Uuid,
    /// Ruta absoluta o relativa en almacenamiento NVMe dentro de `<broll_dir>/<vertical>/`.
    pub ruta_nvme: PathBuf,
    /// Vertical comercial a la que pertenece el clip.
    pub vertical: VerticalComercial,
    /// Perfil psicológico preferente (`None` indica clip versátil para cualquier perfil de la vertical).
    pub perfil_objetivo: Option<PerfilPsicologico>,
    /// Etiquetas semánticas y de ambientación visual.
    #[serde(default)]
    pub etiquetas: Vec<String>,
    /// Duración del clip en milisegundos.
    pub duracion_ms: u64,
    /// Ancho del cuadro en píxeles.
    pub ancho: u32,
    /// Alto del cuadro en píxeles.
    pub alto: u32,
    /// Movimiento de cámara cinematográfico (ej. `dolly_in_slow`, `fpv_architectural_glide`).
    pub movimiento_camara: String,
    /// Huella SHA-256 en hexadecimal del archivo `.mp4`/`.mov` verificado.
    pub sha256: String,
    /// Veces que este clip ha sido seleccionado para ensamblar Reels (base de rotación justa).
    #[serde(default)]
    pub usos_acumulados: u64,
    /// Fecha UTC de registro en la biblioteca NVMe.
    pub registrado_en: DateTime<Utc>,
}

/// Índice serializable en NVMe (`<broll_dir>/index.json`) de toda la biblioteca B-Roll.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndiceBroll {
    /// Lista de clips maestros registrados.
    #[serde(default)]
    pub clips: Vec<ClipBroll>,
    /// Última actualización atómica UTC.
    pub actualizado_en: Option<DateTime<Utc>>,
}

/// Alias semántico de catálogo B-Roll en NVMe.
pub type CatalogoBroll = IndiceBroll;

/// Parámetros para importar y validar un nuevo clip `.mp4`/`.mov` descargado de Higgsfield.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolicitudRegistroClipBroll {
    /// Ruta local del clip de origen a importar (se rechaza *path traversal* `..`).
    pub ruta_origen: PathBuf,
    /// Vertical comercial a la que corresponde el clip.
    pub vertical: VerticalComercial,
    /// Perfil psicológico objetivo opcional.
    #[serde(default)]
    pub perfil_objetivo: Option<PerfilPsicologico>,
    /// Etiquetas descriptivas de escena.
    #[serde(default)]
    pub etiquetas: Vec<String>,
    /// Duración en milisegundos (si se omite o es 0 se asignan 5000 ms por defecto).
    #[serde(default = "duracion_broll_defecto_ms")]
    pub duracion_ms: u64,
    /// Ancho en píxeles (por defecto `1080`).
    #[serde(default = "ancho_broll_defecto")]
    pub ancho: u32,
    /// Alto en píxeles (por defecto `1920`).
    #[serde(default = "alto_broll_defecto")]
    pub alto: u32,
    /// Movimiento de cámara cinematográfico (ej. `dolly_in_slow`).
    #[serde(default = "movimiento_camara_defecto")]
    pub movimiento_camara: String,
}

const fn duracion_broll_defecto_ms() -> u64 {
    5_000
}

const fn ancho_broll_defecto() -> u32 {
    1080
}

const fn alto_broll_defecto() -> u32 {
    1920
}

fn movimiento_camara_defecto() -> String {
    "dolly_in_slow".to_string()
}

/// Estado del ciclo de vida de un brief cinematográfico para generación en Higgsfield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstadoBriefHiggsfield {
    /// Prompt listo para ejecutarse en Higgsfield (web manual o integración API futura).
    PendienteGeneracion,
    /// Clip ya generado e importado a [`BibliotecaBroll`].
    Importado,
    /// Brief descartado por el operador.
    Descartado,
}

/// Prompt cinematográfico estructurado por clúster para generar clips maestros en Higgsfield.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefHiggsfield {
    /// Identificador único del brief.
    pub id: Uuid,
    /// Vertical comercial objetivo.
    pub vertical: VerticalComercial,
    /// Perfil psicológico al que apunta la estética visual.
    pub perfil_objetivo: PerfilPsicologico,
    /// Prompt cinematográfico en inglés técnico (sin texto, sin voz, sin marcas de agua).
    pub escena_visual_en: String,
    /// Movimiento de cámara sugerido para Higgsfield (`dolly_in_slow`, `fpv_architectural_glide`, etc.).
    pub movimiento_camara: String,
    /// Duración objetivo en segundos (`4`–`6`).
    pub duracion_objetivo_seg: u8,
    /// Relación de aspecto vertical (`"9:16"`).
    pub relacion_aspecto: String,
    /// Estado actual del brief en la cola.
    pub estado: EstadoBriefHiggsfield,
    /// UUID del [`ClipBroll`] vinculado cuando el estado pasa a `Importado`.
    #[serde(default)]
    pub clip_asociado_id: Option<Uuid>,
    /// Fecha UTC de creación del brief.
    pub creado_en: DateTime<Utc>,
}

/// Cola persistente en NVMe (`<broll_dir>/higgsfield_briefs.json`) de briefs pendientes e importados.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColaBriefsHiggsfield {
    /// Briefs acumulados en orden de creación.
    #[serde(default)]
    pub briefs: Vec<BriefHiggsfield>,
    /// Última actualización atómica UTC.
    pub actualizado_en: Option<DateTime<Utc>>,
}

/// Gestor persistente en NVMe de clips maestros B-Roll y cola de briefs para Higgsfield.
#[derive(Debug, Clone)]
pub struct BibliotecaBroll {
    broll_dir: PathBuf,
    indice: IndiceBroll,
    cola_briefs: ColaBriefsHiggsfield,
}

impl BibliotecaBroll {
    /// Abre o inicializa la biblioteca B-Roll en `broll_dir`, cargando `index.json` y `higgsfield_briefs.json`.
    pub fn abrir(broll_dir: impl Into<PathBuf>) -> FactoryResult<Self> {
        let broll_dir = broll_dir.into();
        fs::create_dir_all(&broll_dir).map_err(|source| FactoryError::IoArtefacto {
            ruta: broll_dir.clone(),
            source,
        })?;

        let ruta_index = broll_dir.join("index.json");
        let indice = if ruta_index.is_file() {
            let contenido =
                fs::read_to_string(&ruta_index).map_err(|source| FactoryError::IoArtefacto {
                    ruta: ruta_index.clone(),
                    source,
                })?;
            if contenido.trim().is_empty() {
                IndiceBroll::default()
            } else {
                serde_json::from_str::<IndiceBroll>(&contenido).map_err(|e| {
                    FactoryError::DatosInvalidos(format!(
                        "index.json corrupto en '{}': {e}",
                        ruta_index.display()
                    ))
                })?
            }
        } else {
            IndiceBroll::default()
        };

        let ruta_briefs = broll_dir.join("higgsfield_briefs.json");
        let cola_briefs = if ruta_briefs.is_file() {
            let contenido =
                fs::read_to_string(&ruta_briefs).map_err(|source| FactoryError::IoArtefacto {
                    ruta: ruta_briefs.clone(),
                    source,
                })?;
            if contenido.trim().is_empty() {
                ColaBriefsHiggsfield::default()
            } else {
                serde_json::from_str::<ColaBriefsHiggsfield>(&contenido).map_err(|e| {
                    FactoryError::DatosInvalidos(format!(
                        "higgsfield_briefs.json corrupto en '{}': {e}",
                        ruta_briefs.display()
                    ))
                })?
            }
        } else {
            ColaBriefsHiggsfield::default()
        };

        Ok(Self {
            broll_dir,
            indice,
            cola_briefs,
        })
    }

    /// Devuelve el directorio raíz NVMe de la biblioteca B-Roll.
    #[must_use]
    pub fn directorio_raiz(&self) -> &Path {
        &self.broll_dir
    }

    /// Devuelve el índice completo de clips maestros registrados.
    #[must_use]
    pub const fn indice(&self) -> &IndiceBroll {
        &self.indice
    }

    /// Devuelve la cola actual de briefs cinematográficos para Higgsfield.
    #[must_use]
    pub const fn cola_briefs(&self) -> &ColaBriefsHiggsfield {
        &self.cola_briefs
    }

    /// Cuenta cuántos clips existen actualmente por etiqueta de vertical.
    #[must_use]
    pub fn conteo_por_vertical(&self) -> BTreeMap<String, usize> {
        let mut mapa = BTreeMap::new();
        for vertical in TODAS_LAS_VERTICALES {
            mapa.insert(vertical.etiqueta().to_string(), 0);
        }
        for clip in &self.indice.clips {
            *mapa
                .entry(clip.vertical.etiqueta().to_string())
                .or_insert(0) += 1;
        }
        mapa
    }

    /// Valida ruta (cero *path traversal*), comprueba cabecera MP4/MOV (`ftyp`), calcula SHA-256,
    /// copia el clip dentro de `<broll_dir>/<vertical>/` y persiste `index.json` atómicamente.
    #[tracing::instrument(skip(self, solicitud), fields(vertical = solicitud.vertical.etiqueta()))]
    pub fn registrar_clip(
        &mut self,
        solicitud: SolicitudRegistroClipBroll,
    ) -> FactoryResult<ClipBroll> {
        validar_ruta_sin_traversal(&solicitud.ruta_origen)?;

        let meta = fs::metadata(&solicitud.ruta_origen).map_err(|source| {
            FactoryError::IoArtefacto {
                ruta: solicitud.ruta_origen.clone(),
                source,
            }
        })?;
        if !meta.is_file() {
            return Err(FactoryError::DatosInvalidos(format!(
                "la ruta '{}' no corresponde a un archivo regular de video",
                solicitud.ruta_origen.display()
            )));
        }

        let bytes = fs::read(&solicitud.ruta_origen).map_err(|source| FactoryError::IoArtefacto {
            ruta: solicitud.ruta_origen.clone(),
            source,
        })?;
        validar_firma_mp4_mov(&bytes)?;

        let sha256 = sha256_bytes(&bytes);
        let id = Uuid::new_v4();
        let slug_vertical = solicitud.vertical.etiqueta().replace('/', "_");
        let ext = solicitud
            .ruta_origen
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| e == "mp4" || e == "mov")
            .unwrap_or_else(|| "mp4".to_string());

        let dir_vertical = self.broll_dir.join(&slug_vertical);
        fs::create_dir_all(&dir_vertical).map_err(|source| FactoryError::IoArtefacto {
            ruta: dir_vertical.clone(),
            source,
        })?;

        let ruta_destino = dir_vertical.join(format!("{id}.{ext}"));
        escribir_bytes_atomico(&ruta_destino, &bytes)?;

        let movimiento = if solicitud.movimiento_camara.trim().is_empty() {
            movimiento_camara_defecto()
        } else {
            solicitud.movimiento_camara.trim().to_string()
        };

        let clip = ClipBroll {
            id,
            ruta_nvme: ruta_destino,
            vertical: solicitud.vertical,
            perfil_objetivo: solicitud.perfil_objetivo,
            etiquetas: solicitud
                .etiquetas
                .into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            duracion_ms: if solicitud.duracion_ms == 0 {
                duracion_broll_defecto_ms()
            } else {
                solicitud.duracion_ms
            },
            ancho: if solicitud.ancho == 0 {
                ancho_broll_defecto()
            } else {
                solicitud.ancho
            },
            alto: if solicitud.alto == 0 {
                alto_broll_defecto()
            } else {
                solicitud.alto
            },
            movimiento_camara: movimiento,
            sha256,
            usos_acumulados: 0,
            registrado_en: Utc::now(),
        };

        self.indice.clips.push(clip.clone());
        self.indice.actualizado_en = Some(Utc::now());
        self.persistir_indice()?;

        info!(
            clip_id = %clip.id,
            vertical = clip.vertical.etiqueta(),
            ruta = %clip.ruta_nvme.display(),
            "Clip B-Roll maestro registrado y sellado en NVMe"
        );
        Ok(clip)
    }

    /// Selecciona hasta `cantidad` clips de la vertical priorizando afinidad con `perfil`
    /// y menor `usos_acumulados` (rotación determinista), incrementando el contador y
    /// persistiendo `index.json`.
    pub fn seleccionar_clips_para_reel(
        &mut self,
        vertical: &VerticalComercial,
        perfil: PerfilPsicologico,
        cantidad: usize,
    ) -> Vec<ClipBroll> {
        if cantidad == 0 {
            return Vec::new();
        }

        let mut candidatos: Vec<(usize, u8, u64, DateTime<Utc>, Uuid)> = self
            .indice
            .clips
            .iter()
            .enumerate()
            .filter(|(_, clip)| clip.vertical == *vertical && clip.ruta_nvme.is_file())
            .map(|(idx, clip)| {
                let afinidad = match clip.perfil_objetivo {
                    Some(p) if p == perfil => 0_u8,
                    None => 1_u8,
                    Some(_) => 2_u8,
                };
                (
                    idx,
                    afinidad,
                    clip.usos_acumulados,
                    clip.registrado_en,
                    clip.id,
                )
            })
            .collect();

        if candidatos.is_empty() {
            return Vec::new();
        }

        candidatos.sort_by_key(|&(_, afinidad, usos, reg, id)| (afinidad, usos, reg, id));

        let mut seleccionados = Vec::with_capacity(cantidad.min(candidatos.len()));
        for (idx, _, _, _, _) in candidatos.into_iter().take(cantidad) {
            if let Some(clip) = self.indice.clips.get_mut(idx) {
                clip.usos_acumulados = clip.usos_acumulados.saturating_add(1);
                seleccionados.push(clip.clone());
            }
        }

        if !seleccionados.is_empty() {
            self.indice.actualizado_en = Some(Utc::now());
            if let Err(err) = self.persistir_indice() {
                warn!(
                    error = %err,
                    "No se pudo persistir incremento de usos_acumulados en index.json"
                );
            } else {
                debug!(
                    vertical = vertical.etiqueta(),
                    seleccionados = seleccionados.len(),
                    "Clips B-Roll seleccionados con rotación determinista"
                );
            }
        }

        seleccionados
    }

    /// Detecta qué verticales comerciales cuentan con menos de 3 clips maestros registrados.
    #[must_use]
    pub fn detectar_brechas_cobertura(&self) -> Vec<VerticalComercial> {
        TODAS_LAS_VERTICALES
            .iter()
            .copied()
            .filter(|vertical| {
                let conteo = self
                    .indice
                    .clips
                    .iter()
                    .filter(|clip| clip.vertical == *vertical && clip.ruta_nvme.is_file())
                    .count();
                conteo < MIN_CLIPS_COBERTURA_VERTICAL
            })
            .collect()
    }

    /// Genera `cantidad` prompts cinematográficos estructurados para Higgsfield y los persiste en NVMe.
    pub fn generar_briefs_higgsfield(
        &mut self,
        vertical: VerticalComercial,
        perfil_objetivo: PerfilPsicologico,
        cantidad: usize,
    ) -> FactoryResult<Vec<BriefHiggsfield>> {
        let total = cantidad.clamp(1, 10);
        let plantillas = plantillas_higgsfield(vertical, perfil_objetivo);
        let existentes = self.cola_briefs.briefs.len();
        let ahora = Utc::now();

        let mut creados = Vec::with_capacity(total);
        for i in 0..total {
            let (escena_en, camara, duracion) = &plantillas[(existentes + i) % plantillas.len()];
            let brief = BriefHiggsfield {
                id: Uuid::new_v4(),
                vertical,
                perfil_objetivo,
                escena_visual_en: escena_en.clone(),
                movimiento_camara: (*camara).to_string(),
                duracion_objetivo_seg: *duracion,
                relacion_aspecto: "9:16".to_string(),
                estado: EstadoBriefHiggsfield::PendienteGeneracion,
                clip_asociado_id: None,
                creado_en: ahora,
            };
            self.cola_briefs.briefs.push(brief.clone());
            creados.push(brief);
        }

        self.cola_briefs.actualizado_en = Some(ahora);
        self.persistir_cola_briefs()?;
        info!(
            vertical = vertical.etiqueta(),
            perfil = perfil_objetivo.como_str(),
            generados = creados.len(),
            "Briefs cinematográficos de Higgsfield encolados en NVMe"
        );
        Ok(creados)
    }

    /// Marca un brief existente como `Importado` asociándolo al [`ClipBroll`] recién registrado.
    pub fn marcar_brief_importado(
        &mut self,
        brief_id: Uuid,
        clip_id: Uuid,
    ) -> FactoryResult<BriefHiggsfield> {
        let mut encontrado = None;
        for brief in &mut self.cola_briefs.briefs {
            if brief.id == brief_id {
                brief.estado = EstadoBriefHiggsfield::Importado;
                brief.clip_asociado_id = Some(clip_id);
                encontrado = Some(brief.clone());
                break;
            }
        }
        let Some(brief_actualizado) = encontrado else {
            return Err(FactoryError::DatosInvalidos(format!(
                "brief Higgsfield {brief_id} no encontrado en cola"
            )));
        };

        self.cola_briefs.actualizado_en = Some(Utc::now());
        self.persistir_cola_briefs()?;
        Ok(brief_actualizado)
    }

    fn persistir_indice(&self) -> FactoryResult<()> {
        let ruta = self.broll_dir.join("index.json");
        persistir_json_atomico(&ruta, &self.indice)
    }

    fn persistir_cola_briefs(&self) -> FactoryResult<()> {
        let ruta = self.broll_dir.join("higgsfield_briefs.json");
        persistir_json_atomico(&ruta, &self.cola_briefs)
    }
}

/// Rechaza rutas vacías, con bytes nulos o con segmentos de *path traversal* (`..`).
pub fn validar_ruta_sin_traversal(ruta: &Path) -> FactoryResult<()> {
    let texto = ruta.to_string_lossy();
    if texto.trim().is_empty() {
        return Err(FactoryError::DatosInvalidos(
            "la ruta del clip B-Roll no puede estar vacía".to_string(),
        ));
    }
    if texto.contains('\0') {
        return Err(FactoryError::DatosInvalidos(
            "la ruta del clip B-Roll contiene byte nulo".to_string(),
        ));
    }
    for comp in ruta.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(FactoryError::DatosInvalidos(format!(
                "path traversal prohibido en ruta de clip B-Roll: '{}'",
                ruta.display()
            )));
        }
    }
    if texto.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(FactoryError::DatosInvalidos(format!(
            "path traversal prohibido en ruta de clip B-Roll: '{}'",
            ruta.display()
        )));
    }
    Ok(())
}

/// Verifica la firma ISO Base Media File Format (`ftyp`) de archivos `.mp4` o `.mov`.
pub fn validar_firma_mp4_mov(bytes: &[u8]) -> FactoryResult<()> {
    if bytes.len() < 12 {
        return Err(FactoryError::DatosInvalidos(
            "el archivo de video es demasiado pequeño para contener cabecera MP4/MOV válida"
                .to_string(),
        ));
    }

    if &bytes[4..8] != b"ftyp" {
        return Err(FactoryError::DatosInvalidos(
            "el archivo no posee firma de contenedor MP4/MOV ('ftyp') válida".to_string(),
        ));
    }

    let box_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if box_size != 0 && box_size != 1 && box_size < 12 {
        return Err(FactoryError::DatosInvalidos(format!(
            "tamaño de caja ftyp inválido ({box_size} bytes)"
        )));
    }

    let major_brand = &bytes[8..12];
    if !major_brand
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b' ' || *b == b'-' || *b == b'_')
    {
        return Err(FactoryError::DatosInvalidos(
            "cabecera ftyp con marca de contenedor corrupta".to_string(),
        ));
    }

    Ok(())
}

fn persistir_json_atomico<T: Serialize>(ruta: &Path, valor: &T) -> FactoryResult<()> {
    let bytes = serde_json::to_vec_pretty(valor).map_err(|e| {
        FactoryError::DatosInvalidos(format!(
            "error serializando JSON '{}': {e}",
            ruta.display()
        ))
    })?;
    escribir_bytes_atomico(ruta, &bytes)
}

fn escribir_bytes_atomico(ruta: &Path, bytes: &[u8]) -> FactoryResult<()> {
    if let Some(padre) = ruta.parent() {
        if !padre.as_os_str().is_empty() {
            fs::create_dir_all(padre).map_err(|source| FactoryError::IoArtefacto {
                ruta: padre.to_path_buf(),
                source,
            })?;
        }
    }

    let nombre = ruta
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("broll.tmp");
    let ruta_tmp = ruta.with_file_name(format!(".{nombre}.tmp.{}", Uuid::new_v4()));

    {
        let mut archivo = File::create(&ruta_tmp).map_err(|source| FactoryError::IoArtefacto {
            ruta: ruta_tmp.clone(),
            source,
        })?;
        archivo
            .write_all(bytes)
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: ruta_tmp.clone(),
                source,
            })?;
        archivo
            .sync_all()
            .map_err(|source| FactoryError::IoArtefacto {
                ruta: ruta_tmp.clone(),
                source,
            })?;
    }

    fs::rename(&ruta_tmp, ruta).map_err(|source| FactoryError::IoArtefacto {
        ruta: ruta.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn plantillas_higgsfield(
    vertical: VerticalComercial,
    perfil: PerfilPsicologico,
) -> Vec<(String, &'static str, u8)> {
    let tono_estetico = match perfil {
        PerfilPsicologico::AnaliticoFinanciero => "clean architectural precision, sharp natural daylight, minimalist corporate color grading",
        PerfilPsicologico::DinamicoVisual => "dramatic golden hour rim lighting, high-contrast cinema anamorphic lens flare, luxury kinetic energy",
        PerfilPsicologico::ConservadorPatrimonial => "warm timeless twilight illumination, calm editorial depth of field, refined heritage materials",
    };

    let base: Vec<(&'static str, &'static str, u8)> = match vertical {
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial) => vec![
            (
                "Vertical 9:16 ultra-realistic 4K cinema shot of a modern Grade-A logistics industrial park in northern Mexico, gleaming concrete loading bays, sleek glass corporate facade, 35mm anamorphic lens, no text, no logos, no watermarks, silent visual B-roll",
                "fpv_architectural_glide",
                5,
            ),
            (
                "Vertical 9:16 editorial cinematography inside an expansive high-bay automated distribution warehouse, geometric steel trusses, sunlit floor reflections, 50mm cinema prime lens, no text, no watermarks, clean B-roll",
                "dolly_in_slow",
                5,
            ),
            (
                "Vertical 9:16 aerial establishing descent over a thriving industrial corridor and modern business park skyline, stabilized cinema gimbal, no text, no overlays, silent B-roll",
                "orbit_architectural_rise",
                6,
            ),
        ],
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa) => vec![
            (
                "Vertical 9:16 architectural luxury pre-sale residential tower rising over a vibrant Mexican metropolitan skyline, warm glass reflections, cantilevered terraces with greenery, 24mm cinema lens, no text, no logos, clean B-roll",
                "orbit_architectural_rise",
                5,
            ),
            (
                "Vertical 9:16 refined architectural studio desk with brushed travertine, brass scale model details, and sunlight casting crisp geometric shadows across modern blueprints, 50mm macro cinema prime, no readable text, silent B-roll",
                "dolly_in_slow",
                4,
            ),
            (
                "Vertical 9:16 smooth glide across a contemporary rooftop infinity terrace overlooking city mountains, architectural accent lighting, anamorphic bokeh, no text, clean B-roll",
                "fpv_architectural_glide",
                5,
            ),
        ],
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Residencial) => vec![
            (
                "Vertical 9:16 luxury contemporary residence interior in Mexico, double-height travertine walls, warm oak slats, sunlit courtyard reflecting pool, 35mm cinema prime, no text, no watermarks, silent B-roll",
                "handheld_luxury_pan",
                5,
            ),
            (
                "Vertical 9:16 exterior facade of a minimalist concrete and glass luxury villa surrounded by mature agave and architectural uplighting, 40mm cinema lens, no text, clean B-roll",
                "dolly_in_slow",
                5,
            ),
            (
                "Vertical 9:16 sculptural marble kitchen and terrace overlooking lush private gardens, floating dust motes in sunbeams, 50mm shallow depth of field, no text, silent B-roll",
                "gimbal_tracking_push",
                4,
            ),
        ],
        VerticalComercial::Seguros(SubverticalSeguros::HombreClave) => vec![
            (
                "Vertical 9:16 cinematic shot of an executive glass boardroom high above a modern Mexican financial district, dark walnut table reflections, calm strategic authority, 50mm cinema lens, no text, silent B-roll",
                "dolly_in_slow",
                5,
            ),
            (
                "Vertical 9:16 slow silhouette push-in behind a confident business leader standing by floor-to-ceiling windows overlooking financial skyscrapers, shallow depth of field, no text, clean B-roll",
                "gimbal_tracking_push",
                5,
            ),
            (
                "Vertical 9:16 moody architectural close-up of sleek steel and glass corporate towers reflected in calm water symbolizing business resilience, 35mm anamorphic lens, no text, silent B-roll",
                "handheld_luxury_pan",
                4,
            ),
        ],
        VerticalComercial::Seguros(SubverticalSeguros::Gmm) => vec![
            (
                "Vertical 9:16 bright ultra-modern private medical center atrium with warm timber louvers, sunlit glass facade, tranquil healing architecture, 35mm lens, no text, no signage, clean B-roll",
                "dolly_in_slow",
                5,
            ),
            (
                "Vertical 9:16 healthy multi-generational family walking peacefully through a sunlit tree-lined park, soft bokeh, candid editorial warmth, 85mm cinema lens, no text, silent B-roll",
                "gimbal_tracking_push",
                5,
            ),
            (
                "Vertical 9:16 serene minimalist wellness interior with natural sunlight pouring through linen curtains onto warm stone surfaces, 50mm prime, no text, silent B-roll",
                "handheld_luxury_pan",
                4,
            ),
        ],
        VerticalComercial::Seguros(SubverticalSeguros::Educativo) => vec![
            (
                "Vertical 9:16 prestigious modern university courtyard with timeless stone arches and glass library pavilion, soft sunbeams, 35mm cinema lens, no text, no logos, clean B-roll",
                "fpv_architectural_glide",
                5,
            ),
            (
                "Vertical 9:16 quiet sunlit study desk with leather-bound notebooks and architectural lamp glow, shallow depth of field, 50mm prime, no readable text, silent B-roll",
                "dolly_in_slow",
                4,
            ),
            (
                "Vertical 9:16 slow upward architectural glide along sunlit library bookshelves and soaring timber ceilings, 24mm cinema lens, no text, silent B-roll",
                "orbit_architectural_rise",
                5,
            ),
        ],
        VerticalComercial::Seguros(SubverticalSeguros::Vida | SubverticalSeguros::Patrimonial) => vec![
            (
                "Vertical 9:16 peaceful luxury family estate courtyard surrounded by cypress trees, timeless wealth preservation atmosphere, 50mm cinema lens, no text, silent B-roll",
                "dolly_in_slow",
                5,
            ),
            (
                "Vertical 9:16 refined close-up of sunlight glistening across architectural stone, brushed bronze, and a serene reflecting pool inside a private garden, 85mm prime, no text, clean B-roll",
                "handheld_luxury_pan",
                5,
            ),
            (
                "Vertical 9:16 majestic mountain ridges and modern metropolitan skyline in soft amber mist symbolizing long-term financial horizon, stabilized aerial gimbal, no text, silent B-roll",
                "orbit_architectural_rise",
                6,
            ),
        ],
    };

    base.into_iter()
        .map(|(escena, camara, dur)| (format!("{escena}; lighting & look: {tono_estetico}."), camara, dur))
        .collect()
}
