#!/usr/bin/env bash
# detect.sh — Check if OpenClaw is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-agent-memory}"
# Honour OPENCLAW_HOME consistently across detect / install / uninstall
# so a user with a non-default location isn't told "not detected" while
# install / uninstall still target ~/.openclaw.
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"

if [ -d "$OPENCLAW_HOME" ]; then
    echo "[${COMPONENT}] ${AGENT}: detected ${OPENCLAW_HOME} config directory"
    exit 0
fi

if command -v openclaw &>/dev/null; then
    echo "[${COMPONENT}] ${AGENT}: detected openclaw binary"
    exit 0
fi

echo "[${COMPONENT}] ${AGENT}: not detected (neither ${OPENCLAW_HOME} nor openclaw binary found)" >&2
exit 1