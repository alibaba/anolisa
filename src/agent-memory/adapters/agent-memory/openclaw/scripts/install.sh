#!/usr/bin/env bash
# install.sh — Deploy the agent-memory OpenClaw plugin via the openclaw CLI.
#
# This script ONLY deploys an already-built plugin.
# Compilation is the Makefile's job:
#     make -C src/agent-memory build-openclaw-plugin
# If dist/index.js is missing, exit with a clear error.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-agent-memory}"
# ANOLISA_ADAPTER_DIR is injected by anolisa-adapter-ctl (FHS spec §2.4).
# Fall back to the directory containing manifest.json.
PLUGIN_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}/openclaw"

OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
# Honour OPENCLAW_HOME (default: ~/.openclaw). Detect / install /
# uninstall all consult the same variable so a non-default location
# behaves consistently across the three lifecycle scripts.
export OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"

echo "[${COMPONENT}] Installing ${AGENT} plugin..."

if ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "[${COMPONENT}] openclaw CLI not found (OPENCLAW_BIN=${OPENCLAW_BIN}) — skipping plugin installation."
    echo "[${COMPONENT}] Install OpenClaw first, then run this script again."
    exit 0
fi

if [ ! -f "$PLUGIN_DIR/dist/index.js" ]; then
    echo "[${COMPONENT}] ERROR: $PLUGIN_DIR/dist/index.js is missing." >&2
    echo "[${COMPONENT}]        Build the plugin first:" >&2
    echo "[${COMPONENT}]            cd $PLUGIN_DIR && npm run build" >&2
    exit 1
fi

# OpenClaw's signature/sandbox checks default ON. To force-install
# despite those checks (e.g. during local development before the
# adapter bundle is signed), set AGENT_MEMORY_UNSAFE_INSTALL=1
# explicitly. The default path goes through the regular safe install.
INSTALL_ARGS=("--force")
if [ "${AGENT_MEMORY_UNSAFE_INSTALL:-0}" = "1" ]; then
    echo "[${COMPONENT}] AGENT_MEMORY_UNSAFE_INSTALL=1: bypassing OpenClaw signature checks." >&2
    INSTALL_ARGS+=("--dangerously-force-unsafe-install")
fi

"$OPENCLAW_BIN" plugins install "$PLUGIN_DIR" \
    "${INSTALL_ARGS[@]}" || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    echo "[${COMPONENT}] If install fails on signature checks, re-run with AGENT_MEMORY_UNSAFE_INSTALL=1." >&2
    exit 1
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."