#!/bin/bash

set -euo pipefail

# shellcheck source=lib-discover.sh
source "$(dirname "$0")/lib-discover.sh"

SKILL_DST="${HOME}/.openclaw/skills/ws-ckpt"

# 1. Check openclaw availability
if ! command -v openclaw &>/dev/null; then
    echo "ERROR: openclaw is not installed, please install openclaw first"
    exit 1
fi

# 2. Try plugin install (preferred)
if PLUGIN_SRC=$(find_plugin_src openclaw); then
    openclaw plugins install "$PLUGIN_SRC"
    openclaw plugins enable ws-ckpt 2>/dev/null || true
    echo "openclaw ws-ckpt plugin installed and enabled successfully (from $PLUGIN_SRC)"
    exit 0
fi

# 3. Fallback to skill install
if SKILL_SRC=$(find_skill_src); then
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST (from $SKILL_SRC)"
else
    print_search_error
    exit 1
fi
