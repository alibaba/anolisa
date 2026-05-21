#!/bin/bash

SKILL_DST="${HOME}/.openclaw/skills/ws-ckpt"
PLUGIN_ID="ws-ckpt"

# 1. Uninstall plugin if openclaw is available
if command -v openclaw &>/dev/null; then
    openclaw plugins uninstall "$PLUGIN_ID" --force 2>/dev/null || true
fi
rm -rf "${HOME}/.openclaw/extensions/ws-ckpt/"
echo "openclaw ws-ckpt plugin uninstalled"

# 2. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed from $SKILL_DST"
fi