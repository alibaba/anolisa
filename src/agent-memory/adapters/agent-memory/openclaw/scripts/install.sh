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

# The CLI env contract mirrors src/agent-sec-core/openclaw-plugin/scripts/deploy.sh:
# OPENCLAW_HOME is unset so OPENCLAW_STATE_DIR is the single source of truth.
openclaw_cli() {
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" "$@"
}

# Whether $2 (the probed `plugins install --help` text) lists $1 as a standalone
# option token. `openclaw` is a commander program, so a subcommand that does not
# register an option exits 1 with "unknown option" — installer flags may only be
# passed when the running CLI advertises them. An option token continues through
# ASCII alphanumerics, '-', '_' and '.', so a near-miss such as
# `--accept-capabilities-only` never matches `--accept-capabilities` (mirrors
# anolisa-core's help_lists_flag()).
install_help_lists_flag() {
    local flag="$1" normalized
    normalized=" $(printf '%s' "$2" | tr -cs 'A-Za-z0-9._-' ' ') "
    [[ "$normalized" == *" $flag "* ]]
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

# Read the installer's option list once; it is the CLI contract every optional
# flag below is gated on. A failed or empty probe is not fatal: the script falls
# back to the base flags, which is what it did before the probe existed. The
# probe's output is discarded on failure so an error message can never be
# mistaken for an advertised option.
INSTALL_HELP="$(openclaw_cli plugins install --help 2>&1)" || INSTALL_HELP=""
if [ -z "$INSTALL_HELP" ]; then
    echo "[${COMPONENT}] WARNING: could not read '${OPENCLAW_BIN} plugins install --help'; installing with the base flags only." >&2
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

# OpenClaw >= 2026.9.1 requires capability consent before it commits a managed
# plugin install whose source is not a trusted official/first-party record. This
# adapter installs from a local path, which carries no recorded artifact
# integrity, so a previous acceptance can never be carried forward and consent
# is requested on every install (openclaw docs/plugins/manage-plugins.md,
# "Capability consent": "Sources without recorded integrity — notably local
# paths ... ask for consent on every install"). A non-interactive
# `plugins install` cannot prompt, so without --accept-capabilities it exits 1:
#   Plugin "memory-anolisa" requires capability consent. Use openclaw plugins
#   install or openclaw plugins enable with --accept-capabilities, then retry.
# What gets reviewed is the plugin's declared capability surface, not its
# executable bytes: for memory-anolisa that is the 4 `contracts.tools` entries
# in openclaw.plugin.json, which OpenClaw folds into `declared.tools`
# (buildPluginCapabilitySummary()). The conversation hooks and the MCP client
# are registered at runtime from dist/index.js (src/index.ts) and are not part
# of the declared manifest surface. Consent itself is not conditional on that
# surface being non-empty — a local-path install asks either way — so do not
# "fix" this by editing openclaw.plugin.json.
#
# The flag is gated on the help probe above because OpenClaw <= 2026.8.2 does
# not register it (first documented in 2026.9.1) and commander would exit 1 on
# the unknown option, turning "installs fine" into "always fails" on hosts this
# adapter still claims to support (manifest.json compatibleVersions ">=5.0.0").
# Set AGENT_MEMORY_ACCEPT_CAPABILITIES=0 to withhold the flag where operator
# policy forbids non-interactive capability acceptance.
ACCEPT_CAPABILITIES_FLAG="--accept-capabilities"
ACCEPT_CAPABILITIES_ADVERTISED=0
ACCEPT_CAPABILITIES_WITHHELD=0
if install_help_lists_flag "$ACCEPT_CAPABILITIES_FLAG" "$INSTALL_HELP"; then
    ACCEPT_CAPABILITIES_ADVERTISED=1
fi

if [ "${AGENT_MEMORY_ACCEPT_CAPABILITIES:-1}" = "0" ]; then
    ACCEPT_CAPABILITIES_WITHHELD=1
    echo "[${COMPONENT}] AGENT_MEMORY_ACCEPT_CAPABILITIES=0: withholding ${ACCEPT_CAPABILITIES_FLAG}; OpenClaw >= 2026.9.1 refuses a non-interactive install that requires capability consent." >&2
elif [ "$ACCEPT_CAPABILITIES_ADVERTISED" = "1" ]; then
    INSTALL_ARGS+=("$ACCEPT_CAPABILITIES_FLAG")
else
    echo "[${COMPONENT}] '${OPENCLAW_BIN} plugins install' does not advertise ${ACCEPT_CAPABILITIES_FLAG} (OpenClaw < 2026.9.1); installing without it." >&2
fi

if ! openclaw_cli plugins install "$PLUGIN_DIR" "${INSTALL_ARGS[@]}"; then
    echo "[${COMPONENT}] openclaw CLI install failed: ${OPENCLAW_BIN} plugins install ${PLUGIN_DIR} ${INSTALL_ARGS[*]}" >&2
    if [ "$ACCEPT_CAPABILITIES_WITHHELD" = "1" ] && [ "$ACCEPT_CAPABILITIES_ADVERTISED" = "1" ]; then
        echo "[${COMPONENT}]   ${ACCEPT_CAPABILITIES_FLAG} was withheld by AGENT_MEMORY_ACCEPT_CAPABILITIES=0." >&2
        echo "[${COMPONENT}]   If the error above is 'requires capability consent', re-run with AGENT_MEMORY_ACCEPT_CAPABILITIES unset (default: accept)," >&2
        echo "[${COMPONENT}]   or consent yourself: ${OPENCLAW_BIN} plugins install ${PLUGIN_DIR} --force ${ACCEPT_CAPABILITIES_FLAG}" >&2
    elif [ "$ACCEPT_CAPABILITIES_ADVERTISED" = "0" ]; then
        echo "[${COMPONENT}]   This openclaw predates ${ACCEPT_CAPABILITIES_FLAG} (< 2026.9.1), so it cannot offer capability consent." >&2
        echo "[${COMPONENT}]   If the error above is 'requires capability consent', upgrade OpenClaw to >= 2026.9.1 and re-run." >&2
    fi
    echo "[${COMPONENT}]   Otherwise inspect the ${OPENCLAW_BIN} output above; the adapter itself requires OpenClaw >= 5.0.0 (manifest.json compatibleVersions)." >&2
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
