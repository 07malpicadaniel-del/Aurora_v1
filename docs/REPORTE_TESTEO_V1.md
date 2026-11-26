# Reporte de Testeos de Aceptación Real (UAT v1) — Aurora v2 & AXIS v1

**Fecha de ejecución:** 2026-11-25 (UTC `2026-11-26T02:25Z–02:37Z`)  
**Estación de trabajo:** Arch Linux x86_64 · AMD Ryzen 9 7900 · 64 GB RAM · NVIDIA GeForce RTX 5060 Ti 16 GB VRAM (`16311 MiB`) · NVMe local  
**Modelo LLM residente en BOB (`:8086`, `slot_id = 0`):** `gemma-3-12b-it-Q4_K_M.gguf` (`llama-server` CUDA `-ngl 99 -c 4096 --parallel 1`)  
**Seguridad UAT:** `smtp.habilitado = false`, cero tokens externos YouTube/Meta (cero tráfico saliente a terceros), cero `unwrap()`/`expect()` en producción, limpieza post-UAT completada.

---

## 1. Matriz de Aceptación (PASS / FAIL)

| Bloque | Caso de Prueba | Resultado | Evidencia Medida |
| :--- | :--- | :---: | :--- |
| **Bloque 1.1** | Compilación release (`aurora-daemon`, `aurora-smoke`) | **PASS** | `cargo build --release` en `16.53 s` (recompilación incremental `8.57 s`) |
| **Bloque 1.2** | Instalación binaria `~/.local/bin/aurora-daemon` y `systemd --user` | **PASS** | `deploy/scripts/install-systemd-user.sh` + `systemd-analyze --user verify` sin errores |
| **Bloque 1.3** | Arranque `bob-llama.service` (`:8086`) y `aurora-daemon.service` (`:8090`) | **PASS** | Ambos `active (running)` y `enabled`; BOB listo en `3 s`, daemon en `1 s` |
| **Bloque 1.4** | `GET /api/v1/health` y `target/release/aurora-smoke` (4 subsistemas reales) | **PASS** | `bob_llm`, `piper_tts`, `chromium_headless`, `ffmpeg_nvenc` en `[REAL]`; inferencia GBNF en `8487 ms` |
| **Bloque 2.1** | Cola de briefs Higgsfield (`POST /api/v1/factory/broll/briefs`) | **PASS** | 2 briefs `9:16` persistidos en NVMe para `Seguros::HombreClave` / `AnaliticoFinanciero` |
| **Bloque 2.2** | Registro de clip B-Roll `1080x1920` 5s (`POST /api/v1/factory/broll/registrar`) | **PASS** | Importado a `~/.local/share/aurora/assets/broll/seguros_hombre_clave/` con SHA-256 verificado y brief marcado `Importado` |
| **Bloque 2.3** | Cacería OSINT + perfilado BOB GBNF (`Boca del Río, Veracruz`, `Seguros::HombreClave`) | **PASS** | Sonda web pública en `513 ms` (`0` registros sin token DENUE); cacería con semilla pública UAT + 2 roles BOB GBNF en `28376 ms` (`vertical_optima = Seguros::HombreClave`) |
| **Bloque 2.4** | Sala Gatekeeper (`GET /api/v1/gatekeeper/pendientes`) y aprobación (`POST /api/v1/gatekeeper/aprobar/:id`) | **PASS** | `modo_video == "CinematograficoBroll"`, `clips_broll_usados` = 1, `dossier.pdf` + `nota_voz.wav` + `subtitulos.srt` + `reel_vertical.mp4` reales en NVMe (`11826 ms`) |
| **Bloque 2.5** | Validación `ffprobe` y pico de VRAM durante `h264_nvenc` (`< 12.5 GB`) | **PASS** | `h264 1080x1920 30/1 fps` + `aac 22050 Hz`; pico VRAM **`10656 MiB` (`10.41 GiB`)** con `NVENC 100%` |
| **Bloque 2.6** | Resiliencia WAL tras `systemctl --user restart aurora-daemon` | **PASS** | Reinicio en `2 s`; `GET /api/v1/memory/telarana` conservó `5/5` nodos incluido el prospecto UAT |
| **Bloque 3.1** | Bloqueo PII en publicación orgánica (`POST /api/v1/pulse/organico/publicar`) | **PASS** | Correo -> `422 PII_FIREWALL_BLOCK`; nombre de prospecto activo -> `422 PII_FIREWALL_BLOCK`; `libre_de_pii=false` -> `422 ORGANIC_VALIDATION_ERROR`; copy limpio + SHA-256 -> `200 OK` (`simulado: true`) |
| **Bloque 3.2** | Telemetría (`POST /api/v1/pulse/telemetria`) + Consolidación DPO (`POST /api/v1/dpo/consolidar`) | **PASS** | `3760 ms` con Auditor BOB GBNF real; `.jsonl` (`1.5 KB`, 2 pares `chosen`/`rejected`) con **0 fugas de PII** |
| **Bloque 3.3** | Purga física LFPDPPP (`POST /api/v1/lfpdppp/opt-out`) + reinicio + escaneo binario NVMe | **PASS** | `opt-out` en `9 ms`; tras reinicio `GET /api/v1/memory/subgrafo/:id` -> `404 NOT_FOUND`; `wal.log = 0 bytes` y `0` coincidencias PII en `state.bin` (`8281 bytes`) |
| **Bloque 4.1** | Consejo Multi-Agente (`GET /api/v1/council/cuotas`, `POST /api/v1/council/delegar`) | **PASS** | Cascada nube atendida por `ClaudePro` (`10335 ms`, `quota_state.json` persistido en NVMe); delegación local `BobLocal` en `2101 ms` (`45.40 tok/s`) |
| **Bloque 4.2** | Bloqueo de seguridad `aurora-forge` sobre `crates/aurora-domain/src/validation.rs` | **PASS** | HTTP `403 FORBIDDEN` (`FORGE_SECURITY_BLOCK`) y `0` ramas `forge/*` creadas |
| **Bloque 5.1** | Portal móvil `/mobile` y autenticación Bearer (`AURORA_MOBILE_TOKEN`) en instancia aislada `:18090` | **PASS** | `/mobile` -> `200 OK` (`15090 B` HTML monocromático); API sin token o token inválido -> `401`; con Bearer válido -> `200` |
| **Bloque 5.2** | Compatibilidad de `axis_v1` (`pnpm build`, `cargo test --workspace`, contrato HTTP `:8090`) | **PASS** | `pnpm build` en `461 ms`, `3/3` tests Rust en verde, `8/8` endpoints cockpit en `:8090` -> `200 OK` |

