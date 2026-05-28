#!/bin/bash

set -euo pipefail

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
SKILL_DST="${OPENCLAW_STATE_DIR%/}/skills/ws-ckpt"
PLUGIN_ID="ws-ckpt"

# 1. Uninstall plugin if openclaw is available
if command -v "$OPENCLAW_BIN" &>/dev/null; then
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins uninstall "$PLUGIN_ID" --force 2>/dev/null || true
fi
rm -rf "${OPENCLAW_STATE_DIR%/}/extensions/ws-ckpt/"
echo "openclaw ws-ckpt plugin uninstalled"

# 2. Remove ws-ckpt-* entries from tools.alsoAllow in openclaw.json
OPENCLAW_CONFIG="${OPENCLAW_CONFIG_PATH:-${OPENCLAW_STATE_DIR}/openclaw.json}"
if [ -f "$OPENCLAW_CONFIG" ]; then
    node -e '
var fs = require("fs");
var configPath = process.argv[1];
var config;
try { config = JSON.parse(fs.readFileSync(configPath, "utf8")); }
catch(e) { process.exit(0); }
var tools = config.tools;
if (!tools || typeof tools !== "object") process.exit(0);
var alsoAllow = tools.alsoAllow;
if (!Array.isArray(alsoAllow)) process.exit(0);
var filtered = alsoAllow.filter(function(e) { return !(typeof e === "string" && e.startsWith("ws-ckpt-")); });
if (filtered.length === alsoAllow.length) process.exit(0);
tools.alsoAllow = filtered;
fs.writeFileSync(configPath, JSON.stringify(config, null, 2) + "\n");
console.log("removed ws-ckpt entries from tools.alsoAllow in " + configPath);
' "$OPENCLAW_CONFIG" 2>/dev/null || true
fi

# 3. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed from $SKILL_DST"
fi
