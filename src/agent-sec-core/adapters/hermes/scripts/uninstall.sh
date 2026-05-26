#!/usr/bin/env bash
# Remove agent-sec resources from Hermes.
#
# Uses `hermes plugins` CLI when available; otherwise falls back to deleting
# the plugin directory under HERMES_HOME/plugins. Also removes installed
# sec-core skills under HERMES_SKILLS_DIR.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_BIN="${HERMES_BIN:-}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
SEC_CORE_SKILLS=(code-scanner prompt-scanner skill-ledger)
PLUGIN_ID="agent-sec-core-hermes-plugin"
export PATH="$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"

if [ -z "$HERMES_BIN" ]; then
    HERMES_BIN="$(command -v hermes 2>/dev/null || true)"
fi

log() {
    echo "[${COMPONENT}] $*"
}

if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: $HERMES_BIN plugins disable ${PLUGIN_ID}"
        echo "DRY-RUN: $HERMES_BIN plugins remove ${PLUGIN_ID}"
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

for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    log "remove skill ${skill_name} from ${HERMES_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${HERMES_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$HERMES_SKILLS_DIR/$skill_name"
    fi
done

log "Hermes resources removed"
