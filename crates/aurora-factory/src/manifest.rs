use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use aurora_cortex::{BriefCreativo, DiagnosticoComercial, OrdenProduccionActivo};
use aurora_domain::{validar_prospecto, ActivoGenerado, EstadoAprobacion, Prospecto};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::error::{FactoryError, FactoryResult};

/// Solicitud completa recibida por `aurora-factory` tras aprobación explícita del Gatekeeper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolicitudManufactura {
    /// Orden emitida por `aurora-cortex` que DEBE estar en [`EstadoAprobacion::Aprobado`].
    pub orden: OrdenProduccionActivo,
    /// Prospecto canónico validado bajo LFPDPPP.
    pub prospecto: Prospecto,
    /// Diagnóstico estratégico generado por BOB (`Perfilador`).
    pub diagnostico: DiagnosticoComercial,
    /// Brief multimodal adaptado psicológicamente (`DirectorCreativo`).
    pub brief: BriefCreativo,
}

impl SolicitudManufactura {
    /// Verifica de forma determinista la barrera Zero-Bypass Gatekeeper, LFPDPPP y coherencia de IDs.
    ///
    /// Debe ejecutarse ANTES de crear directorios o archivos en disco NVMe.
    pub fn validar_barrera_gatekeeper(&self) -> FactoryResult<()> {
        if self.prospecto.lfpdppp.opt_out {
            warn!(
                prospecto_id = %self.prospecto.id,
                orden_id = %self.orden.orden_id,
                "Manufactura abortada por bandera LFPDPPP opt_out"
            );
            return Err(FactoryError::ProspectoOptOut {
                prospecto_id: self.prospecto.id,
            });
        }

        validar_prospecto(&self.prospecto)?;

        if self.orden.prospecto_id != self.prospecto.id
            || self.diagnostico.prospecto_id != self.prospecto.id
            || self.brief.prospecto_id != self.prospecto.id
        {
            return Err(FactoryError::DatosInvalidos(format!(
                "inconsistencia de prospecto_id entre orden ({}), diagnostico ({}), brief ({}) y prospecto ({})",
                self.orden.prospecto_id,
                self.diagnostico.prospecto_id,
                self.brief.prospecto_id,
                self.prospecto.id
            )));
        }

        let estado_orden = self.orden.estado_gatekeeper();
        if estado_orden != EstadoAprobacion::Aprobado {
            warn!(
                orden_id = %self.orden.orden_id,
                prospecto_id = %self.prospecto.id,
                estado = ?estado_orden,
                "Barrera Zero-Bypass bloqueó manufactura sin EstadoAprobacion::Aprobado"
            );
            return Err(FactoryError::NoAprobadoPorGatekeeper {
                orden_id: self.orden.orden_id,
                prospecto_id: self.prospecto.id,
                estado_actual: estado_orden,
            });
        }

        for activo in self.orden.activos_iniciales() {
            if activo.estado != EstadoAprobacion::Aprobado {
                warn!(
                    orden_id = %self.orden.orden_id,
                    activo_id = %activo.id,
                    estado = ?activo.estado,
                    "Barrera Zero-Bypass detectó activo no aprobado dentro de la orden"
                );
                return Err(FactoryError::NoAprobadoPorGatekeeper {
                    orden_id: self.orden.orden_id,
                    prospecto_id: self.prospecto.id,
                    estado_actual: activo.estado,
                });
            }
        }

        if self.brief.gancho_3s.trim().is_empty() || self.brief.guion_tts.trim().is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el brief requiere gancho_3s y guion_tts no vacíos".to_string(),
            ));
        }

        if self.brief.escenas_reel.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el brief debe incluir al menos una EscenaReel".to_string(),
            ));
        }

        if self.brief.secciones_dossier_pdf.is_empty() {
            return Err(FactoryError::DatosInvalidos(
                "el brief debe incluir al menos una sección para el Dossier PDF".to_string(),
            ));
        }

        Ok(())
    }
}

