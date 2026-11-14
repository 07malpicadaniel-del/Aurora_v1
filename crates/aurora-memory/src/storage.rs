use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurora_domain::{
    uuid_a_node_id_u64, validar_prospecto, AuroraError, IntoVectorPayload, Prospecto,
    RelacionTopologica,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, info_span, warn};
use uuid::Uuid;

use crate::embedder::TextEmbedder;
use crate::error::{MemoryError, MemoryResult};
use crate::islands::{EntradaVectorial, GestorIslas, IslaVertical};
use crate::temporal_graph::{ConfigDecaimiento4D, GrafoTemporal4D};

const WAL_MAGIC: [u8; 4] = *b"AWAL";
const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WalOp {
    UpsertProspecto {
        prospecto: Box<Prospecto>,
        isla: IslaVertical,
        vector: Vec<f32>,
    },
    InsertarRelacion {
        relacion: RelacionTopologica,
    },
    PurgeTombstone {
        prospecto_id: Uuid,
        node_id: u64,
        purgado_en: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotEstado {
    version: u32,
    dimension: usize,
    nodos: Vec<(Prospecto, IslaVertical, Vec<f32>)>,
    relaciones: Vec<RelacionTopologica>,
    blacklist_opt_out: Vec<Uuid>,
}

/// Motor transaccional completo en CPU/RAM + NVMe (WAL + snapshots `bincode` + purga LFPDPPP).
pub struct MotorMemoriaAurora {
    islas: GestorIslas,
    grafo: GrafoTemporal4D,
    embedder: Arc<dyn TextEmbedder>,
    opt_out_blacklist: HashSet<Uuid>,
    data_dir: Option<PathBuf>,
    wal_writer: Option<BufWriter<File>>,
}

impl MotorMemoriaAurora {
    /// Crea un motor efímero puro en RAM (sin persistencia en disco), útil para pruebas y benchmarks.
    #[must_use]
    pub fn en_memoria(
        embedder: Arc<dyn TextEmbedder>,
        config_decaimiento: ConfigDecaimiento4D,
    ) -> Self {
        let dim = embedder.dimension();
        Self {
            islas: GestorIslas::new(dim),
            grafo: GrafoTemporal4D::new(config_decaimiento),
            embedder,
            opt_out_blacklist: HashSet::new(),
            data_dir: None,
            wal_writer: None,
        }
    }

    /// Abre o recupera el estado completo desde un directorio en disco NVMe (`state.bin` + `wal.log`).
    #[tracing::instrument(skip(data_dir, embedder), fields(dir = %data_dir.as_ref().display()))]
    pub fn abrir(
        data_dir: impl AsRef<Path>,
        embedder: Arc<dyn TextEmbedder>,
        config_decaimiento: ConfigDecaimiento4D,
    ) -> MemoryResult<Self> {
        let dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(|e| MemoryError::WalIo {
            ruta: dir.clone(),
            source: e,
        })?;

        let dim = embedder.dimension();
        let mut motor = Self {
            islas: GestorIslas::new(dim),
            grafo: GrafoTemporal4D::new(config_decaimiento),
            embedder,
            opt_out_blacklist: HashSet::new(),
            data_dir: Some(dir.clone()),
            wal_writer: None,
        };

        let snapshot_path = dir.join("state.bin");
        if snapshot_path.is_file() {
            motor.cargar_snapshot(&snapshot_path)?;
        }

        let wal_path = dir.join("wal.log");
        if wal_path.is_file() {
            motor.reproducir_wal(&wal_path)?;
        }

        let writer = abrir_wal_append(&wal_path)?;
        motor.wal_writer = Some(writer);

        info!(
            nodos = motor.grafo.len_nodos(),
            relaciones = motor.grafo.len_relaciones(),
            vectores = motor.islas.total_vectores(),
            "MotorMemoriaAurora recuperado desde NVMe"
        );
        Ok(motor)
    }

    /// Acceso de solo lectura al gestor de islas vectoriales.
    #[must_use]
    pub fn islas(&self) -> &GestorIslas {
        &self.islas
    }

    /// Acceso de solo lectura al grafo temporal 4D.
    #[must_use]
    pub fn grafo(&self) -> &GrafoTemporal4D {
        &self.grafo
    }

    /// Acceso de solo lectura al embedder CPU configurado.
    #[must_use]
    pub fn embedder(&self) -> &Arc<dyn TextEmbedder> {
        &self.embedder
    }

    /// Indica si un UUID fue purgado previamente por exclusión LFPDPPP (`opt_out`).
    #[must_use]
    pub fn esta_en_blacklist(&self, prospecto_id: Uuid) -> bool {
        self.opt_out_blacklist.contains(&prospecto_id)
    }

    /// Valida LFPDPPP, vectoriza en CPU, registra en WAL y actualiza isla + grafo 4D.
    #[tracing::instrument(skip(self, prospecto), fields(prospecto_id = %prospecto.id, isla = isla.nombre()))]
    pub fn ingestar_prospecto(
        &mut self,
        prospecto: Prospecto,
        isla: IslaVertical,
    ) -> MemoryResult<u64> {
        if self.opt_out_blacklist.contains(&prospecto.id) {
            return Err(AuroraError::OptOutActive {
                prospecto_id: prospecto.id,
            }
            .into());
        }

        validar_prospecto(&prospecto)?;
        let payload = prospecto.to_vector_payload()?;
        let vector = self.embedder.embed(&payload.texto_embedding)?;

        self.aplicar_upsert_con_vector(
            prospecto,
            isla,
            vector,
            payload.peso_inicial_4d,
            payload.timestamp_captura_unix,
            true,
        )
    }

    /// Inserta un prospecto con un vector precalculado (útil en ingesta masiva paralela CPU).
    pub fn ingestar_prospecto_con_vector(
        &mut self,
        prospecto: Prospecto,
        isla: IslaVertical,
        vector: Vec<f32>,
    ) -> MemoryResult<u64> {
        if self.opt_out_blacklist.contains(&prospecto.id) {
            return Err(AuroraError::OptOutActive {
                prospecto_id: prospecto.id,
            }
            .into());
        }

        validar_prospecto(&prospecto)?;
        let payload = prospecto.to_vector_payload()?;
        self.aplicar_upsert_con_vector(
            prospecto,
            isla,
            vector,
            payload.peso_inicial_4d,
            payload.timestamp_captura_unix,
            true,
        )
    }

    /// Registra una relación del Círculo Cercano en WAL y en el grafo 4D.
    pub fn ingestar_relacion(&mut self, relacion: RelacionTopologica) -> MemoryResult<()> {
        if self.opt_out_blacklist.contains(&relacion.origen_id)
            || self.opt_out_blacklist.contains(&relacion.destino_id)
        {
            return Err(MemoryError::PurgaLfpdppp(
                "no se puede enlazar un nodo excluido por opt_out".to_string(),
            ));
        }

        self.grafo.insertar_relacion(relacion.clone())?;
        self.escribir_wal(&WalOp::InsertarRelacion { relacion })?;
        Ok(())
    }

    /// Ingesta atómica por lote de prospectos (con vectorización paralela `rayon`) y relaciones.
    #[tracing::instrument(skip(self, prospectos, relaciones), fields(prospectos = prospectos.len(), relaciones = relaciones.len(), isla = isla.nombre()))]
    pub fn ingestar_lote(
        &mut self,
        prospectos: Vec<Prospecto>,
        relaciones: Vec<RelacionTopologica>,
        isla: IslaVertical,
    ) -> MemoryResult<Vec<u64>> {
        let mut textos = Vec::with_capacity(prospectos.len());
        let mut payloads = Vec::with_capacity(prospectos.len());

        for p in &prospectos {
            if self.opt_out_blacklist.contains(&p.id) {
                return Err(AuroraError::OptOutActive { prospecto_id: p.id }.into());
            }
            validar_prospecto(p)?;
            let pl = p.to_vector_payload()?;
            textos.push(pl.texto_embedding.clone());
            payloads.push(pl);
        }

        let refs: Vec<&str> = textos.iter().map(String::as_str).collect();
        let vectores = self.embedder.embed_batch(&refs)?;

        let mut node_ids = Vec::with_capacity(prospectos.len());
        for ((prospecto, payload), vector) in prospectos
            .into_iter()
            .zip(payloads.into_iter())
            .zip(vectores.into_iter())
        {
            let nid = self.aplicar_upsert_con_vector(
                prospecto,
                isla,
                vector,
                payload.peso_inicial_4d,
                payload.timestamp_captura_unix,
                true,
            )?;
            node_ids.push(nid);
        }

        for rel in relaciones {
            self.ingestar_relacion(rel)?;
        }

        Ok(node_ids)
    }

    /// Purga física LFPDPPP (`opt_out`):
    /// 1. Añade `prospecto_id` a la lista negra de exclusión.
    /// 2. Elimina el vector de todas las islas (`swap_remove`).
    /// 3. Elimina el nodo y todas sus aristas entrantes/salientes del grafo 4D.
    /// 4. Registra `PurgeTombstone` en WAL y compacta `state.bin` + trunca `wal.log` para que
    ///    no quede ningún dato personal recuperable en disco NVMe.
    #[tracing::instrument(skip(self), fields(prospecto_id = %prospecto_id))]
    pub fn purgar_prospecto_opt_out(&mut self, prospecto_id: Uuid) -> MemoryResult<()> {
        let _span = info_span!("purgar_prospecto_opt_out", prospecto_id = %prospecto_id).entered();
        let node_id = uuid_a_node_id_u64(prospecto_id);

        self.opt_out_blacklist.insert(prospecto_id);
        let vectores_eliminados = self.islas.eliminar_de_todas(node_id);
        let nodo_eliminado = self.grafo.eliminar_nodo_y_aristas(prospecto_id).is_some();

        self.escribir_wal(&WalOp::PurgeTombstone {
            prospecto_id,
            node_id,
            purgado_en: Utc::now(),
        })?;

        if self.data_dir.is_some() {
            self.compactar_wal()?;
        }

        warn!(
            prospecto_id = %prospecto_id,
            vectores_eliminados,
            nodo_eliminado,
            "Purga física LFPDPPP ejecutada y compactada en disco"
        );
        Ok(())
    }

    /// Escribe atómicamente el snapshot `state.bin` (solo registros vivos) y trunca `wal.log`.
    #[tracing::instrument(skip(self))]
    pub fn compactar_wal(&mut self) -> MemoryResult<()> {
        let Some(dir) = self.data_dir.clone() else {
            return Ok(());
        };

        // Asegurar cierre de writer previo antes de truncar WAL.
        if let Some(mut writer) = self.wal_writer.take() {
            writer.flush().map_err(|e| MemoryError::WalIo {
                ruta: dir.join("wal.log"),
                source: e,
            })?;
        }

        let mut nodos_vivos = Vec::with_capacity(self.grafo.len_nodos());
        for (isla, entrada) in self.islas.iter_entradas() {
            if let Some(reg) = self.grafo.obtener_nodo(entrada.node_id) {
                nodos_vivos.push((reg.prospecto.clone(), isla, entrada.vector.clone()));
            }
        }

        let relaciones_vivas: Vec<RelacionTopologica> =
            self.grafo.iter_relaciones().cloned().collect();

        let snapshot = SnapshotEstado {
            version: SNAPSHOT_VERSION,
            dimension: self.islas.dimension(),
            nodos: nodos_vivos,
            relaciones: relaciones_vivas,
            blacklist_opt_out: self.opt_out_blacklist.iter().copied().collect(),
        };

        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| MemoryError::SerializacionBinaria(e.to_string()))?;

        let tmp_path = dir.join("state.bin.tmp");
        let final_path = dir.join("state.bin");
        {
            let mut file = File::create(&tmp_path).map_err(|e| MemoryError::WalIo {
                ruta: tmp_path.clone(),
                source: e,
            })?;
            file.write_all(&bytes).map_err(|e| MemoryError::WalIo {
                ruta: tmp_path.clone(),
                source: e,
            })?;
            file.sync_all().map_err(|e| MemoryError::WalIo {
                ruta: tmp_path.clone(),
                source: e,
            })?;
        }

        fs::rename(&tmp_path, &final_path).map_err(|e| MemoryError::WalIo {
            ruta: final_path,
            source: e,
        })?;

        let wal_path = dir.join("wal.log");
        {
            let wal_trunc = File::create(&wal_path).map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
            wal_trunc.sync_all().map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
        }

        self.wal_writer = Some(abrir_wal_append(&wal_path)?);
        debug!("Snapshot NVMe compactado y WAL truncado");
        Ok(())
    }

    fn aplicar_upsert_con_vector(
        &mut self,
        prospecto: Prospecto,
        isla: IslaVertical,
        vector: Vec<f32>,
        peso_inicial_4d: u32,
        timestamp_captura_unix: u64,
        registrar_en_wal: bool,
    ) -> MemoryResult<u64> {
        let prospecto_id = prospecto.id;
        let node_id = self.grafo.upsert_prospecto(prospecto.clone(), isla)?;

        self.islas.upsert(
            isla,
            EntradaVectorial {
                node_id,
                prospecto_id,
                vector: vector.clone(),
                peso_inicial_4d,
                timestamp_captura_unix,
            },
        )?;

        if registrar_en_wal {
            self.escribir_wal(&WalOp::UpsertProspecto {
                prospecto: Box::new(prospecto),
                isla,
                vector,
            })?;
        }

        Ok(node_id)
    }

    fn escribir_wal(&mut self, op: &WalOp) -> MemoryResult<()> {
        let (Some(writer), Some(dir)) = (&mut self.wal_writer, &self.data_dir) else {
            return Ok(());
        };

        let payload =
            bincode::serialize(op).map_err(|e| MemoryError::SerializacionBinaria(e.to_string()))?;
        let len_u32 = u32::try_from(payload.len())
            .map_err(|_| MemoryError::SerializacionBinaria("trama WAL excede u32".to_string()))?;
        let checksum = fnv1a_checksum(&payload);
        let wal_path = dir.join("wal.log");

        writer
            .write_all(&WAL_MAGIC)
            .map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
        writer
            .write_all(&len_u32.to_le_bytes())
            .map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
        writer
            .write_all(&checksum.to_le_bytes())
            .map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
        writer
            .write_all(&payload)
            .map_err(|e| MemoryError::WalIo {
                ruta: wal_path.clone(),
                source: e,
            })?;
        writer.flush().map_err(|e| MemoryError::WalIo {
            ruta: wal_path,
            source: e,
        })?;

        Ok(())
    }

    fn cargar_snapshot(&mut self, path: &Path) -> MemoryResult<()> {
        let bytes = fs::read(path).map_err(|e| MemoryError::WalIo {
            ruta: path.to_path_buf(),
            source: e,
        })?;
        let snap: SnapshotEstado = bincode::deserialize(&bytes)
            .map_err(|e| MemoryError::SerializacionBinaria(e.to_string()))?;

        if snap.version != SNAPSHOT_VERSION {
            return Err(MemoryError::WalCorrupto(format!(
                "versión de snapshot incompatible: {}",
                snap.version
            )));
        }
        if snap.dimension != self.islas.dimension() {
            return Err(MemoryError::DimensionMismatch {
                esperada: self.islas.dimension(),
                recibida: snap.dimension,
            });
        }

        for id in snap.blacklist_opt_out {
            self.opt_out_blacklist.insert(id);
        }

        for (prospecto, isla, vector) in snap.nodos {
            if self.opt_out_blacklist.contains(&prospecto.id) {
                continue;
            }
            let payload = prospecto.to_vector_payload()?;
            self.aplicar_upsert_con_vector(
                prospecto,
                isla,
                vector,
                payload.peso_inicial_4d,
                payload.timestamp_captura_unix,
                false,
            )?;
        }

        for rel in snap.relaciones {
            if !self.opt_out_blacklist.contains(&rel.origen_id)
                && !self.opt_out_blacklist.contains(&rel.destino_id)
            {
                self.grafo.insertar_relacion(rel)?;
            }
        }

        Ok(())
    }

    fn reproducir_wal(&mut self, path: &Path) -> MemoryResult<()> {
        let file = File::open(path).map_err(|e| MemoryError::WalIo {
            ruta: path.to_path_buf(),
            source: e,
        })?;
        let mut reader = BufReader::new(file);
        let mut magic = [0u8; 4];

        loop {
            match reader.read_exact(&mut magic) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(MemoryError::WalIo {
                        ruta: path.to_path_buf(),
                        source: e,
                    });
                }
            }

            if magic != WAL_MAGIC {
                return Err(MemoryError::WalCorrupto(
                    "cabecera mágica WAL inválida".to_string(),
                ));
            }

            let mut len_buf = [0u8; 4];
            let mut sum_buf = [0u8; 8];
            reader
                .read_exact(&mut len_buf)
                .map_err(|e| MemoryError::WalCorrupto(format!("longitud truncada: {e}")))?;
            reader
                .read_exact(&mut sum_buf)
                .map_err(|e| MemoryError::WalCorrupto(format!("checksum truncado: {e}")))?;

            let len = u32::from_le_bytes(len_buf) as usize;
            let expected_sum = u64::from_le_bytes(sum_buf);

            let mut payload = vec![0u8; len];
            reader
                .read_exact(&mut payload)
                .map_err(|e| MemoryError::WalCorrupto(format!("payload truncado: {e}")))?;

            if fnv1a_checksum(&payload) != expected_sum {
                return Err(MemoryError::WalCorrupto(
                    "checksum WAL no coincide; trama corrupta".to_string(),
                ));
            }

            let op: WalOp = bincode::deserialize(&payload)
                .map_err(|e| MemoryError::SerializacionBinaria(e.to_string()))?;

            match op {
                WalOp::UpsertProspecto {
                    prospecto,
                    isla,
                    vector,
                } => {
                    let prospecto = *prospecto;
                    if !self.opt_out_blacklist.contains(&prospecto.id) {
                        let pl = prospecto.to_vector_payload()?;
                        self.aplicar_upsert_con_vector(
                            prospecto,
                            isla,
                            vector,
                            pl.peso_inicial_4d,
                            pl.timestamp_captura_unix,
                            false,
                        )?;
                    }
                }
                WalOp::InsertarRelacion { relacion } => {
                    if !self.opt_out_blacklist.contains(&relacion.origen_id)
                        && !self.opt_out_blacklist.contains(&relacion.destino_id)
                    {
                        self.grafo.insertar_relacion(relacion)?;
                    }
                }
                WalOp::PurgeTombstone {
                    prospecto_id,
                    node_id,
                    ..
                } => {
                    self.opt_out_blacklist.insert(prospecto_id);
                    self.islas.eliminar_de_todas(node_id);
                    self.grafo.eliminar_nodo_y_aristas(prospecto_id);
                }
            }
        }

        Ok(())
    }
}

fn abrir_wal_append(path: &Path) -> MemoryResult<BufWriter<File>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| MemoryError::WalIo {
            ruta: path.to_path_buf(),
            source: e,
        })?;
    Ok(BufWriter::new(file))
}

fn fnv1a_checksum(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
