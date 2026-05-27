#!/usr/bin/env bash
# Remove agent-sec resources from OpenClaw.
#
# TODO(adapter-manifest): this is only the build-all adapter boundary. sec-core
# currently owns plugin install through openclaw-plugin/scripts/deploy.sh, while
# uninstall still has to call the OpenClaw CLI directly until sec-core provides
# a matching uninstall action.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
SEC_CORE_SKILLS=(code-scanner prompt-scanner skill-ledger)
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
export PATH="$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

log() {
    echo "[${COMPONENT}] $*"
}

if [ -n "$OPENCLAW_BIN" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=${OPENCLAW_STATE_DIR} ${OPENCLAW_BIN} plugins uninstall agent-sec --force"
    else
        env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins uninstall agent-sec --force || true
    fi
else
    log "openclaw CLI not found; plugin config cleanup skipped"
fi

for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    log "remove skill ${skill_name} from ${OPENCLAW_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${OPENCLAW_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$OPENCLAW_SKILLS_DIR/$skill_name"
    fi
done

log "OpenClaw resources removed"