/// Metadatos verificables de un artefacto materializado en NVMe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadatosArtefacto {
    /// Ruta absoluta o relativa en almacenamiento NVMe.
    pub ruta_nvme: PathBuf,
    /// Huella criptográfica SHA-256 en hexadecimal minúscula (64 caracteres).
    pub sha256_hex: String,
    /// Huella rápida FNV-1a de 64 bits en hexadecimal (16 caracteres).
    pub fnv1a_64_hex: String,
    /// Tamaño exacto del archivo en bytes.
    pub tamano_bytes: u64,
    /// Duración multimedia en milisegundos cuando aplica (`WAV`, `MP4`, `SRT`).
    pub duracion_ms: Option<u64>,
    /// Tiempo invertido en renderizar/codificar el artefacto en milisegundos.
    pub tiempo_renderizado_ms: u64,
}

/// Modalidad de composición empleada al manufacturar el Reel vertical MP4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ModoComposicionVideo {
    /// Respaldo clásico animando diapositivas estáticas HTML -> PNG (`zoompan`).
    #[default]
    DiapositivasEstaticas,
    /// Composición cinematográfica mezclando clips maestros B-Roll + overlays PNG transparentes.
    CinematograficoBroll,
}

/// Paquete multimodal completo listo para despacho supervisado en Fase 5 (`aurora-pulse`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaqueteManufacturado {
    /// Identificador único de la orden procesada.
    pub orden_id: Uuid,
    /// Identificador de la campaña asociada.
    pub campana_id: Uuid,
    /// Prospecto destinatario.
    pub prospecto_id: Uuid,
    /// Directorio aislado de salida `<base_dir>/<prospecto_id>/<orden_id>`.
    pub directorio_salida: PathBuf,
    /// Dossier ejecutivo en PDF.
    pub dossier_pdf: MetadatosArtefacto,
    /// Nota de voz personalizada WAV sintetizada en CPU/RAM.
    pub nota_voz_wav: MetadatosArtefacto,
    /// Reel vertical MP4 `1080x1920` ensamblado con subtítulos y audio.
    pub reel_vertical_mp4: MetadatosArtefacto,
    /// Archivo de subtítulos SRT sincronizado.
    pub subtitulos_srt: MetadatosArtefacto,
    /// Mensaje de texto directo personalizado listo para el canal sugerido.
    pub mensaje_directo_texto: String,
    /// Activos canónicos actualizados con rutas finales en NVMe y estado `Aprobado`.
    pub activos_actualizados: Vec<ActivoGenerado>,
    /// Modalidad de composición visual aplicada (`CinematograficoBroll` vs `DiapositivasEstaticas`).
    #[serde(default)]
    pub modo_video: ModoComposicionVideo,
    /// Identificadores de los clips maestros B-Roll utilizados cuando `modo_video == CinematograficoBroll`.
    #[serde(default)]
    pub clips_broll_usados: Vec<Uuid>,
    /// Marca temporal UTC de cierre de manufactura.
    pub generado_en: DateTime<Utc>,
}

/// Lee un archivo en NVMe y calcula su tamaño, SHA-256 y FNV-1a 64-bit.
pub async fn construir_metadatos_artefacto(
    ruta: &Path,
    duracion_ms: Option<u64>,
    tiempo_renderizado_ms: u64,
) -> FactoryResult<MetadatosArtefacto> {
    let bytes = tokio::fs::read(ruta)
        .await
        .map_err(|source| FactoryError::IoArtefacto {
            ruta: ruta.to_path_buf(),
            source,
        })?;

    if bytes.is_empty() {
        return Err(FactoryError::DatosInvalidos(format!(
            "el artefacto generado '{}' está vacío (0 bytes)",
            ruta.display()
        )));
    }

    Ok(MetadatosArtefacto {
        ruta_nvme: ruta.to_path_buf(),
        sha256_hex: sha256_bytes(&bytes),
        fnv1a_64_hex: format!("{:016x}", fnv1a_64_bytes(&bytes)),
        tamano_bytes: bytes.len() as u64,
        duracion_ms,
        tiempo_renderizado_ms,
    })
}

/// Calcula el hash FNV-1a de 64 bits en Rust puro.
#[must_use]
pub fn fnv1a_64_bytes(data: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Calcula el digest SHA-256 (FIPS 180-4) en Rust puro sin dependencias externas.
#[must_use]
pub fn sha256_bytes(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4,
        0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe,
        0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f,
        0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da, 0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7,
        0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc,
        0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
        0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070, 0x19a4_c116,
        0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
        0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7,
        0xc671_78f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, word_bytes) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut hex = String::with_capacity(64);
    for word in h {
        let _ = write!(hex, "{word:08x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::sha256_bytes;

    #[test]
    fn sha256_vectores_nist_verificados() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
