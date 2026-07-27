#!/usr/bin/env bash
# uninstall.sh — Disable and remove tokenless plugin from Hermes Agent.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-hermes}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
HERMES_BIN="${HERMES_BIN:-}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
export PATH="$HOME/.local/bin:${HERMES_HOME%/}/bin:/usr/local/bin:$PATH"

PLUGIN_DST="${HERMES_HOME%/}/plugins/tokenless"

echo "[${COMPONENT}] Uninstalling ${AGENT} plugin..."

if [ -z "$HERMES_BIN" ]; then
    HERMES_BIN="$(command -v hermes 2>/dev/null || true)"
fi

if [ "$DRY_RUN" = "1" ]; then
    if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
        echo "DRY-RUN: HERMES_HOME=${HERMES_HOME%/} $HERMES_BIN plugins disable tokenless"
        echo "DRY-RUN: HERMES_HOME=${HERMES_HOME%/} $HERMES_BIN plugins remove tokenless"
    else
        echo "DRY-RUN: hermes CLI not found; skip CLI disable/remove"
    fi
    echo "DRY-RUN: rm -f $PLUGIN_DST/__init__.py $PLUGIN_DST/plugin.yaml"
    echo "DRY-RUN: rmdir $PLUGIN_DST || rm -rf $PLUGIN_DST"
    exit 0
fi

# Disable via hermes CLI if available (removes from plugins.enabled in config.yaml)
if [ -n "$HERMES_BIN" ] && [ -x "$HERMES_BIN" ]; then
    HERMES_HOME="${HERMES_HOME%/}" "$HERMES_BIN" plugins disable tokenless || true
    HERMES_HOME="${HERMES_HOME%/}" "$HERMES_BIN" plugins remove tokenless || true
fi

# Always clean up filesystem artifacts (the CLI may leave the symlink behind
# when the plugin wasn't fully registered, e.g. partial install).
if [ -d "$PLUGIN_DST" ] || [ -L "$PLUGIN_DST" ]; then
    rm -f "$PLUGIN_DST/__init__.py" "$PLUGIN_DST/plugin.yaml" 2>/dev/null || true
    rmdir "$PLUGIN_DST" 2>/dev/null || rm -rf "$PLUGIN_DST" 2>/dev/null || true
fi

echo "[${COMPONENT}] ${AGENT} plugin uninstalled."
