#!/usr/bin/env bash
# uninstall.sh — Remove tokenless plugin via OpenClaw official CLI.
#
# TODO(adapter-manifest): keep this explicit script while adapter actions are
# invoked by component Makefile/build-all instead of a shared manifest runner.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
export PATH="$HOME/.local/bin:${OPENCLAW_STATE_DIR%/}/bin:/usr/local/bin:$PATH"

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

echo "[${COMPONENT}] Removing ${AGENT} plugin..."

if [ "$DRY_RUN" = "1" ]; then
    if [ -n "$OPENCLAW_BIN" ]; then
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins uninstall tokenless --force"
    else
        echo "DRY-RUN: openclaw CLI not found; remove plugin files manually"
    fi
    echo "DRY-RUN: rm -rf ${OPENCLAW_STATE_DIR%/}/plugins/tokenless"
    echo "DRY-RUN: rm -rf ${OPENCLAW_STATE_DIR%/}/extensions/tokenless"
    exit 0
fi

if [ -z "$OPENCLAW_BIN" ]; then
    echo "[${COMPONENT}] openclaw CLI not found — removing plugin files manually."
    rm -rf "${OPENCLAW_STATE_DIR%/}/plugins/tokenless" 2>/dev/null || true
    rm -rf "${OPENCLAW_STATE_DIR%/}/extensions/tokenless" 2>/dev/null || true
    echo "[${COMPONENT}] Plugin files removed. Manually clean up openclaw.json if needed."
    exit 0
fi

# Use openclaw CLI for proper removal (handles file cleanup + config update)
env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins uninstall tokenless --force || true

echo "[${COMPONENT}] ${AGENT} plugin removed via openclaw CLI."