---

## 2. Hallazgos Reales Detectados y Corregidos durante el UAT

Todos los parches respetan `unwrap_used = "deny"` y `expect_used = "deny"`, pasando `cargo clippy --workspace --all-targets -- -D warnings` y `66/66` tests:

1. **Código HTTP `422` en rechazos de publicación orgánica (`crates/aurora-daemon/src/error.rs`):** `PulseError::DespachoFallido { canal: "PublicacionOrganica", .. }` caía en `500 INTERNAL_SERVER_ERROR`. Se mapeó explícitamente a `422 UNPROCESSABLE_ENTITY` (`ORGANIC_VALIDATION_ERROR`).
2. **Protección contra nombres/aliases de prospectos activos en Carril Verde (`crates/aurora-daemon/src/state.rs`):** `publicar_reel_organico` ahora valida título, descripción y hashtags contra `PiiFirewall::verificar_texto_sin_pii_generica` y contra todos los nodos activos del grafo 4D vía `PiiFirewall::verificar_libre_de_pii`, devolviendo `422 PII_FIREWALL_BLOCK` ante cualquier coincidencia.
3. **`PATH` de `systemd --user` en Arch Linux (`deploy/systemd/aurora-daemon.service`):** Se añadió `Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin` para que el demonio descubra `piper`, `claude` y `codex` instalados en `~/.local/bin` (pasando a `3/4` CLIs del Consejo detectados en producción).
4. **Coherencia `vertical_optima` vs `isla` en modelos 12B (`crates/aurora-memory/src/islands.rs`, `crates/aurora-cortex/src/roles.rs`, `crates/aurora-cortex/src/engine.rs`):** En la primera corrida del Bloque 2, Gemma 3 12B devolvió `vertical_optima = Inmobiliaria::ComercialIndustrial` pese a estar en `isla = seguros_hombre_clave` y sugerir `producto_gancho = "Seguros HombreClave"`, haciendo que `BibliotecaBroll` degradara a diapositivas. Se añadió `IslaVertical::a_vertical_comercial()`, el hint explícito `vertical_isla_json` en el prompt del `Perfilador` y reconciliación determinista ante contradicciones de sector. En la repetición, `modo_video` pasó a `CinematograficoBroll` al primer intento.
5. **Caso límite de puntuación en `PiiFirewall` (`crates/aurora-council/src/sanitizer.rs`):** `es_telefono_mx_valido` descartaba números telefónicos mexicanos seguidos de coma en prosa (p. ej. `+52 229 555 0199, RFC ...`) porque el filtro de comas no distinguía `, ` (prosa) de `,00` (cifras monetarias). Corregido con test de regresión en `crates/aurora-council/tests/council_tests.rs`.

