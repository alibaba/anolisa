#!/usr/bin/env bash
# install.sh — Deploy the tokenless OpenClaw plugin via the openclaw CLI.
#
# Responsibility boundary (mirrors sec-core/openclaw-plugin/scripts/deploy.sh):
#   - This script ONLY deploys an already-built plugin.
#   - Compilation (index.ts -> dist/index.js) is the Makefile's job:
#       make -C src/tokenless build-openclaw-plugin
#     which `make install` runs automatically before `install-adapter-resources`
#     copies the result into $SHARE_DIR/openclaw.
#   - If dist/index.js is missing, exit with a clear error pointing at the
#     Makefile target. Do NOT compile here — adapters shouldn't invoke npm at
#     deploy time.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

# Allow the orchestrator (or a packaging script) to inject a specific openclaw
# binary. Defaults to whatever `openclaw` resolves to on PATH.
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
export PATH="$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"

PLUGIN_SRC="$ADAPTER_DIR/openclaw"

echo "[${COMPONENT}] Installing ${AGENT} plugin..."

if ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "[${COMPONENT}] openclaw CLI not found (OPENCLAW_BIN=${OPENCLAW_BIN}) — skipping plugin installation."
    echo "[${COMPONENT}] Install OpenClaw first, then run this script again."
    exit 0
fi

if [ ! -d "$PLUGIN_SRC" ]; then
    echo "[${COMPONENT}] Plugin source not found: $PLUGIN_SRC" >&2
    exit 1
fi

if [ ! -f "$PLUGIN_SRC/dist/index.js" ]; then
    echo "[${COMPONENT}] ERROR: $PLUGIN_SRC/dist/index.js is missing." >&2
    echo "[${COMPONENT}]        Build the plugin first:" >&2
    echo "[${COMPONENT}]            make -C src/tokenless build-openclaw-plugin" >&2
    echo "[${COMPONENT}]        (run by 'make install' automatically; only an issue when" >&2
    echo "[${COMPONENT}]         deploying a hand-assembled adapter directory)." >&2
    exit 1
fi

env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install "$PLUGIN_SRC" \
    --force --dangerously-force-unsafe-install || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    exit 1
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."
