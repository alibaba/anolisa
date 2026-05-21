#!/usr/bin/env bash
# Install ws-ckpt resources into OpenClaw.
#
# TODO(adapter-manifest): ws-ckpt currently has empty actions in its manifest.
# Keep install/uninstall here until the shared adapter runner can execute
# manifest-declared actions and resource copies directly.
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-ws-ckpt}"
PROJECT_ROOT="${ANOLISA_PROJECT_ROOT:-}"
TARGET_DIR="${ANOLISA_TARGET_DIR:-}"
INSTALL_MODE="${ANOLISA_INSTALL_MODE:-user}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-$HOME/.openclaw/skills}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"

log() {
    echo "[${COMPONENT}] $*"
}

find_skill_dir() {
    local candidate
    local candidates=()
    if [ -n "$TARGET_DIR" ]; then
        candidates+=("$TARGET_DIR/share/anolisa/skills/ws-ckpt")
    fi
    candidates+=(
        "$HOME/.copilot-shell/skills/ws-ckpt" \
        "/usr/share/anolisa/skills/ws-ckpt" \
        "$PROJECT_ROOT/src/ws-ckpt/src/skills/ws-ckpt"
    )
    for candidate in "${candidates[@]}"; do
        if [ -n "$candidate" ] && [ -d "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

find_service_file() {
    local candidate
    local candidates=()
    if [ -n "$TARGET_DIR" ]; then
        candidates+=("$TARGET_DIR/lib/systemd/system/ws-ckpt.service")
    fi
    candidates+=(
        "/usr/lib/systemd/system/ws-ckpt.service" \
        "$PROJECT_ROOT/src/ws-ckpt/src/systemd/ws-ckpt.service"
    )
    for candidate in "${candidates[@]}"; do
        if [ -n "$candidate" ] && [ -f "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
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

copy_tree() {
    local src="$1" dst="$2"
    log "install skill ws-ckpt -> ${dst}"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: mkdir -p ${dst}"
        echo "DRY-RUN: cp -rp ${src}/. ${dst}/"
    else
        rm -rf "$dst"
        mkdir -p "$dst"
        cp -rp "$src/." "$dst/"
    fi
}

install_system_service() {
    local src="$1" dst="/usr/lib/systemd/system/ws-ckpt.service"
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: install -m 0644 ${src} ${dst}"
        echo "DRY-RUN: systemctl daemon-reload"
        echo "DRY-RUN: systemctl enable ws-ckpt.service"
        echo "DRY-RUN: systemctl restart ws-ckpt.service"
        return 0
    fi

    as_root install -d -m 0755 "$(dirname "$dst")"
    as_root install -p -m 0644 "$src" "$dst"
    if systemd_is_available; then
        as_root systemctl daemon-reload || true
        as_root systemctl enable ws-ckpt.service || true
        as_root systemctl restart ws-ckpt.service || true
    else
        log "systemd is not active; installed service but skipped enable/restart"
    fi
}

skill_src="$(find_skill_dir)" || {
    echo "[${COMPONENT}] ws-ckpt skill resource not found" >&2
    exit 1
}
copy_tree "$skill_src" "$OPENCLAW_SKILLS_DIR/ws-ckpt"

if [ "$INSTALL_MODE" = "system" ]; then
    service_src="$(find_service_file)" || {
        echo "[${COMPONENT}] ws-ckpt systemd service resource not found" >&2
        exit 1
    }
    install_system_service "$service_src"
else
    log "user mode skips systemd service; pass --system to manage ws-ckpt.service"
fi

log "OpenClaw resources installed"
