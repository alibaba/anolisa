#!/usr/bin/env bash
# uninstall.sh — Remove tokenless plugin via OpenClaw official CLI.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

echo "[${COMPONENT}] Removing ${AGENT} plugin..."

if ! command -v openclaw &>/dev/null; then
    echo "[${COMPONENT}] openclaw CLI not found — removing plugin files manually."
    rm -rf "$HOME/.openclaw/plugins/tokenless-openclaw" 2>/dev/null || true
    rm -rf "$HOME/.openclaw/extensions/tokenless-openclaw" 2>/dev/null || true
    echo "[${COMPONENT}] Plugin files removed. Manually clean up openclaw.json if needed."
    exit 0
fi

# Use openclaw CLI for proper removal (handles file cleanup + config update)
openclaw plugins uninstall tokenless-openclaw --force || true

echo "[${COMPONENT}] ${AGENT} plugin removed via openclaw CLI."