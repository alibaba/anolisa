#!/bin/bash

PLUGIN_DST="${HOME}/.hermes/plugins/ws-ckpt"
SKILL_DST="${HOME}/.hermes/skills/ws-ckpt"

# 1. Remove plugin symlink
if [ -L "$PLUGIN_DST" ] || [ -d "$PLUGIN_DST" ]; then
    rm -rf "$PLUGIN_DST"
    echo "plugin removed: $PLUGIN_DST"
fi

# 2. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed: $SKILL_DST"
fi