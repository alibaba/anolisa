#!/usr/bin/env bash
# detect.sh — Check if OpenClaw is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
export PATH="$HOME/.local/bin:${OPENCLAW_HOME:-$HOME/.openclaw}/bin:/usr/local/bin:$PATH"

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

if [ -d "$HOME/.openclaw" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected ~/.openclaw config directory"
    exit 0
fi

if [ -n "$OPENCLAW_BIN" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected openclaw binary"
    exit 0
fi

echo "[${COMPONENT}] ${AGENT}: not detected (neither ~/.openclaw nor openclaw binary found)" >&2
exit 1
