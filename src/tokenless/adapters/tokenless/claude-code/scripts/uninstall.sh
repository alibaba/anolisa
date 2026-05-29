#!/usr/bin/env bash
# uninstall.sh — Remove the tokenless plugin and the anolisa marketplace via
# the official `claude plugin` CLI. Falls back to manual cleanup of the
# settings.json + plugin cache when claude is unavailable (e.g. RPM %preun
# running after the user uninstalled claude themselves).
set -euo pipefail

AGENT="${ANOLISA_TARGET:-claude-code}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"

MARKETPLACE_NAME="anolisa"
PLUGIN_ID="tokenless@${MARKETPLACE_NAME}"

CLAUDE_BIN="${CLAUDE_BIN:-claude}"
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"

echo "[${COMPONENT}] Uninstalling ${AGENT} plugin..."

if command -v "$CLAUDE_BIN" &>/dev/null; then
    "$CLAUDE_BIN" plugin uninstall "$PLUGIN_ID" 2>&1 || true
    "$CLAUDE_BIN" plugin marketplace remove "$MARKETPLACE_NAME" 2>&1 || true
    echo "[${COMPONENT}] ${AGENT} plugin removed via claude CLI."
    exit 0
fi

echo "[${COMPONENT}] claude CLI not found — falling back to manual cleanup."
SETTINGS="$HOME/.claude/settings.json"
if command -v jq &>/dev/null && [ -f "$SETTINGS" ]; then
    # del(.[$key]) silently no-ops on non-object values. Verify shape up-front
    # so a schema change in a future Claude Code release surfaces as a clear
    # warning instead of a silent "cleaned" message that left residue behind.
    EP_TYPE=$(jq -r '.enabledPlugins | type' "$SETTINGS" 2>/dev/null || echo "missing")
    MK_TYPE=$(jq -r '.extraKnownMarketplaces | type' "$SETTINGS" 2>/dev/null || echo "missing")
    if [ "$EP_TYPE" != "object" ] && [ "$EP_TYPE" != "null" ] && [ "$EP_TYPE" != "missing" ]; then
        echo "[${COMPONENT}] WARN: .enabledPlugins is ${EP_TYPE}, expected object — skipping jq cleanup." >&2
        echo "[${COMPONENT}] WARN: remove '${PLUGIN_ID}' from $SETTINGS manually." >&2
    elif [ "$MK_TYPE" != "object" ] && [ "$MK_TYPE" != "null" ] && [ "$MK_TYPE" != "missing" ]; then
        echo "[${COMPONENT}] WARN: .extraKnownMarketplaces is ${MK_TYPE}, expected object — skipping jq cleanup." >&2
        echo "[${COMPONENT}] WARN: remove '${MARKETPLACE_NAME}' from $SETTINGS manually." >&2
    else
        tmp="$(mktemp "${SETTINGS}.XXXXXX")"
        jq --arg id "$PLUGIN_ID" --arg mkt "$MARKETPLACE_NAME" '
            if .enabledPlugins then .enabledPlugins |= del(.[$id]) else . end
            | if .extraKnownMarketplaces then .extraKnownMarketplaces |= del(.[$mkt]) else . end
        ' "$SETTINGS" > "$tmp" && mv "$tmp" "$SETTINGS"
        echo "[${COMPONENT}] cleaned $SETTINGS"
    fi
fi
rm -rf "$HOME/.claude/plugins/cache/${MARKETPLACE_NAME}" 2>/dev/null || true
echo "[${COMPONENT}] manual cleanup complete."
