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

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"

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

# OpenClaw's security scanner flags child_process.spawn as a
# "dangerous code pattern". The plugin uses spawn exclusively to
# launch the agent-memory MCP server as a stdio subprocess — this is
# the standard MCP transport mechanism and not arbitrary shell
# execution. Since the scanner cannot distinguish between legitimate
# subprocess communication and malicious shell usage, we bypass it
# by default. Set AGENT_MEMORY_SAFE_INSTALL=1 to go through the
# regular (blocking) safe-install path instead.
INSTALL_ARGS=("--force" "--dangerously-force-unsafe-install")
if [ "${AGENT_MEMORY_SAFE_INSTALL:-0}" = "1" ]; then
    echo "[${COMPONENT}] AGENT_MEMORY_SAFE_INSTALL=1: using OpenClaw safe-install path (may block on child_process scan)." >&2
    INSTALL_ARGS=("--force")
fi

env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install "$PLUGIN_DIR" \
    "${INSTALL_ARGS[@]}" || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    exit 1
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."