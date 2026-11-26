#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNIT_SRC_DIR="${SCRIPT_DIR}/../systemd"
UNIT_DST_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/aurora"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/aurora"

mkdir -p "${UNIT_DST_DIR}" "${CONFIG_DIR}" "${DATA_DIR}/assets/broll"

cp "${UNIT_SRC_DIR}/bob-llama.service" "${UNIT_DST_DIR}/bob-llama.service"
cp "${UNIT_SRC_DIR}/aurora-daemon.service" "${UNIT_DST_DIR}/aurora-daemon.service"

echo "[Aurora] Unidades instaladas en ${UNIT_DST_DIR}:"
echo "  - ${UNIT_DST_DIR}/bob-llama.service"
echo "  - ${UNIT_DST_DIR}/aurora-daemon.service"

if command -v systemd-analyze >/dev/null 2>&1; then
  echo "[Aurora] Validando sintaxis con systemd-analyze --user verify..."
  systemd-analyze --user verify \
    "${UNIT_DST_DIR}/bob-llama.service" \
    "${UNIT_DST_DIR}/aurora-daemon.service" || true
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload || true
fi

cat <<'EOF'

Siguientes pasos recomendados:
  1. Compila e instala el binario release si aún no existe en ~/.local/bin/aurora-daemon:
       cargo build --release -p aurora-daemon
       install -Dm755 target/release/aurora-daemon ~/.local/bin/aurora-daemon
  2. Configura ~/.config/aurora/aurora.toml (y opcionalmente ~/.config/aurora/secrets.env
     con AURORA_MOBILE_TOKEN, AURORA_YOUTUBE_ACCESS_TOKEN, AURORA_META_* y AURORA_SMTP_PASSWORD).
  3. Habilita y arranca los servicios de usuario 24/7:
       systemctl --user enable --now bob-llama.service aurora-daemon.service
       loginctl enable-linger "$USER"
  4. Revisa estado y bitácoras:
       systemctl --user status bob-llama.service aurora-daemon.service
       journalctl --user -u aurora-daemon.service -f
EOF
