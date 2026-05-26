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

# 1. Check openclaw availability. Dry-run should not require the CLI.
if [ "$DRY_RUN" != "1" ] && ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "ERROR: openclaw is not installed, please install openclaw first"
    exit 1
fi

# 2. Try plugin install (preferred).
if PLUGIN_SRC=$(find_plugin_src openclaw); then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins install $PLUGIN_SRC --force"
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins enable ws-ckpt"
        exit 0
    fi
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install "$PLUGIN_SRC" --force
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
