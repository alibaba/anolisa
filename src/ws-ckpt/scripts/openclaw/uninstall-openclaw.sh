#!/bin/bash

set -euo pipefail

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
SKILL_DST="${OPENCLAW_STATE_DIR%/}/skills/ws-ckpt"
PLUGIN_ID="ws-ckpt"

if [ "$DRY_RUN" = "1" ]; then
    echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins uninstall $PLUGIN_ID --force"
    echo "DRY-RUN: rm -rf ${OPENCLAW_STATE_DIR%/}/extensions/ws-ckpt/"
    echo "DRY-RUN: remove ws-ckpt tool allow entries via: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN config set tools.alsoAllow <filtered-json> --strict-json"
    echo "DRY-RUN: rm -rf $SKILL_DST"
    exit 0
fi

# 1. Uninstall plugin if openclaw is available
if command -v "$OPENCLAW_BIN" &>/dev/null; then
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins uninstall "$PLUGIN_ID" --force 2>/dev/null || true
fi
rm -rf "${OPENCLAW_STATE_DIR%/}/extensions/ws-ckpt/"
echo "openclaw ws-ckpt plugin uninstalled"

# 2. Remove ws-ckpt-* entries from tools.alsoAllow through the sanctioned
#    config mutation path. Never write openclaw.json directly: OpenClaw
#    >= 2026.9.2 guards config with a snapshot hash and rejects out-of-band
#    writes ("config changed since last load"). Reading the file to compute
#    the filtered array is safe; the write goes through `openclaw config set`.
OPENCLAW_CONFIG="${OPENCLAW_CONFIG_PATH:-${OPENCLAW_STATE_DIR}/openclaw.json}"
if [ -f "$OPENCLAW_CONFIG" ] && command -v "$OPENCLAW_BIN" &>/dev/null; then
    if [ "$OPENCLAW_CONFIG" != "${OPENCLAW_STATE_DIR}/openclaw.json" ]; then
        echo "WARN: OPENCLAW_CONFIG_PATH ($OPENCLAW_CONFIG) differs from the state-dir config" >&2
        echo "      (${OPENCLAW_STATE_DIR}/openclaw.json) that 'openclaw config set' writes;" >&2
        echo "      make sure OpenClaw actually reads the same file." >&2
    fi
    # Filter helper exit codes: 0 = filtered array on stdout; 2 = nothing to
    # remove (silent skip); 3 = config unreadable/unparseable; anything else
    # (e.g. 127 = node missing) warns below so leftover entries have a clue.
    # openclaw.json is JSON5 (OpenClaw tolerates comments/trailing commas),
    # so parsing goes through a string-aware json5-lite fallback first.
    filtered_allow="$(node -e '
var fs = require("fs");
var configPath = process.argv[1];

// Minimal JSON5 tolerance: strip // and /* */ comments and trailing commas
// via a state machine that never touches string contents (regex would
// corrupt e.g. URLs containing //). Other JSON5 syntax (single-quoted
// strings, unquoted keys, hex numbers) still fails closed.
function parseConfig(text) {
    try { return JSON.parse(text); } catch (e) { /* fall through to json5-lite */ }
    var out = "", i = 0, n = text.length, inStr = false, esc = false;
    while (i < n) {
        var c = text[i];
        if (inStr) {
            out += c;
            if (esc) esc = false;
            else if (c === "\\") esc = true;
            else if (c === "\"") inStr = false;
            i++; continue;
        }
        if (c === "\"") { inStr = true; out += c; i++; continue; }
        if (c === "/" && text[i + 1] === "/") { while (i < n && text[i] !== "\n") i++; continue; }
        if (c === "/" && text[i + 1] === "*") { i += 2; while (i + 1 < n && !(text[i] === "*" && text[i + 1] === "/")) i++; i += 2; continue; }
        if (c === ",") {
            // Trailing comma: skip whitespace AND comments before deciding.
            var j = i + 1;
            for (;;) {
                while (j < n && /\s/.test(text[j])) j++;
                if (text[j] === "/" && text[j + 1] === "/") { while (j < n && text[j] !== "\n") j++; continue; }
                if (text[j] === "/" && text[j + 1] === "*") { j += 2; while (j + 1 < n && !(text[j] === "*" && text[j + 1] === "/")) j++; j += 2; continue; }
                break;
            }
            if (text[j] === "}" || text[j] === "]") { i++; continue; }
            out += c; i++; continue;
        }
        out += c; i++;
    }
    return JSON.parse(out);
}

var config;
try { config = parseConfig(fs.readFileSync(configPath, "utf8")); }
catch(e) { process.exit(3); }
var tools = config.tools;
if (!tools || typeof tools !== "object") process.exit(2);
var alsoAllow = tools.alsoAllow;
if (!Array.isArray(alsoAllow)) process.exit(2);
var filtered = alsoAllow.filter(function(e) { return !(typeof e === "string" && e.startsWith("ws-ckpt-")); });
if (filtered.length === alsoAllow.length) process.exit(2);
process.stdout.write(JSON.stringify(filtered));
' "$OPENCLAW_CONFIG" 2>/dev/null)" && rc=0 || rc=$?
    if [ "$rc" = "0" ]; then
        if env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
            "$OPENCLAW_BIN" config set tools.alsoAllow "$filtered_allow" --strict-json; then
            echo "removed ws-ckpt entries from tools.alsoAllow"
        else
            echo "WARN: could not remove ws-ckpt entries from tools.alsoAllow via 'openclaw config set'" >&2
        fi
    elif [ "$rc" != "2" ]; then
        echo "WARN: could not read $OPENCLAW_CONFIG (exit $rc); ws-ckpt-* entries may remain in tools.alsoAllow" >&2
    fi
fi

# 3. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed from $SKILL_DST"
fi
