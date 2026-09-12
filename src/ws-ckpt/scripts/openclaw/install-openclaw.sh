#!/bin/bash

set -euo pipefail

# shellcheck source=lib-discover.sh
source "$(dirname "$0")/lib-discover.sh"

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"
DRY_RUN="${ANOLISA_DRY_RUN:-0}"
SKILL_DST="${OPENCLAW_STATE_DIR%/}/skills/ws-ckpt"

help_lists_flag() {
    local help_text="$1"
    local flag="$2"

    grep -Eq "(^|[^[:alnum:]_.-])${flag}([^[:alnum:]_.-]|$)" <<<"$help_text"
}

# 1. Check openclaw availability. Dry-run should not require the CLI.
if [ "$DRY_RUN" != "1" ] && ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "ERROR: openclaw is not installed, please install openclaw first"
    exit 1
fi

# 2. Try plugin install (preferred).
if PLUGIN_SRC=$(find_plugin_src openclaw); then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: probe '$OPENCLAW_BIN plugins install --help' for --accept-capabilities"
        echo "DRY-RUN: merge ws-ckpt tools into tools.alsoAllow, then env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN config set tools.alsoAllow <merged-json> --strict-json"
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins install $PLUGIN_SRC --force [--accept-capabilities when supported]"
        echo "DRY-RUN: env -u OPENCLAW_HOME OPENCLAW_STATE_DIR=$OPENCLAW_STATE_DIR $OPENCLAW_BIN plugins enable ws-ckpt"
        exit 0
    fi

    install_args=(plugins install "$PLUGIN_SRC" --force)
    install_help=""
    if install_help="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
        "$OPENCLAW_BIN" plugins install --help 2>/dev/null)" \
        && help_lists_flag "$install_help" "--accept-capabilities"; then
        install_args+=(--accept-capabilities)
    fi

    # Pre-write tools.alsoAllow through the sanctioned config mutation path so
    # the plugin's register() never has to write openclaw.json: OpenClaw
    # >= 2026.9.2 rejects out-of-band config writes with "config changed since
    # last load", which fails the install. Reading the file from disk is safe;
    # only writes must go through `openclaw config set`.
    # The tool list mirrors WS_CKPT_TOOL_NAMES in src/plugins/openclaw (kept
    # in sync by a drift-guard unit test in whitelist.test.ts).
    OPENCLAW_CONFIG="${OPENCLAW_CONFIG_PATH:-${OPENCLAW_STATE_DIR}/openclaw.json}"
    if [ "$OPENCLAW_CONFIG" != "${OPENCLAW_STATE_DIR}/openclaw.json" ]; then
        echo "WARN: OPENCLAW_CONFIG_PATH ($OPENCLAW_CONFIG) differs from the state-dir config" >&2
        echo "      (${OPENCLAW_STATE_DIR}/openclaw.json) that 'openclaw config set' writes;" >&2
        echo "      make sure OpenClaw actually reads the same file." >&2
    fi
    # Merge helper exit codes: 0 = merged array on stdout; 2 = already
    # complete (silent skip); 3 = config unreadable/unparseable (never
    # clobber); anything else (e.g. 127 = node missing) warns below.
    # openclaw.json is JSON5 (OpenClaw tolerates comments/trailing commas),
    # so parsing goes through a string-aware json5-lite fallback first.
    merged_allow="$(node -e '
var fs = require("fs");
var configPath = process.argv[1];
var OURS = ["ws-ckpt-checkpoint","ws-ckpt-rollback","ws-ckpt-list","ws-ckpt-delete","ws-ckpt-diff","ws-ckpt-config","ws-ckpt-status"];

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

var current = [];
if (fs.existsSync(configPath)) {
    var parsed;
    try { parsed = parseConfig(fs.readFileSync(configPath, "utf8")); }
    catch (e) { process.exit(3); }
    var allow = parsed && parsed.tools && parsed.tools.alsoAllow;
    if (Array.isArray(allow)) current = allow.map(String);
}
var missing = OURS.filter(function (t) { return current.indexOf(t) === -1; });
if (missing.length === 0) process.exit(2);
process.stdout.write(JSON.stringify(current.concat(missing)));
' "$OPENCLAW_CONFIG" 2>/dev/null)" && rc=0 || rc=$?
    if [ "$rc" = "0" ]; then
        env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
            "$OPENCLAW_BIN" config set tools.alsoAllow "$merged_allow" --strict-json \
            || echo "WARN: could not add ws-ckpt tools to tools.alsoAllow via 'openclaw config set'; ws-ckpt tools may stay blocked" >&2
    elif [ "$rc" != "2" ]; then
        echo "WARN: could not merge tools.alsoAllow from $OPENCLAW_CONFIG (exit $rc); skipping config pre-write" >&2
    fi

    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
        "$OPENCLAW_BIN" "${install_args[@]}"
    env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins enable ws-ckpt 2>/dev/null || true
    echo "openclaw ws-ckpt plugin installed and enabled successfully (from $PLUGIN_SRC)"
    exit 0
fi

# 3. Fallback to skill install
if SKILL_SRC=$(find_skill_src); then
    if [ "$DRY_RUN" = "1" ]; then
        echo "DRY-RUN: mkdir -p $SKILL_DST"
        echo "DRY-RUN: cp -pr $SKILL_SRC/. $SKILL_DST/"
        exit 0
    fi
    mkdir -p "$SKILL_DST"
    cp -pr "$SKILL_SRC"/. "$SKILL_DST/"
    echo "skill installed to $SKILL_DST (from $SKILL_SRC)"
else
    print_search_error
    exit 1
fi
