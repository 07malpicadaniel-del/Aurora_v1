# Observaciones Técnicas y Consideraciones de Integración

Documento derivado de la inspección directa del ecosistema hermano (`bob_engine` / `bob_betta`, `axis_v1` y la especificación preliminar `AURORA_V1.md`) frente a los requisitos de **Aurora v2**.

## 1. Idioma del Embedder Nativo en `bob_betta/vector_db`
- **Observación:** En `bob_engine/vector_db/src/ingestion/embedder.rs`, `NativeEmbedder` descarga y ejecuta `BAAI/bge-base-en-v1.5` (768 dimensiones, BERT en CPU vía `candle`).
- **Riesgo:** `bge-base-en-v1.5` está entrenado primordialmente para inglés. Los prospectos, cargos ejecutivos, colonias y gatillos vitales de Aurora están en español de México (ej. *"bodega aduanal"*, *"hombre clave"*, *"preventa"*).
- **Consideración para Fase 1/2:** Evaluar un modelo multilingüe de 768 dimensiones compatible con la arquitectura BERT/XLM-RoBERTa en `candle` (o mantener `VECTOR_DIM = 768` cambiando únicamente el cargador de pesos/tokenizer en `bob_betta` sin romper el layout SIMD `#[repr(C, align(64))]`).

## 2. Desajuste de Tipos entre `Prospecto` (`Uuid`) y `vector_db` (`NodeId = u64`)
- **Observación:** `bob_engine/vector_db/src/common/types.rs` define `pub type NodeId = u64;`, mientras que Aurora identifica prospectos y campañas con `Uuid` (128 bits).
- **Mitigación aplicada en Fase 0:** `aurora-domain::bridge::uuid_a_node_id_u64` proyecta determinísticamente cada `Uuid` a `u64` mediante FNV-1a y conserva el `Uuid` completo dentro de `VectorPayload.metadatos["prospecto_id"]`.
- **Consideración para Fase 2:** En la capa de persistencia del adaptador `aurora <-> bob_betta`, mantener un índice bidireccional `u64 <-> Uuid` (y verificar colisiones antes de insertar) para garantizar resolución inversa exacta al recuperar subgrafos.

## 3. Islas Dinámicas vs. `IslandCategory` Fija en `vector_db`
- **Observación:** `HippocampusEngine` en `bob_engine/vector_db/src/lib.rs` utiliza `IslandCategory::{Code, EpisodicMemory, CoreRules, WorkingMemory}`. Sin embargo, `causal_plane.rs` ya expone `DynamicIslandManager` y `VectorThalamus`.
- **Consideración:** Para evitar contaminar la memoria episódica/código de BOB con datos comerciales sujetos a LFPDPPP (y facilitar purgas por `opt_out` o derechos ARCO), los nodos de Aurora deben residir en islas aisladas (ej. `aurora_prospectos`, `aurora_inmobiliaria`, `aurora_seguros`) usando `DynamicIslandManager` o una variante dedicada en `bob_betta`.

## 4. Evolución del `Gatekeeper` en `bob_betta/router`
- **Observación:** `bob_engine/router/src/gatekeeper.rs` actualmente audita bloques de código (`Rust`, `Bash`, `Python`) con patrones destructivos de sistema (`rm -rf /`, `mkfs`, etc.).
- **Consideración para Fase 3/4:** Aurora necesita un **Gatekeeper Comercial y LFPDPPP** que:
  1. Verifique que `opt_out == false` en el instante previo al despacho (por si el titular solicitó exclusión después de la captura).
  2. Exija transición explícita `PendienteGatekeeper -> Aprobado` antes de pasar a `Despachado`.
  3. Registre `motivo_rechazo` para retroalimentación DPO/calidad sin mezclar datos personales sensibles en datasets de entrenamiento sin anonimización.

## 5. Modelo LLM Configurado en `cortex` vs. Documento Preliminar
- **Observación:** `AURORA_V1.md` menciona *"Razonamiento Táctico (Gemma)"*, mientras que `bob_engine/cortex/src/config.rs` apunta actualmente a `Qwen3.8-27B-Uncensored-noMTP-IQ4_XS.gguf` en `127.0.0.1:8085` con `ctx_size: 8192` y `n_gpu_layers: 99`.
- **Consideración:** Mantener el contrato de Aurora agnóstico al modelo concreto detrás de `cortex` (vía esquema JSON estricto para tool-calling). En una RTX 5060 Ti de 16 GB VRAM, `ctx_size` y cuantización (`IQ4_XS` / `Q4_K_M`) deben dejar margen suficiente (~1.5–2 GB libres) cuando se dispare `ffmpeg` con NVENC en Fase 4, o bien serializar la fase de renderizado respecto a la de inferencia pesada.

## 6. Regla Cero `unwrap()` / `expect()` frente a `bob_engine`
- **Observación:** En `bob_engine/vector_db/src/lib.rs` y `cortex/src/config.rs` existen llamadas a `.expect(...)` en inicialización.
- **Consideración:** `Aurora_v1` aplica `unwrap_used = "deny"` y `expect_used = "deny"` vía `[workspace.lints.clippy]`. Cuando en Fase 1/2 se conecte el adaptador hacia `bob_betta`, los fallos de WAL, embeddings o HTTP hacia `cortex` deben envolverse siempre en `AuroraError::PuenteMemoria` sin propagar `expect()` hacia Aurora.

## 7. Borrado y Derecho de Cancelación/Oposición (LFPDPPP) en WAL Inmutable
- **Observación:** `vector_db` usa un Write-Ahead Log (`bob_memories.wal`) de solo escritura secuencial (`append`) y snapshot binario (`bob_causal.bin`).
- **Consideración Legal Crítica:** Si un titular ejerce su derecho de Cancelación u Oposición (`opt_out = true`), no basta con filtrar resultados en consulta: en Fase 2 se debe contemplar un mecanismo de **compactación / tombstoning del WAL** para purgar físicamente el texto y vector del nodo excluido del disco local.
