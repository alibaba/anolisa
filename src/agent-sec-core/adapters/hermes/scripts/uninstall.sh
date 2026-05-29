#!/usr/bin/env bash
# Remove agent-sec resources from Hermes.
#
# Uses `hermes plugins` CLI when available; otherwise falls back to deleting
# the plugin directory under HERMES_HOME/plugins. Also removes installed
# sec-core skills under HERMES_SKILLS_DIR.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
MANIFEST_PATH="${ANOLISA_MANIFEST_PATH:-}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_BIN="${HERMES_BIN:-}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
export PATH="$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"
COMMON_HELPER="${ADAPTER_DIR}/common/manifest.sh"
[ -f "$COMMON_HELPER" ] || {
    echo "[${COMPONENT}] missing adapter common helper: $COMMON_HELPER" >&2
    exit 1
}
. "$COMMON_HELPER"

if [ -z "$HERMES_BIN" ]; then
    HERMES_BIN="$(command -v hermes 2>/dev/null || true)"
fi

log() {
    echo "[${COMPONENT}] $*"
}

PLUGIN_ID="$(sec_core_manifest_plugin_id "${ANOLISA_TARGET:-hermes}" "$MANIFEST_PATH")"

if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: HERMES_HOME=${HERMES_HOME%/} $HERMES_BIN plugins disable ${PLUGIN_ID}"
        echo "DRY-RUN: HERMES_HOME=${HERMES_HOME%/} $HERMES_BIN plugins remove ${PLUGIN_ID}"
    else
        HERMES_HOME="${HERMES_HOME%/}" "$HERMES_BIN" plugins disable "$PLUGIN_ID" 2>/dev/null || true
        HERMES_HOME="${HERMES_HOME%/}" "$HERMES_BIN" plugins remove "$PLUGIN_ID" 2>/dev/null || true
    fi
else
    log "hermes CLI not found; falling back to filesystem cleanup"
fi

plugin_dst="${HERMES_HOME%/}/plugins/${PLUGIN_ID}"
if [ -d "$plugin_dst" ] || [ -L "$plugin_dst" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${plugin_dst}"
    else
        rm -rf "$plugin_dst"
        log "removed plugin directory ${plugin_dst}"
    fi
fi

SEC_CORE_SKILLS=()
while IFS= read -r skill_name; do
    [ -n "$skill_name" ] && SEC_CORE_SKILLS+=("$skill_name")
done < <(sec_core_manifest_skills "${ANOLISA_TARGET:-hermes}" "$MANIFEST_PATH")

for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    log "remove skill ${skill_name} from ${HERMES_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${HERMES_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$HERMES_SKILLS_DIR/$skill_name"
    fi
done

log "Hermes resources removed"
