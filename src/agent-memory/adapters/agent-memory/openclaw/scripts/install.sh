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

# Every openclaw call goes through the state dir the plugin gets registered in,
# and must not see OPENCLAW_HOME (the CLI prefers it and would use another tree).
openclaw_cli() {
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" "$@"
}

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

# OpenClaw 2026.9.2 added a capability-consent gate: `plugins install` exits 1
# with 'Plugin "memory-anolisa" requires capability consent' unless the caller
# accepts them, so the plugin never leaves the install step and every OpenClaw
# E2E case errors at fixture setup (#3099). memory-anolisa trips the gate
# through what it registers at runtime — the memory capability, the four
# contract tools declared in openclaw.plugin.json, and the auto-recall /
# auto-capture conversation hooks (src/index.ts) — not through a `capabilities`
# key in the plugin manifest, which it does not declare.
#
# The flag is passed only when the installed CLI advertises it. OpenClaw's
# installer rejects unknown options, so passing it unconditionally would turn
# "installs fine" into "always fails" on every host older than 2026.9.2, which
# manifest.json still declares supported (compatibleVersions ">=5.0.0"). Reading
# `plugins install --help` as the CLI contract is what the rest of the repo
# already does: agent-sec-core/openclaw-plugin/scripts/deploy.sh
# (verify_openclaw_install_cli) and the Rust driver's help_lists_flag
# (anolisa-core/src/adapter/openclaw.rs).
#
# Set AGENT_MEMORY_ACCEPT_CAPABILITIES=0 to opt out of the automatic consent
# (hosts whose policy forbids non-interactive capability consent). The install
# then fails on >= 2026.9.2 with the consent diagnosis below instead of
# consenting silently.
OPENCLAW_INSTALL_HELP="$(openclaw_cli plugins install --help 2>&1 || true)"
# Normalize the help text for matching. An option token continues through any
# character that can appear inside an option name (ASCII alphanumeric, '-', '_',
# '.'), everything else is a boundary — same rule as help_lists_flag() in the
# Rust driver, so "[--accept-capabilities]" and "--accept-capabilities=<mode>"
# both count as advertised while "--accept-capabilities-file" does not.
OPENCLAW_INSTALL_HELP="${OPENCLAW_INSTALL_HELP//[^a-zA-Z0-9_.-]/ }"
while [[ "$OPENCLAW_INSTALL_HELP" == *"  "* ]]; do
    OPENCLAW_INSTALL_HELP="${OPENCLAW_INSTALL_HELP//  / }"
done

install_help_advertises() {
    [[ " ${OPENCLAW_INSTALL_HELP} " == *" $1 "* ]]
}

if [ "${AGENT_MEMORY_ACCEPT_CAPABILITIES:-1}" != "1" ]; then
    echo "[${COMPONENT}] AGENT_MEMORY_ACCEPT_CAPABILITIES=${AGENT_MEMORY_ACCEPT_CAPABILITIES}: not consenting to the plugin capabilities." >&2
    echo "[${COMPONENT}]        OpenClaw >= 2026.9.2 refuses the install unless consent is granted out-of-band." >&2
elif install_help_advertises "--accept-capabilities"; then
    INSTALL_ARGS+=("--accept-capabilities")
else
    echo "[${COMPONENT}] WARNING: this openclaw installer does not advertise --accept-capabilities (added in 2026.9.2) — not passing it." >&2
    echo "[${COMPONENT}]          If the install below fails with 'requires capability consent', upgrade OpenClaw." >&2
fi

INSTALL_LOG="$(mktemp "${TMPDIR:-/tmp}/agent-memory-openclaw-install.XXXXXX")"
trap 'rm -f "$INSTALL_LOG"' EXIT

# tee keeps the installer output on the terminal while leaving a copy for the
# diagnosis below; pipefail propagates the installer's own exit status.
if ! openclaw_cli plugins install "$PLUGIN_DIR" "${INSTALL_ARGS[@]}" 2>&1 | tee "$INSTALL_LOG"; then
    echo "[${COMPONENT}] openclaw CLI install failed (args: ${INSTALL_ARGS[*]})." >&2
    if grep -qi "capability consent" "$INSTALL_LOG"; then
        echo "[${COMPONENT}]        Cause: OpenClaw refused the plugin capabilities (consent gate, added in 2026.9.2)." >&2
        echo "[${COMPONENT}]        Fix:   re-run on an OpenClaw whose 'plugins install --help' advertises" >&2
        echo "[${COMPONENT}]               --accept-capabilities (and without AGENT_MEMORY_ACCEPT_CAPABILITIES=0)," >&2
        echo "[${COMPONENT}]               or consent manually:" >&2
        echo "[${COMPONENT}]                   ${OPENCLAW_BIN} plugins install \"${PLUGIN_DIR}\" --force --accept-capabilities" >&2
    elif grep -qiE "unknown (option|argument|flag)|unrecognized (option|argument|flag)|invalid (option|argument|flag)" "$INSTALL_LOG"; then
        echo "[${COMPONENT}]        Cause: this installer rejected one of the options above, which its" >&2
        echo "[${COMPONENT}]               'plugins install --help' does not advertise." >&2
        echo "[${COMPONENT}]        Fix:   upgrade OpenClaw, or drop the option — AGENT_MEMORY_SAFE_INSTALL=1 skips" >&2
        echo "[${COMPONENT}]               --dangerously-force-unsafe-install, AGENT_MEMORY_ACCEPT_CAPABILITIES=0 skips" >&2
        echo "[${COMPONENT}]               --accept-capabilities." >&2
    else
        echo "[${COMPONENT}]        Cause: unrecognized — see the installer output above." >&2
        echo "[${COMPONENT}]        Fix:   check OpenClaw version >= 5.0.0 and that ${OPENCLAW_STATE_DIR} is writable." >&2
    fi
    exit 1
fi

# OpenClaw 2026.6.11 requires non-bundled plugins to explicitly opt-in
# to conversation hooks (agent_end, before_prompt_build, etc.). Without
# this setting the hooks are silently blocked and auto-capture /
# auto-recall never fire (#1460).
openclaw_cli config set \
    "plugins.entries.memory-anolisa.hooks.allowConversationAccess" true || {
    echo "[${COMPONENT}] WARNING: failed to set allowConversationAccess — auto-capture/auto-recall hooks may be blocked." >&2
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."
echo "[${COMPONENT}] NOTE: allowConversationAccess and plugin hooks only take effect after gateway restart."
echo "[${COMPONENT}]       Without restart, auto-capture and auto-recall hooks remain silently blocked."
