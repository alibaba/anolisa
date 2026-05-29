#!/usr/bin/env bash
# Install agent-sec resources into Hermes through sec-core's own deployer.
#
# This is a thin adapter wrapper for the anolisa adapter runner. It locates
# the staged/installed hermes-plugin resource and delegates the actual install
# to hermes-plugin/scripts/deploy.sh, which is the sec-core-owned Hermes plugin
# registration entrypoint. Skill syncing into HERMES_SKILLS_DIR is handled here.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
MANIFEST_PATH="${ANOLISA_MANIFEST_PATH:-}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
SEC_CORE_HERMES_PLUGIN_DIR="${SEC_CORE_HERMES_PLUGIN_DIR:-}"
SEC_CORE_BIN_DIR="${SEC_CORE_BIN_DIR:-$HOME/.local/bin}"
export PATH="$SEC_CORE_BIN_DIR:$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"
COMMON_HELPER="${ADAPTER_DIR}/common/manifest.sh"
[ -f "$COMMON_HELPER" ] || {
    echo "[${COMPONENT}] missing adapter common helper: $COMMON_HELPER" >&2
    exit 1
}
. "$COMMON_HELPER"

log() {
    echo "[${COMPONENT}] $*"
}

find_plugin_dir() {
    local candidate
    local candidates=()
    if [ -n "$TARGET_DIR" ]; then
        candidates+=(
            "$TARGET_DIR/build/hermes-plugin"
            "$TARGET_DIR/lib/anolisa/sec-core/hermes-plugin"
        )
    fi
    candidates+=(
        "$SEC_CORE_HERMES_PLUGIN_DIR" \
        "$HOME/.local/lib/anolisa/sec-core/hermes-plugin" \
        "/usr/local/lib/anolisa/sec-core/hermes-plugin" \
        "/usr/lib/anolisa/sec-core/hermes-plugin" \
        "/opt/agent-sec/hermes-plugin"
    )
    for candidate in "${candidates[@]}"; do
        if [ -n "$candidate" ] && [ -d "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

find_skill_dir() {
    local skill_name="$1" candidate found
    local candidates=()
    if [ -n "$TARGET_DIR" ]; then
        candidates+=(
            "$TARGET_DIR/build/skills"
            "$TARGET_DIR/share/anolisa/skills"
        )
    fi
    if [ -n "$PROJECT_ROOT" ]; then
        candidates+=("$PROJECT_ROOT/src/agent-sec-core/skills")
    fi
    candidates+=(
        "$HOME/.copilot-shell/skills" \
        "/usr/share/anolisa/skills"
    )
    for candidate in "${candidates[@]}"; do
        [ -n "$candidate" ] && [ -d "$candidate" ] || continue
        if [ -f "$candidate/$skill_name/SKILL.md" ]; then
            echo "$candidate/$skill_name"
            return 0
        fi
        found="$(find "$candidate" -path "*/$skill_name/SKILL.md" -type f -print -quit)"
        if [ -n "$found" ]; then
            dirname "$found"
            return 0
        fi
    done
    return 1
}

plugin_dir="$(find_plugin_dir)" || {
    echo "[${COMPONENT}] Hermes plugin resource not found" >&2
    echo "[${COMPONENT}] Searched source-build stage, user install, and system install paths." >&2
    echo "[${COMPONENT}] Build/install sec-core first; the development source plugin is not installed directly." >&2
    exit 1
}
deploy_script="$plugin_dir/scripts/deploy.sh"
[ -x "$deploy_script" ] || {
    echo "[${COMPONENT}] missing executable deploy script: $deploy_script" >&2
    exit 1
}

if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: HERMES_HOME=${HERMES_HOME%/} ${deploy_script} ${plugin_dir}"
else
    HERMES_HOME="${HERMES_HOME%/}" "$deploy_script" "$plugin_dir"
fi

SEC_CORE_SKILLS=()
while IFS= read -r skill_name; do
    [ -n "$skill_name" ] && SEC_CORE_SKILLS+=("$skill_name")
done < <(sec_core_manifest_skills "${ANOLISA_TARGET:-hermes}" "$MANIFEST_PATH")

if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: mkdir -p ${HERMES_SKILLS_DIR}"
else
    mkdir -p "$HERMES_SKILLS_DIR"
fi
for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    skill_dir="$(find_skill_dir "$skill_name")" || {
        echo "[${COMPONENT}] skill resource not found: ${skill_name}" >&2
        exit 1
    }
    log "install skill ${skill_name} -> ${HERMES_SKILLS_DIR}/${skill_name}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: mkdir -p ${HERMES_SKILLS_DIR}/${skill_name}"
        echo "DRY-RUN: cp -rp ${skill_dir}/. ${HERMES_SKILLS_DIR}/${skill_name}/"
    else
        rm -rf "$HERMES_SKILLS_DIR/$skill_name"
        mkdir -p "$HERMES_SKILLS_DIR/$skill_name"
        cp -rp "$skill_dir/." "$HERMES_SKILLS_DIR/$skill_name/"
    fi
done

log "Hermes resources installed"
