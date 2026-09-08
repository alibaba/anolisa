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

# --force is unconditional: a noninteractive install must never prompt. Every
# other option depends on what this host's installer advertises, so it is
# appended after the help probe below.
INSTALL_ARGS=("--force")

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

# Legacy hosts gate installs on a safety scan of child_process usage whose only
# non-interactive bypass is --dangerously-force-unsafe-install. That scan cannot
# tell memory-anolisa's spawn of the agent-memory MCP server (the standard stdio
# MCP transport) from arbitrary shell execution, so those hosts need the bypass.
# Current OpenClaw keeps the token as a deprecated no-op — 2026.9.2 advertises
# "Deprecated no-op; security.installPolicy may still block" — where passing it
# accomplishes nothing and breaks outright once the token is removed, so keep it
# only while the advertised option still has effect; on no-op hosts the scan
# follows the operator-owned security.installPolicy. Classified from the help
# text the consent probe above already captured — one CLI call, no version
# gate — with the same whole-token match and the same line-scoped "no-op" check
# as anolisa-core's unsafe_install_support() and tokenless's installer. The
# lowercase fold uses tr, not ${var,,}, keeping the idiom identical to
# tokenless's installer, which must stay runnable on macOS bash 3.2.
#
# AGENT_MEMORY_SAFE_INSTALL=1 keeps its historical meaning — never request the
# bypass — which is observable only where the flag still has effect. On no-op
# hosts both settings install identically because there is no install-time scan
# left to bypass, so the old "bypass vs. blocking safe-install scan" wording is
# gone rather than redefined into something the host no longer does.
UNSAFE_SUPPORT=absent
while IFS= read -r help_line; do
    if [[ "$help_line" =~ (^|[^[:alnum:]_.-])--dangerously-force-unsafe-install([^[:alnum:]_.-]|$) ]]; then
        help_line_lc="$(printf '%s' "$help_line" | tr '[:upper:]' '[:lower:]')"
        case "$help_line_lc" in
            *"no op"*|*"no-op"*) UNSAFE_SUPPORT=noop ;;
            *) UNSAFE_SUPPORT=effective ;;
        esac
        break
    fi
done <<< "$INSTALL_HELP"

if [ "${AGENT_MEMORY_SAFE_INSTALL:-0}" = "1" ]; then
    echo "[${COMPONENT}] AGENT_MEMORY_SAFE_INSTALL=1: not requesting --dangerously-force-unsafe-install." >&2
elif [ "$UNSAFE_SUPPORT" = "effective" ]; then
    INSTALL_ARGS+=("--dangerously-force-unsafe-install")
    echo "[${COMPONENT}] Passing --dangerously-force-unsafe-install: this OpenClaw still scans plugin code at install time and would flag the MCP server's child_process.spawn."
elif [ "$UNSAFE_SUPPORT" = "noop" ]; then
    # Silent omission would leave the operator guessing why a bypass their
    # runbook expects was not sent; a failed probe stays silent here because
    # its WARNING above already reports base-flags-only.
    echo "[${COMPONENT}] Skipping --dangerously-force-unsafe-install: this OpenClaw advertises it as a deprecated no-op; the install-time scan moved to security.installPolicy."
fi

env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install "$PLUGIN_DIR" \
    "${INSTALL_ARGS[@]}" || {
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    if [ "$UNSAFE_SUPPORT" = "noop" ]; then
        echo "[${COMPONENT}]       this OpenClaw advertises --dangerously-force-unsafe-install as a" >&2
        echo "[${COMPONENT}]       deprecated no-op; the safety scan follows the operator-owned" >&2
        echo "[${COMPONENT}]       security.installPolicy instead." >&2
    fi
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
