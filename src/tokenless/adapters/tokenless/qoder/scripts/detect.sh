#!/usr/bin/env bash
# detect.sh — Check if Qoder CLI is installed and compatible.
# Exit 0 = ready to install, non-0 = not available.
#
# Mirrors install.sh's binary search so detect and install agree on
# "is Qoder CLI installed". A bare ~/.qoder/ directory (e.g. created by
# an unrelated tool) is not enough — qodercli itself must be present.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

# Pick the highest versioned qodercli-X.Y.Z; sort -V handles semver
# ordering (10 > 9). Empty when no match.
versioned_glob="$HOME/.qoder/bin/qodercli/qodercli-${ANOLISA_QODER_VERSION:-*}"
# shellcheck disable=SC2086  # intentional glob expansion
latest_versioned="$(ls -d $versioned_glob 2>/dev/null | sort -V | tail -1 || true)"

for candidate in "$latest_versioned" \
                 "$HOME/.qoder/bin/qodercli/qodercli" \
                 "qodercli"; do
    [ -z "$candidate" ] && continue
    if [ -x "$candidate" ] || command -v "$candidate" &>/dev/null; then
        echo "[${COMPONENT}] ${AGENT}: detected qodercli at ${candidate}"
        exit 0
    fi
done

echo "[${COMPONENT}] ${AGENT}: qodercli not found in standard locations" >&2
exit 1
