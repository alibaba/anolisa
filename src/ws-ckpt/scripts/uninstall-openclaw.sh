#!/bin/bash

set -euo pipefail

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
SKILL_DST="${OPENCLAW_HOME%/}/skills/ws-ckpt"
PLUGIN_ID="ws-ckpt"

# 1. Uninstall plugin if openclaw is available
if command -v "$OPENCLAW_BIN" &>/dev/null; then
    OPENCLAW_HOME="${OPENCLAW_HOME%/}" "$OPENCLAW_BIN" plugins uninstall "$PLUGIN_ID" --force 2>/dev/null || true
fi
rm -rf "${OPENCLAW_HOME%/}/extensions/ws-ckpt/"
echo "openclaw ws-ckpt plugin uninstalled"

# 2. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed from $SKILL_DST"
fi
