#!/usr/bin/env bash
# detect.sh — Inspect agent-sec-core OpenClaw integration. Read-only.
#
# Reports OpenClaw CLI, agent-sec plugin, sec-core runtime binary, sec-core
# skills, and adapter resource availability. Exits 0 when the plugin and all
# expected skills/binaries are in place, non-zero otherwise.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
AGENT="${ANOLISA_TARGET:-openclaw}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
INSTALL_MODE="${ANOLISA_INSTALL_MODE:-user}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
SEC_CORE_BIN_DIR="${SEC_CORE_BIN_DIR:-$HOME/.local/bin}"
SEC_CORE_OPENCLAW_PLUGIN_DIR="${SEC_CORE_OPENCLAW_PLUGIN_DIR:-}"
export PATH="$SEC_CORE_BIN_DIR:$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"

SEC_CORE_SKILLS=(code-scanner prompt-scanner skill-ledger)
PLUGIN_ID="agent-sec"

line()  { printf '[%s] %s\n' "$COMPONENT" "$*"; }
field() { printf '[%s]   %-26s %s\n' "$COMPONENT" "$1" "$2"; }

PREREQ_MISSING=()
INSTALL_MISSING=()
note_prereq_missing() { PREREQ_MISSING+=("$1"); }
note_install_missing() { INSTALL_MISSING+=("$1"); }

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

line "${AGENT} detect"
if [ -n "$OPENCLAW_BIN" ] && [ -x "$OPENCLAW_BIN" ]; then
    field "openclaw CLI" "present (${OPENCLAW_BIN})"
else
    field "openclaw CLI" "missing"
    note_prereq_missing "openclaw CLI"
fi

# agent-sec plugin — check OpenClaw plugin listing first, then on-disk extension.
plugin_state="missing"
plugin_detail="$PLUGIN_ID"
if [ -n "$OPENCLAW_BIN" ] && [ -x "$OPENCLAW_BIN" ]; then
    plugins_json="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins list --json 2>/dev/null || true)"
    plugins_txt="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins list 2>/dev/null || true)"
    if grep -qE "\"id\"[[:space:]]*:[[:space:]]*\"${PLUGIN_ID}\"" <<<"$plugins_json" \
       || grep -qE "(^|[[:space:]])${PLUGIN_ID}([[:space:]]|$)" <<<"$plugins_txt"; then
        plugin_state="listed"
        plugin_detail="$PLUGIN_ID (openclaw plugins list)"
    fi
fi
if [ "$plugin_state" = "missing" ] && [ -d "${OPENCLAW_STATE_DIR%/}/extensions/${PLUGIN_ID}" ]; then
    plugin_state="installed"
    plugin_detail="${OPENCLAW_STATE_DIR%/}/extensions/${PLUGIN_ID}"
fi
if [ "$plugin_state" != "missing" ]; then
    field "${PLUGIN_ID} plugin" "${plugin_state} (${plugin_detail})"
else
    field "${PLUGIN_ID} plugin" "missing"
    note_install_missing "${PLUGIN_ID} plugin"
fi

# Runtime binary — sec-core ships agent-sec-cli under SEC_CORE_BIN_DIR / PATH.
runtime_bin="$(command -v agent-sec-cli 2>/dev/null || true)"
if [ -n "$runtime_bin" ]; then
    field "agent-sec-cli" "present (${runtime_bin})"
else
    field "agent-sec-cli" "missing"
    note_prereq_missing "agent-sec-cli"
fi

# Adapter resources — prefer directly installable artifacts only:
# source-build stage > user install > system install.  The development source
# plugin is intentionally not used because it can contain node_modules from
# local builds and break OpenClaw's peerDependency linking.
plugin_sources=()
[ -n "$TARGET_DIR" ] && plugin_sources+=(
    "$TARGET_DIR/build/openclaw-plugin"
    "$TARGET_DIR/lib/anolisa/sec-core/openclaw-plugin"
)
plugin_sources+=(
    "$SEC_CORE_OPENCLAW_PLUGIN_DIR"
    "$HOME/.local/lib/anolisa/sec-core/openclaw-plugin"
    "/usr/local/lib/anolisa/sec-core/openclaw-plugin"
    "/usr/lib/anolisa/sec-core/openclaw-plugin"
    "/opt/agent-sec/openclaw-plugin"
)

plugin_resource="-"
for cand in "${plugin_sources[@]}"; do
    if [ -n "$cand" ] && [ -d "$cand" ] && [ -x "$cand/scripts/deploy.sh" ]; then
        plugin_resource="$cand"
        break
    fi
done
field "plugin resource" "$plugin_resource"
if [ "$plugin_resource" = "-" ]; then
    note_prereq_missing "plugin resource"
fi

# sec-core skills — list each explicitly so users see exact install paths.
missing_skills=()
for s in "${SEC_CORE_SKILLS[@]}"; do
    sf="${OPENCLAW_SKILLS_DIR%/}/$s/SKILL.md"
    if [ -f "$sf" ]; then
        field "$s/SKILL.md" "present (${sf})"
    else
        field "$s/SKILL.md" "missing (${sf})"
        missing_skills+=("$s")
    fi
done
if [ ${#missing_skills[@]} -gt 0 ]; then
    note_install_missing "skills"
fi

if [ ${#PREREQ_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: missing prerequisites (${PREREQ_MISSING[*]})"
    exit 2
fi
if [ ${#INSTALL_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: not installed (ready to install)"
    exit 1
fi
line "${AGENT}: ready"
exit 0