---

## 3. Latencias Medidas y Perfil de Hardware (RTX 5060 Ti 16 GB)

### Latencias E2E Medidas en Vivo

| Operación | Motor Real Ejercitado | Latencia Medida |
| :--- | :--- | ---: |
| `aurora-smoke` (1 rol `Perfilador` GBNF) | `gemma-3-12b-it-Q4_K_M` (`slot_id = 0`) | `8487 ms` |
| Sonda OSINT web pública MX (sin token DENUE) | `reqwest` + `rustls-tls` | `513 ms` |
| Cacería OSINT + perfilado completo (2 roles GBNF) | `OSINT` + `Memoria 4D` + `BOB` (`slot_id = 0`) | `28376 ms` |
| Aprobación Gatekeeper + Manufactura `CinematograficoBroll` (total) | `Chrome PDF` + `Piper CPU` + `SRT` + `FFmpeg NVENC` | `11826 ms` |
| ↳ Maquetado Dossier PDF (`58721 B`) | `/usr/bin/google-chrome-stable --headless=new --disable-gpu` | `564 ms` |
| ↳ Locución neuronal `nota_voz.wav` (`3.37 MB`, `76.5 s`) | `/home/daniel/.local/bin/piper` (`es_MX-ald-medium.onnx` CPU) | `2990 ms` |
| ↳ Composición Reel `1080x1920` B-Roll + overlays + SRT (`76.8 MB`) | `ffmpeg` + `h264_nvenc` (GPU hardware encoder) | `8330 ms` |
| Consolidación DPO con Auditor GBNF (5 eventos -> 2 pares `.jsonl`) | `BOB Auditor` + recalibración 4D | `3760 ms` |
| Purga física LFPDPPP `opt-out` + compactación WAL | `state.bin` atómico + truncado `wal.log` | `9 ms` |
| Delegación Consejo Multi-Agente (nube con `PiiFirewall`) | `ClaudePro` CLI (`/home/daniel/.local/bin/claude`) | `10335 ms` |
| Delegación Consejo Multi-Agente (local) | `BobLocal` (`gemma-3-12b`, `45.40 tok/s`) | `2101 ms` |
| Compilación frontend `axis_v1` (`pnpm build`) | `tsc && vite build` | `461 ms` |

### Consumo de VRAM Medido (`nvidia-smi`, 54 muestras @ 200 ms durante `h264_nvenc`)

- **Línea base (BOB `gemma-3-12b` cargado + escritorio Wayland/Niri):** `10370–10384 MiB` (`10.13–10.14 GiB`) / `16311 MiB`
- **Pico real durante codificación `CinematograficoBroll` `h264_nvenc`:** **`10656 MiB` (`10.41 GiB`) / `16311 MiB`** (umbral UAT: `< 12.5 GB` — **margen libre superior a 5.5 GiB**)
- **Utilización pico:** GPU `89 %` · NVENC **`100 %`**

