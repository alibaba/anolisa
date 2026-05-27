#!/usr/bin/env bash
# Install agent-sec resources into OpenClaw through sec-core's own deployer.
#
# TODO(adapter-manifest): this is only a thin adapter wrapper for build-all.
# Do not duplicate or replace openclaw-plugin/scripts/deploy.sh; that script is
# the sec-core-owned OpenClaw plugin registration entrypoint. This wrapper only
# locates staged/source resources, delegates plugin install to deploy.sh, and
# keeps OpenClaw skill syncing outside build-all.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-sec-core}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
SEC_CORE_OPENCLAW_PLUGIN_DIR="${SEC_CORE_OPENCLAW_PLUGIN_DIR:-}"
SEC_CORE_BIN_DIR="${SEC_CORE_BIN_DIR:-$HOME/.local/bin}"
export PATH="$SEC_CORE_BIN_DIR:$HOME/.local/bin:/usr/local/bin:$PATH"
SEC_CORE_SKILLS=(code-scanner prompt-scanner skill-ledger)

log() {
    echo "[${COMPONENT}] $*"
}

find_plugin_dir() {
    local candidate
    local candidates=()
    if [ -n "$TARGET_DIR" ]; then
        candidates+=(
            "$TARGET_DIR/build/openclaw-plugin"
            "$TARGET_DIR/lib/anolisa/sec-core/openclaw-plugin"
        )
    fi
    candidates+=(
        "$SEC_CORE_OPENCLAW_PLUGIN_DIR" \
        "$HOME/.local/lib/anolisa/sec-core/openclaw-plugin" \
        "/usr/local/lib/anolisa/sec-core/openclaw-plugin" \
        "/usr/lib/anolisa/sec-core/openclaw-plugin" \
        "/opt/agent-sec/openclaw-plugin"
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
    echo "[${COMPONENT}] OpenClaw plugin resource not found" >&2
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
    echo "DRY-RUN: ${deploy_script} ${plugin_dir}"
else
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$deploy_script" "$plugin_dir"
fi

if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: mkdir -p ${OPENCLAW_SKILLS_DIR}"
else
    mkdir -p "$OPENCLAW_SKILLS_DIR"
fi
for skill_name in "${SEC_CORE_SKILLS[@]}"; do
    skill_dir="$(find_skill_dir "$skill_name")" || {
        echo "[${COMPONENT}] skill resource not found: ${skill_name}" >&2
        exit 1
    }
    log "install skill ${skill_name} -> ${OPENCLAW_SKILLS_DIR}/${skill_name}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: mkdir -p ${OPENCLAW_SKILLS_DIR}/${skill_name}"
        echo "DRY-RUN: cp -rp ${skill_dir}/. ${OPENCLAW_SKILLS_DIR}/${skill_name}/"
    else
        rm -rf "$OPENCLAW_SKILLS_DIR/$skill_name"
        mkdir -p "$OPENCLAW_SKILLS_DIR/$skill_name"
        cp -rp "$skill_dir/." "$OPENCLAW_SKILLS_DIR/$skill_name/"
    fi
done

log "OpenClaw resources installed"
