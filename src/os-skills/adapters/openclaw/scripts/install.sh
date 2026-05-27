#!/usr/bin/env bash
# Install os-skills into OpenClaw.
#
# TODO(adapter-manifest): this script is intentionally explicit for now.
# The manifest keeps actions empty until a shared adapter runner can resolve
# resources and invoke install/uninstall in a uniform way.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-os-skills}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_STATE_DIR%/}/skills}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
OS_SKILLS=(
    copaw-usage
    install-claude-code
    install-copaw
    install-hermes
    install-openclaw
    setup-mcp
    aliyun-ecs
    github
    kernel-dev
    sysom-agentsight
    sysom-diagnosis
    clawhub-skill-mng
    cosh-guide
    humanizer
    image-gen
    pdf-reader
    xlsx
    alinux-cve-query
    alinux-admin
    backup-restore
    regex-mastery
    shell-scripting
    storage-resize
    upgrade-alinux-kernel
)

log() {
    echo "[${COMPONENT}] $*"
}

find_skill_dir() {
    local skill_name="$1" root found
    local roots=()
    if [ -n "$TARGET_DIR" ]; then
        roots+=("$TARGET_DIR/share/anolisa/skills")
    fi
    if [ -n "$PROJECT_ROOT" ]; then
        roots+=("$PROJECT_ROOT/src/os-skills")
    fi
    roots+=(
        "$HOME/.copilot-shell/skills" \
        "$HOME/.local/share/anolisa/skills" \
        "/usr/share/anolisa/skills"
    )
    for root in "${roots[@]}"; do
        [ -n "$root" ] && [ -d "$root" ] || continue
        if [ -f "$root/$skill_name/SKILL.md" ]; then
            echo "$root/$skill_name"
            return 0
        fi
        found="$(find "$root" -path "*/$skill_name/SKILL.md" -type f -print -quit)"
        if [ -n "$found" ]; then
            dirname "$found"
            return 0
        fi
    done
    return 1
}

if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: mkdir -p ${OPENCLAW_SKILLS_DIR}"
else
    mkdir -p "$OPENCLAW_SKILLS_DIR"
fi
for skill_name in "${OS_SKILLS[@]}"; do
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
log "OpenClaw skills installed to ${OPENCLAW_SKILLS_DIR}"
