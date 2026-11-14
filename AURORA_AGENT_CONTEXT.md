# AURORA_AGENT_CONTEXT.md

Contexto operativo y reglas de ingeniería obligatorias para cualquier agente o desarrollador que trabaje dentro del repositorio `Aurora_v1` (**Aurora v2**).

## 1. Propósito del Proyecto

**Aurora v2** es un motor autónomo de inteligencia comercial, perfilamiento topológico 4D y generación multimodal en **Rust puro** para los sectores **Inmobiliario** (Preventa, Residencial, Comercial/Industrial) y **Asegurador** (Vida, GMM, Hombre Clave, Educativo, Patrimonial) en México.

## 2. Ecosistema Local y Repositorios Hermanos

- **Entorno anfitrión:** Arch Linux (Wayland / Hyprland) sobre AMD Ryzen 9, **64 GB RAM** y **NVIDIA RTX 5060 Ti (16 GB VRAM)**.
- **Repositorio actual (`Aurora_v1`):** `https://github.com/07malpicadaniel-del/Aurora_v1.git`
- **Motor cognitivo y memoria (`../bob_betta`):** `https://github.com/07malpicadaniel-del/bob_betta.git`
  - `cortex`: Cliente/gestor de inferencia local (`llama.cpp` server en `127.0.0.1:8085`).
  - `vector_db`: Base vectorial topológica 4D nativa (HNSW + `VECTOR_DIM = 768` + `causal_plane` + `decay` exponencial + WAL).
  - `router`: Orquestador de herramientas (`tools`, `gatekeeper`, `browser_cdp`, `self_improve`).
- **Interfaz de escritorio (`../axis_v1`):** `https://github.com/07malpicadaniel-del/axis_v1.git`
  - Aplicación Tauri v2 + React 19 + Tailwind CSS v4 (`pnpm`) para visualización y supervisión en fases posteriores.

## 3. Reglas de Presupuesto de Hardware

Para evitar saturación térmica y errores OOM en sesiones autónomas largas:

1. **NVIDIA RTX 5060 Ti (16 GB VRAM) — USO EXCLUSIVO:**
   - Inferencia LLM local (`cortex` / `llama-server`).
   - Codificación acelerada de video (`NVENC` vía `ffmpeg` en Fase 4).
   - **Prohibido** cargar modelos de embeddings o TTS en VRAM mientras el LLM esté residente.
2. **CPU (AMD Ryzen 9) + RAM del Sistema (64 GB) — CARGA ESTRUCTURAL:**
   - Grafo topológico HNSW e hipergrafo causal (`vector_db`).
   - Cálculo de embeddings en CPU (AVX2/AVX-512 con `candle`).
   - Síntesis de voz (TTS) local en CPU.
   - Concurrencia masiva de extracción OSINT (`tokio`) y maquetación documental.

## 4. Mapa de Crates del Workspace

| Crate | Ruta | Estado Actual | Responsabilidad |
| :--- | :--- | :--- | :--- |
| `aurora-domain` | `crates/aurora-domain` | **Completa (Fase 0)** | Modelos canónicos, errores `AuroraError`, validación determinista LFPDPPP, puente `IntoVectorPayload`. |
| `aurora-osint` | `crates/aurora-osint` | **Completa (Fase 1)** | Enjambre asíncrono de extracción OSINT sobre fuentes públicas mexicanas y normalización en memoria. |
| `aurora-memory` | `crates/aurora-memory` | **Completa (Fase 2)** | Telaraña topológica 4D, embeddings multilingües CPU/RAM, islas vectoriales SIMD y WAL NVMe con purga física LFPDPPP. |
| `aurora-cortex` | `crates/aurora-cortex` | **Completa (Fase 3)** | Córtex comercial de BOB (`Perfilador`, `DirectorCreativo`, `Auditor`), gramáticas GBNF estrictas y `ToolDispatcher` bajo Gatekeeper. |
| `aurora-factory` | `crates/aurora-factory` | Stub (Fase 4) | Generación local de `DossierPdf`, `NotaVozTts`, `ReelVerticalMp4` y `MensajeDirecto`. |
| `aurora-pulse` | `crates/aurora-pulse` | Stub (Fase 5) | Colas de despacho supervisado por Gatekeeper y puente hacia `../axis_v1`. |

## 5. Reglas de Calidad Inquebrantables

1. **Cero `unwrap()` ni `expect()`:** Todo código del workspace (incluidos lints de Clippy `unwrap_used = "deny"` y `expect_used = "deny"`) debe propagar errores mediante `AuroraResult<T>` (`Result<T, AuroraError>`) con `thiserror`.
2. **Aislamiento de Stack:** Prohibido introducir bases de datos externas (Postgres, Supabase, ChromaDB, Qdrant, SQLite externo) o runtimes de Python/Node.js en el núcleo del motor. Todo es Rust puro.
3. **Cumplimiento LFPDPPP Obligatorio:**
   - Todo `Prospecto` requiere `fuente_origen`, `url_evidencia` HTTP/HTTPS verificable, `capturado_en` (UTC) y bandera `opt_out`.
   - Cualquier nodo con `opt_out == true` debe ser bloqueado antes de vectorización o despacho (`AuroraError::OptOutActive`).
4. **Observabilidad Headless:** Instrumentar los flujos clave (validación, proyección, ingesta, generación y despacho) con `tracing` (`info_span!`, `debug!`, `warn!`).
5. **Alcance Estricto por Fase:** No implementar código de fases futuras fuera de su turno sin aprobación arquitectónica explícita.
