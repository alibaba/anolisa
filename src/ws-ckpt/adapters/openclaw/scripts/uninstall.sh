#!/usr/bin/env bash
# Remove ws-ckpt resources from OpenClaw.
#
# TODO(adapter-manifest): ws-ckpt currently has empty actions in its manifest.
# Keep install/uninstall here until a common adapter runner owns this flow.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-ws-ckpt}"
INSTALL_MODE="${ANOLISA_INSTALL_MODE:-user}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-$HOME/.openclaw/skills}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"

log() {
    echo "[${COMPONENT}] $*"
}

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    else
        sudo "$@"
    fi
}

systemd_is_available() {
    command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]
}

log "remove skill ws-ckpt from ${OPENCLAW_SKILLS_DIR}"
if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: rm -rf ${OPENCLAW_SKILLS_DIR}/ws-ckpt"
else
    rm -rf "$OPENCLAW_SKILLS_DIR/ws-ckpt"
fi

if [ "$INSTALL_MODE" = "system" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: systemctl stop ws-ckpt.service"
        echo "DRY-RUN: systemctl disable ws-ckpt.service"
        echo "DRY-RUN: rm -f /usr/lib/systemd/system/ws-ckpt.service"
        echo "DRY-RUN: systemctl daemon-reload"
    else
        if systemd_is_available; then
            as_root systemctl stop ws-ckpt.service 2>/dev/null || true
            as_root systemctl disable ws-ckpt.service 2>/dev/null || true
        fi
        as_root rm -f /usr/lib/systemd/system/ws-ckpt.service
        if systemd_is_available; then
            as_root systemctl daemon-reload || true
        fi
    fi
else
    log "user mode skips systemd service removal"
fi

log "OpenClaw resources removed"
