#!/usr/bin/env bash
# install.sh — Install tokenless plugin into OpenClaw via official CLI.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

PLUGIN_SRC="$ADAPTER_DIR/openclaw"

echo "[${COMPONENT}] Installing ${AGENT} plugin..."

if ! command -v openclaw &>/dev/null; then
    echo "[${COMPONENT}] openclaw CLI not found — skipping plugin installation."
    echo "[${COMPONENT}] Install OpenClaw first, then run this script again."
    exit 0
fi

if [ ! -d "$PLUGIN_SRC" ]; then
    echo "[${COMPONENT}] Plugin source not found: $PLUGIN_SRC"
    exit 1
fi

# Use openclaw CLI for registration (handles file copy, TS compilation, config update)
openclaw plugins install "$PLUGIN_SRC" --force --dangerously-force-unsafe-install || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0"
    exit 1
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run 'openclaw gateway restart' to activate."