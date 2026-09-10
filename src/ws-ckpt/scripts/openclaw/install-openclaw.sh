#!/bin/bash

set -euo pipefail

# shellcheck source=lib-discover.sh
source "$(dirname "$0")/lib-discover.sh"

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
SKILL_DST="${OPENCLAW_STATE_DIR%/}/skills/ws-ckpt"

help_lists_flag() {
    local help_text="$1"
    local flag="$2"

    grep -Eq "(^|[^[:alnum:]_.-])${flag}([^[:alnum:]_.-]|$)" <<<"$help_text"
}

# 1. Check openclaw availability. Dry-run should not require the CLI.
if [ "$DRY_RUN" != "1" ] && ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "ERROR: openclaw is not installed, please install openclaw first"
    exit 1
fi

# 2. Try plugin install (preferred).
if PLUGIN_SRC=$(find_plugin_src openclaw); then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: probe '$OPENCLAW_BIN plugins install --help' for --accept-capabilities"
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins install $PLUGIN_SRC --force [--accept-capabilities when supported]"
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins enable ws-ckpt"
        exit 0
    fi

    install_args=(plugins install "$PLUGIN_SRC" --force)
    install_help=""
    if install_help="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
        "$OPENCLAW_BIN" plugins install --help 2>/dev/null)" \
        && help_lists_flag "$install_help" "--accept-capabilities"; then
        install_args+=(--accept-capabilities)
    fi

    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
        "$OPENCLAW_BIN" "${install_args[@]}"
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins enable ws-ckpt 2>/dev/null || true
    echo "openclaw ws-ckpt plugin installed and enabled successfully (from $PLUGIN_SRC)"
    exit 0
fi

# 3. Fallback to skill install
if SKILL_SRC=$(find_skill_src); then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: mkdir -p $SKILL_DST"
        echo "DRY-RUN: cp -pr $SKILL_SRC/. $SKILL_DST/"
        exit 0
    fi
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST (from $SKILL_SRC)"
else
    print_search_error
    exit 1
fi
