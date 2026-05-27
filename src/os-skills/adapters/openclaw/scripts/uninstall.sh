#!/usr/bin/env bash
# Remove os-skills from OpenClaw.
#
# TODO(adapter-manifest): remove this hand-written resource discovery once
# manifest actions/resources are consumed by a shared adapter runner.
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

for skill_name in "${OS_SKILLS[@]}"; do
    log "remove skill ${skill_name} from ${OPENCLAW_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${OPENCLAW_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$OPENCLAW_SKILLS_DIR/$skill_name"
    fi
done
log "OpenClaw skills removed from ${OPENCLAW_SKILLS_DIR}"
