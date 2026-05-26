#!/usr/bin/env bash
# Remove os-skills from Hermes. Only removes the known skill list.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-os-skills}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_SKILLS_DIR="${HERMES_SKILLS_DIR:-${HERMES_HOME%/}/skills}"
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
    log "remove skill ${skill_name} from ${HERMES_SKILLS_DIR}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: rm -rf ${HERMES_SKILLS_DIR}/${skill_name}"
    else
        rm -rf "$HERMES_SKILLS_DIR/$skill_name"
    fi
done
log "Hermes skills removed from ${HERMES_SKILLS_DIR}"
