#!/bin/bash

PLUGIN_SRC=/usr/share/anolisa/runtime/ws-ckpt/plugins/openclaw
SKILL_SRC=/usr/share/anolisa/runtime/skills/ws-ckpt
SKILL_DST="${HOME}/.openclaw/skills/ws-ckpt"

# 1. Check openclaw availability
if ! command -v openclaw &>/dev/null; then
    echo "ERROR: openclaw is not installed, please install openclaw first"
    exit 1
fi

# 2. Try plugin install (preferred)
if [ -d "$PLUGIN_SRC" ]; then
    openclaw plugins install "$PLUGIN_SRC"
    openclaw plugins enable ws-ckpt 2>/dev/null || true
    echo "openclaw ws-ckpt plugin installed and enabled successfully"
    exit 0
fi

# 3. Fallback to skill install
if [ -d "$SKILL_SRC" ]; then
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST"
else
    echo "ERROR: neither $PLUGIN_SRC nor $SKILL_SRC exists, please install ws-ckpt via RPM or make install first"
    exit 1
fi