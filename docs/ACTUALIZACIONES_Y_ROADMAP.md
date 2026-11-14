# Actualizaciones Arquitectónicas y Roadmap Operativo

Registro de evoluciones aplicadas en **Aurora v2** respecto a la visión inicial de `AURORA_V1.md` y hoja de ruta ejecutable para las fases posteriores.

## 1. Actualizaciones Aplicadas en Fase 0 (Respecto a `AURORA_V1.md`)

1. **Formalización de Fase 0 (Dominio y Cumplimiento antes de OSINT):**
   - `AURORA_V1.md` iniciaba directamente en la Fase 1 (scraper OSINT). En Aurora v2 se antepuso la **Fase 0** para establecer modelos tipados, validación determinista y trazabilidad LFPDPPP (`MetadatosLfpdppp`) antes de tocar red o almacenamiento.
2. **Separación Estricta de Responsabilidades en 4 Crates:**
   - `aurora-domain` concentra el dominio puro sin IO ni runtime asíncrono, permitiendo pruebas rápidas, serialización binaria/JSON verificada y reutilización directa por `aurora-osint`, `aurora-factory`, `aurora-pulse` y eventualmente `../axis_v1` (Tauri v2).
3. **Adaptación Explícita al Motor de Decaimiento 4D de `bob_betta`:**
   - `GatilloVital` incorpora `ocurrido_en` (`DateTime<Utc>`) y `peso_urgencia` (`[0.0, 1.0]`), proyectados por `IntoVectorPayload` hacia `timestamp_captura_unix`, `gatillos_proyectados` y `peso_inicial_4d` (`[1, 100]`), alineados con `TopologicalDecay::compute_dynamic_score`.
4. **Contrato de Supervisión Gatekeeper en Modelos:**
   - `ActivoGenerado` incluye `EstadoAprobacion` (`PendienteGatekeeper`, `Aprobado`, `Despachado`, `Rechazado`) y `motivo_rechazo` validado, asegurando que ningún entregable salga sin revisión.
5. **Observabilidad Headless Obligatoria:**
   - Sustitución de logs ad-hoc por spans y eventos estructurados con `tracing`.

## 2. Roadmap y Criterios de Salida por Fase

### Fase 1 — `aurora-osint` (Enjambre de Extracción en Rust)
- **Objetivo:** Recolectar señales públicas mexicanas (ej. DENUE/INEGI, registros corporativos públicos, noticias de expansión logística) con `tokio` + cliente HTTP puro en Rust y normalización sin alocaciones innecesarias.
- **Criterio de entrada:** Fase 0 compilando y pasando `clippy -D warnings` y `cargo test`.
- **Criterio de salida:** Pipeline de captura que rechaza automáticamente registros sin `url_evidencia` HTTP/HTTPS o marcados en lista local de `opt_out`, emitiendo `VectorPayload` validados.

### Fase 2 — Integración con `bob_betta/vector_db` (Telaraña Topológica 4D)
- **Objetivo:** Implementar el adaptador concreto que consume `VectorPayload` y `RelacionTopologica`, genera embeddings en CPU, inserta en isla aislada y registra aristas del Círculo Cercano (`SocioComercial`, `Familiar`, `MismoClubCirculo`, `InteraccionFrecuente`).
- **Actualización necesaria en `bob_betta`:** Evaluar soporte de embedder multilingüe 768d y compactación de WAL para borrado físico ante `opt_out`.

### Fase 3 — Orquestación Táctica (`bob_betta/cortex` + `router`)
- **Objetivo:** Consultas de subgrafos (prospecto + vecinos de 1 y 2 saltos + gatillos vigentes tras decaimiento 4D) para que el LLM elija la `VerticalComercial` óptima y redacte guiones adaptados al `PerfilPsicologico`.
- **Actualización necesaria:** Esquemas JSON estrictos con validación posterior en Rust y verificación de presupuesto de contexto (`ctx_size`).

### Fase 4 — `aurora-factory` (Fábrica Multimodal Local)
- **Objetivo:** Generación de `DossierPdf` (HTML/CSS a PDF local sin Node/Python), `NotaVozTts` (CPU) y `ReelVerticalMp4` (`ffmpeg` con NVENC).
- **Nota de hardware:** Coordinar semáforos entre inferencia LLM y tareas NVENC en los 16 GB de VRAM de la RTX 5060 Ti.

### Fase 5 — `aurora-pulse` e Integración con `axis_v1`
- **Objetivo:** Exponer cola de `ActivoGenerado` en `PendienteGatekeeper`, métricas de campañas (`CampanaVertical`) y visor del Círculo Cercano hacia `../axis_v1` (Tauri v2 + React 19 + Tailwind v4).
- **Consideración de interfaz:** `axis_v1` ya cuenta con `tauri = "2"` y `@tauri-apps/api = "^2.12.1"`; en Fase 5 bastará incorporar `aurora-domain` como dependencia de lectura/IPC en `axis_v1/src-tauri` para compartir exactamente los mismos tipos serializables sin duplicar esquemas.

## 3. Consideraciones de Operación Continua

- **Higiene de Git y Artefactos:** Los binarios pesados (`*.wal`, `*.bin`, `*.mp4`, `*.pdf`, `*.wav`) permanecen excluidos en `.gitignore`.
- **Reproducibilidad sin Red:** Mantener dependencias de `aurora-domain` acotadas al núcleo estándar de crates Rust (`serde`, `thiserror`, `tracing`, `chrono`, `uuid`, `bincode`) para compilación rápida y determinista.
