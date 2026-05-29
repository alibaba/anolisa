#!/usr/bin/env bash
# Remove agent-sec resources from OpenClaw.
#
# TODO(adapter-manifest): this is only the build-all adapter boundary. sec-core
# currently owns plugin install through openclaw-plugin/scripts/deploy.sh, while
# uninstall still has to call the OpenClaw CLI directly until sec-core provides
# a matching uninstall action.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
MANIFEST_PATH="${ANOLISA_MANIFEST_PATH:-}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
export PATH="$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"
COMMON_HELPER="${ADAPTER_DIR}/common/manifest.sh"
[ -f "$COMMON_HELPER" ] || {
    echo "[${COMPONENT}] missing adapter common helper: $COMMON_HELPER" >&2
    exit 1
}
. "$COMMON_HELPER"

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

log() {
    echo "[${COMPONENT}] $*"
}

PLUGIN_ID="$(sec_core_manifest_plugin_id "${ANOLISA_TARGET:-openclaw}" "$MANIFEST_PATH")"

if [ -n "$OPENCLAW_BIN" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=${OPENCLAW_STATE_DIR} ${OPENCLAW_BIN} plugins uninstall ${PLUGIN_ID} --force"
    else
        env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins uninstall "$PLUGIN_ID" --force || true
    fi
else
    log "openclaw CLI not found; plugin config cleanup skipped"
fi

SEC_CORE_SKILLS=()
while IFS= read -r skill_name; do
    [ -n "$skill_name" ] && SEC_CORE_SKILLS+=("$skill_name")
done < <(sec_core_manifest_skills "${ANOLISA_TARGET:-openclaw}" "$MANIFEST_PATH")
for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    log "remove skill ${skill_name} from ${OPENCLAW_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${OPENCLAW_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$OPENCLAW_SKILLS_DIR/$skill_name"
    fi
done

log "OpenClaw resources removed"
