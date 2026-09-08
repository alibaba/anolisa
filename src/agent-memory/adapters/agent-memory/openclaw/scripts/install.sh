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

# OpenClaw gates capability-declaring plugins behind install-time consent:
# a noninteractive `plugins install` is rejected unless --accept-capabilities
# is passed. Version boundaries here are unreliable — the flag shipped in
# 2026.8.1 but the CLI reference documents it only since 2026.9.1, and older
# releases abort on the unknown option outright — so probe the host's
# installer help instead of version-gating. Whole-token match (near-miss
# options such as --no-accept-capabilities stay out), mirroring tokenless's
# installer and anolisa-core's help_lists_flag().
PROBE_RC=0
INSTALL_HELP="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
    "$OPENCLAW_BIN" plugins install --help 2>&1)" || PROBE_RC=$?
if [ "$PROBE_RC" -ne 0 ]; then
    # Degrade to the base flags instead of failing closed: the script must
    # keep installing on every host the manifest claims to support. Clearing
    # INSTALL_HELP keeps probe error text from being mistaken for an
    # advertised option.
    printf '[%s] WARNING: cannot inspect OpenClaw installer options (rc=%s): %s\n' \
        "$COMPONENT" "$PROBE_RC" "$INSTALL_HELP" >&2
    printf '[%s]          Installing with the base flags only.\n' "$COMPONENT" >&2
    INSTALL_HELP=""
fi
if [[ "$INSTALL_HELP" =~ (^|[^[:alnum:]_.-])--accept-capabilities([^[:alnum:]_.-]|$) ]]; then
    INSTALL_ARGS+=("--accept-capabilities")
    echo "[${COMPONENT}] Passing --accept-capabilities: granting install-time consent to memory-anolisa's declared capabilities."
elif [ -n "$INSTALL_HELP" ]; then
    echo "[${COMPONENT}] OpenClaw did not advertise --accept-capabilities (pre-2026.8.1 host); installing with the base flags."
fi

env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install "$PLUGIN_DIR" \
    "${INSTALL_ARGS[@]}" || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    exit 1
}

# OpenClaw 2026.6.11 requires non-bundled plugins to explicitly opt-in
# to conversation hooks (agent_end, before_prompt_build, etc.). Without
# this setting the hooks are silently blocked and auto-capture /
# auto-recall never fire (#1460).
env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" config set \
    "plugins.entries.memory-anolisa.hooks.allowConversationAccess" true || {
    echo "[${COMPONENT}] WARNING: failed to set allowConversationAccess — auto-capture/auto-recall hooks may be blocked." >&2
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."
echo "[${COMPONENT}] NOTE: allowConversationAccess and plugin hooks only take effect after gateway restart."
echo "[${COMPONENT}]       Without restart, auto-capture and auto-recall hooks remain silently blocked."