### Evidencia `ffprobe` del Reel `CinematograficoBroll` Producido

```json
{
  "streams": [
    {
      "index": 0,
      "codec_name": "h264",
      "codec_type": "video",
      "width": 1080,
      "height": 1920,
      "r_frame_rate": "30/1",
      "avg_frame_rate": "30/1"
    },
    {
      "index": 1,
      "codec_name": "aac",
      "codec_type": "audio",
      "sample_rate": "22050",
      "channels": 1
    }
  ],
  "format": {
    "duration": "76.498594",
    "size": "76837419",
    "bit_rate": "8035433"
  }
}
```

---

## 4. Salida de `aurora-smoke` y Estado Final `systemd --user`

### `target/release/aurora-smoke`

```text
Modo efectivo: real_con_fallback | BOB URL: http://127.0.0.1:8086 (slot_id = 0)
  [REAL    ] BOB Córtex (llama-server) | http://127.0.0.1:8086/health | En línea en slot único VRAM id=0
  [REAL    ] Piper TTS CPU        | /home/daniel/.local/bin/piper | Binario + modelo ONNX '/home/daniel/.local/share/aurora/models/tts/es_MX-ald-medium.onnx' listos
  [REAL    ] Chromium Headless CPU | /usr/bin/google-chrome-stable | Navegador headless CPU listo (--headless=new --disable-gpu)
  [REAL    ] FFmpeg + NVENC       | /usr/bin/ffmpeg | FFmpeg + codec hardware h264_nvenc listo
  [FALLBACK] Browser CDP (:9222)  | http://127.0.0.1:9222 | Endpoint CDP inactivo
  [FALLBACK] SMTP TLS (rustls)    | smtp.resend.com:587 | smtp.habilitado = false
Modelo activo en BOB (http://127.0.0.1:8086/props): gemma-3-12b-it-Q4_K_M.gguf
Latencia de inferencia real GBNF: 8487 ms
```

### `systemctl --user status bob-llama.service aurora-daemon.service` (Post-Limpieza)

```text
● bob-llama.service - BOB Local LLM Server (llama.cpp CUDA slot_id=0 on 127.0.0.1:8086)
     Loaded: loaded (/home/daniel/.config/systemd/user/bob-llama.service; enabled; preset: enabled)
     Active: active (running) since Wed 2026-11-25 20:27:04 CST
   Main PID: 567096 (llama-server)
     Memory: 599.7M (peak: 719.3M)

● aurora-daemon.service - Aurora v2 Autonomous Commercial Intelligence Daemon (AFK 24/7)
     Loaded: loaded (/home/daniel/.config/systemd/user/aurora-daemon.service; enabled; preset: enabled)
     Active: active (running) since Wed 2026-11-25 20:37:10 CST
   Main PID: 624070 (aurora-daemon)
     Memory: 4.8M (peak: 19.4M)
     Logs: subsistemas=bob_llm=REAL, piper_tts=REAL, chromium_headless=REAL, ffmpeg_nvenc=REAL,
           cdp_browser=FALLBACK, smtp_transport=FALLBACK, consejo_clis_reales=3/4
```

---

## 5. Verificación de Limpieza Post-UAT

- Eliminados clips y briefs sintéticos de `~/.local/share/aurora/assets/broll/` y archivo fuente temporal.
- Eliminados artefactos UAT (`28fbb6c7-...`, `123df7f0-...`) de `~/.local/share/aurora/artefactos/`.
- Eliminado dataset sintético UAT `aurora_dpo_preferences_20261126T023337Z.jsonl` de `~/.local/share/aurora/dpo/`.
- Restaurada `gatekeeper_queue.json` al estado previo al UAT (`ordenes_pendientes: 0`).
- Verificada ausencia total de ramas `forge/*` y archivos temporales `/tmp/uat_*`.
- Servicios `bob-llama.service` (`:8086`) y `aurora-daemon.service` (`:8090`) activos, estables y listos para operación comercial real.
