#!/bin/bash

PLUGIN_SRC=/usr/share/anolisa/runtime/ws-ckpt/plugins/hermes
PLUGIN_DST="${HOME}/.hermes/plugins/ws-ckpt"
SKILL_SRC=/usr/share/anolisa/runtime/skills/ws-ckpt
SKILL_DST="${HOME}/.hermes/skills/ws-ckpt"

# 1. Check plugin source
if [ -d "$PLUGIN_SRC" ]; then
    # Plugin available, install via symlink
    mkdir -p "$(dirname "$PLUGIN_DST")"
    ln -sfn "$PLUGIN_SRC" "$PLUGIN_DST"
    echo "hermes ws-ckpt plugin linked: $PLUGIN_DST -> $PLUGIN_SRC"
    exit 0
fi

# 2. Fallback to skill install
if [ -d "$SKILL_SRC" ]; then
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST"
else
    echo "ERROR: neither $PLUGIN_SRC nor $SKILL_SRC exists, please install ws-ckpt via RPM or make install first"
    exit 1
fi