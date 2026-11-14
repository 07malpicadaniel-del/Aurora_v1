# Arquitectura Funcional y Técnica — Aurora v2

## 1. Visión General

**Aurora v2** es un motor autónomo en Rust orientado a inteligencia comercial B2B/B2C de alto valor en México para dos verticales principales:

- **Sector Inmobiliario:** `Preventa`, `Residencial`, `ComercialIndustrial`.
- **Sector Asegurador:** `Vida`, `Gmm` (Gastos Médicos Mayores), `HombreClave`, `Educativo`, `Patrimonial`.

Su arquitectura separa estrictamente el **dominio de negocio y cumplimiento legal** (`Aurora_v1`), la **memoria topológica 4D y razonamiento local** (`bob_betta`) y la **superficie de supervisión visual** (`axis_v1`).

## 2. Topología del Sistema y Fases

```text
[Fase 1: aurora-osint] ---> [Fase 0: aurora-domain (LFPDPPP + Validación)]
                                         |
                                         v (IntoVectorPayload)
                            [Fase 2: bob_betta/vector_db (HNSW 4D + CausalHypergraph)]
                                         |
                                         v
                            [Fase 3: bob_betta/cortex + router (Análisis Táctico + Gatekeeper)]
                                         |
                                         v
                            [Fase 4: aurora-factory (PDF + TTS CPU + Reel NVENC)]
                                         |
                                         v
                            [Fase 5: aurora-pulse <---> axis_v1 (Despacho + UI Tauri v2)]
```

### Desglose de Fases

- **Fase 0 — Contrato de Dominio (Implementada):**
  - Workspace Cargo en `edition = "2021"`, `resolver = "2"`.
  - Implementación íntegra de `aurora-domain` (`error`, `models`, `validation`, `bridge`) y stubs compilables para `aurora-osint`, `aurora-factory`, `aurora-pulse`.
- **Fase 1 — Enjambre de Extracción (`aurora-osint`):**
  - Clientes asíncronos sobre `tokio` para fuentes públicas mexicanas (directorios empresariales, registros públicos, comunicados corporativos).
  - Limpieza y normalización en memoria, validación LFPDPPP obligatoria y emisión de `VectorPayload`.
- **Fase 2 — La Telaraña Cognitiva (`bob_betta/vector_db`):**
  - Indexación de prospectos (vectores de 768 dimensiones en CPU) y enlaces del **Círculo Cercano** (`RelacionTopologica`).
  - Decaimiento temporal 4D alimentado por `peso_urgencia` y `ocurrido_en` de cada `GatilloVital`.
- **Fase 3 — Cerebro Orquestador (`bob_betta/cortex` + `router`):**
  - Evaluación del subgrafo del prospecto usando inferencia LLM en los 16 GB VRAM de la RTX 5060 Ti.
  - Selección de vertical/producto y estilo comunicativo según `PerfilPsicologico`.
- **Fase 4 — Fábrica Multimodal (`aurora-factory`):**
  - `DossierPdf`: maquetación HTML/CSS local a PDF.
  - `NotaVozTts`: síntesis de voz en CPU.
  - `ReelVerticalMp4`: composición ligera vía `ffmpeg` + NVENC sin competir por VRAM con el LLM activo.
  - `MensajeDirecto`: redacción personalizada.
- **Fase 5 — Pulso y Supervisión (`aurora-pulse` + `axis_v1`):**
  - Flujo explícito de estados (`PendienteGatekeeper` -> `Aprobado` -> `Despachado` / `Rechazado`) antes de cualquier envío externo.

## 3. Diseño de `aurora-domain` (Fase 0)

### 3.1 Manejo de Errores (`error.rs`)
Todos los errores se expresan mediante `AuroraError` (`thiserror::Error`) y el alias `AuroraResult<T>`:
- Cumplimiento LFPDPPP: `OptOutActive`, `FuentePublicaFaltante`, `UrlEvidenciaInvalida`.
- Dominio: `Validacion`, `PesoFueraDeRango`, `AutoRelacion`.
- Serialización y puente: `SerializacionJson`, `SerializacionBinaria`, `PuenteMemoria`.

### 3.2 Modelos Canónicos (`models.rs`)
- `VerticalComercial`: `Inmobiliaria(SubverticalInmobiliaria)` | `Seguros(SubverticalSeguros)`.
- `Prospecto`: identificador `Uuid`, `nombre_normalizado`, `aliases`, `Ubicacion`, `CapacidadAdquisitiva`, `PerfilPsicologico` (`AnaliticoFinanciero`, `DinamicoVisual`, `ConservadorPatrimonial`), `MetadatosLfpdppp` y `Vec<GatilloVital>`.
- `GatilloVital`: `TipoGatilloVital` (`AperturaEmpresa`, `CambioPuesto`, `Matrimonio`, `Nacimiento`, `ExpansionLogistica`, `CompraActivo`), descripción, `ocurrido_en: DateTime<Utc>` y `peso_urgencia: f32`.
- `RelacionTopologica`: `origen_id`, `destino_id`, `TipoVinculo` (`SocioComercial`, `Familiar`, `MismoClubCirculo`, `InteraccionFrecuente`), `fuerza: f32` y `contexto_compartido`.
- `CampanaVertical` y `ActivoGenerado` con `EstadoAprobacion` (`PendienteGatekeeper`, `Aprobado`, `Despachado`, `Rechazado`).

### 3.3 Validación Determinista (`validation.rs`)
Antes de aceptar cualquier prospecto o proyectarlo a memoria vectorial:
1. Bloqueo estricto si `opt_out == true`.
2. Verificación de `fuente_origen` no vacía y `url_evidencia` con esquema `http://` o `https://`.
3. Comprobación de rangos finitos `[0.0, 1.0]` en `peso_urgencia` y `fuerza`.
4. Prohibición de auto-relaciones (`origen_id == destino_id`).
5. Exigencia de `motivo_rechazo` cuando un `ActivoGenerado` pasa a `Rechazado`.

### 3.4 Puente Vectorial 4D (`bridge.rs`)
El trait `IntoVectorPayload` ejecuta `validar_prospecto` y genera un `VectorPayload` desacoplado:
- Proyecta el `Uuid` a un `nodo_id_sugerido: u64` determinista (FNV-1a) compatible con `vector_db::common::types::NodeId`, preservando el `Uuid` completo en `metadatos`.
- Construye un `texto_embedding` estructurado en español con ubicación, perfil, capacidad y gatillos.
- Escala la urgencia máxima `[0.0, 1.0]` a `peso_inicial_4d` (`[1, 100]`) y convierte marcas temporales UTC a segundos Unix (`u64`) compatibles con `TopologicalDecay` de `bob_betta/vector_db`.

## 4. Cumplimiento LFPDPPP

El diseño impone *privacy by design* desde Fase 0:
- Solo se permite ingesta respaldada por fuente pública trazable (`fuente_origen` + `url_evidencia` + `capturado_en`).
- La bandera `opt_out` actúa como cortocircuito determinista tanto en validación como en la generación de `VectorPayload`.
- Ningún activo comercial puede pasar a `Despachado` sin transitar explícitamente por aprobación (`EstadoAprobacion`).
