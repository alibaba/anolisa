#!/usr/bin/env bash
# detect.sh — Inspect agent-sec-core Hermes integration. Read-only.
#
# Reports hermes CLI, Hermes home, agent-sec-cli, plugin resource, and the
# installed plugin under $HERMES_HOME/plugins/agent-sec-core-hermes-plugin.
# Exits 0 when installed/ready, 1 when not installed but installable, and 2
# when prerequisites are missing.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
AGENT="${ANOLISA_TARGET:-hermes}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
MANIFEST_PATH="${ANOLISA_MANIFEST_PATH:-}"
INSTALL_MODE="${ANOLISA_INSTALL_MODE:-user}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_BIN="${HERMES_BIN:-}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
SEC_CORE_BIN_DIR="${SEC_CORE_BIN_DIR:-$HOME/.local/bin}"
SEC_CORE_HERMES_PLUGIN_DIR="${SEC_CORE_HERMES_PLUGIN_DIR:-}"
export PATH="$SEC_CORE_BIN_DIR:$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"

COMMON_HELPER="${ADAPTER_DIR}/common/manifest.sh"
[ -f "$COMMON_HELPER" ] || {
    echo "[${COMPONENT}] missing adapter common helper: $COMMON_HELPER" >&2
    exit 1
}
. "$COMMON_HELPER"

line()  { printf '[%s] %s\n' "$COMPONENT" "$*"; }
field() { printf '[%s]   %-26s %s\n' "$COMPONENT" "$1" "$2"; }

PREREQ_MISSING=()
INSTALL_MISSING=()
note_prereq_missing() { PREREQ_MISSING+=("$1"); }
note_install_missing() { INSTALL_MISSING+=("$1"); }

if [ -z "$HERMES_BIN" ]; then
    HERMES_BIN="$(command -v hermes 2>/dev/null || true)"
fi
PLUGIN_ID="$(sec_core_manifest_plugin_id "${ANOLISA_TARGET:-hermes}" "$MANIFEST_PATH")"

line "${AGENT} detect"
if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
    field "hermes CLI" "present (${HERMES_BIN})"
else
    field "hermes CLI" "missing"
    note_prereq_missing "hermes CLI"
fi

if [ -d "$HERMES_HOME" ]; then
    field "hermes home" "present (${HERMES_HOME})"
else
    field "hermes home" "not installed (${HERMES_HOME})"
    note_install_missing "hermes home"
fi

# Runtime binary — sec-core ships agent-sec-cli under SEC_CORE_BIN_DIR / PATH.
runtime_bin="$(command -v agent-sec-cli 2>/dev/null || true)"
if [ -n "$runtime_bin" ]; then
    field "agent-sec-cli" "present (${runtime_bin})"
else
    field "agent-sec-cli" "missing"
    note_prereq_missing "agent-sec-cli"
fi

# Plugin source resource — required to (re-)install.
plugin_sources=()
[ -n "$TARGET_DIR" ] && plugin_sources+=(
    "$TARGET_DIR/build/hermes-plugin"
    "$TARGET_DIR/lib/anolisa/sec-core/hermes-plugin"
)
plugin_sources+=(
    "$SEC_CORE_HERMES_PLUGIN_DIR"
    "$HOME/.local/lib/anolisa/sec-core/hermes-plugin"
    "/usr/local/lib/anolisa/sec-core/hermes-plugin"
    "/usr/lib/anolisa/sec-core/hermes-plugin"
    "/opt/agent-sec/hermes-plugin"
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

# Installed plugin under HERMES_HOME/plugins.
plugin_dst="${HERMES_HOME%/}/plugins/${PLUGIN_ID}"
if [ -d "$plugin_dst" ] || [ -L "$plugin_dst" ]; then
    field "${PLUGIN_ID}" "installed (${plugin_dst})"
else
    field "${PLUGIN_ID}" "missing (${plugin_dst})"
    note_install_missing "${PLUGIN_ID} plugin"
fi

# sec-core skills under Hermes skills dir.
SEC_CORE_SKILLS=()
while IFS= read -r skill_name; do
    [ -n "$skill_name" ] && SEC_CORE_SKILLS+=("$skill_name")
done < <(sec_core_manifest_skills "${ANOLISA_TARGET:-hermes}" "$MANIFEST_PATH")
missing_skills=()
for s in "${SEC_CORE_SKILLS[@]}"; do
    sf="${HERMES_SKILLS_DIR%/}/$s/SKILL.md"
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
