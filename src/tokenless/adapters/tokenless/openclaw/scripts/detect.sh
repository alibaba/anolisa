#!/usr/bin/env bash
# detect.sh — Check if OpenClaw is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

if [ -d "$HOME/.openclaw" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected ~/.openclaw config directory"
    exit 0
fi

if command -v openclaw &>/dev/null; then
    echo "[${COMPONENT}] ${AGENT}: detected openclaw binary"
    exit 0
fi

echo "[${COMPONENT}] ${AGENT}: not detected (neither ~/.openclaw nor openclaw binary found)" >&2
exit 1