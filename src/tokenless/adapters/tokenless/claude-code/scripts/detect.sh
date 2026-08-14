#!/usr/bin/env bash
# detect.sh — Inspect Claude Code presence and the tokenless plugin state.
# Read-only. Tri-state exit aligns with openclaw/hermes detect.sh:
#   0 = installed and ready
#   1 = not installed but installable (prereqs OK)
#   2 = missing prerequisites
set -euo pipefail

COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
AGENT="${ANOLISA_TARGET:-claude-code}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

PLUGIN_ID="${COMPONENT}@anolisa-${COMPONENT}"
PLUGIN_SRC="$ADAPTER_DIR/claude-code"

CLAUDE_BIN="${CLAUDE_BIN:-}"
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"

# First-run settling retries: right after provisioning, the claude binary or
# the plugin registry may be transiently invisible on the very first detect.sh
# execution (filesystem/PATH init timing race). settle() only retries checks
# that report a retryable failure (exit status 1); checks that succeed or
# report a definitive result return immediately, so steady-state runs stay
# fast.
DETECT_RETRIES="${TOKENLESS_DETECT_RETRIES:-3}"
DETECT_RETRY_DELAY="${TOKENLESS_DETECT_RETRY_DELAY:-1}"

# settle <cmd...> — run cmd once; if it reports a retryable failure (exit
# status 1), sleep DETECT_RETRY_DELAY and retry, up to DETECT_RETRIES retries
# (that is, at most 1 + DETECT_RETRIES attempts in total). Exit status 0 is
# success. Any other exit status is a definitive result and is returned
# immediately without further retries. Returns the final exit status.
# Callers must therefore reserve exit status 1 for conditions that a retry
# may still resolve.
settle() {
    local retry=0 rc=0
    "$@"; rc=$?
    while [ "$rc" -eq 1 ] && [ "$retry" -lt "$DETECT_RETRIES" ]; do
        retry=$((retry + 1))
        sleep "$DETECT_RETRY_DELAY"
        "$@"; rc=$?
    done
    return "$rc"
}

line()  { printf '[%s] %s\n' "$COMPONENT" "$*"; }
field() { printf '[%s]   %-26s %s\n' "$COMPONENT" "$1" "$2"; }

PREREQ_MISSING=()
INSTALL_MISSING=()
note_prereq_missing()  { PREREQ_MISSING+=("$1"); }
note_install_missing() { INSTALL_MISSING+=("$1"); }

find_claude_bin() {
    CLAUDE_BIN="$(command -v claude 2>/dev/null || true)"
    [ -n "$CLAUDE_BIN" ]
}

if [ -z "$CLAUDE_BIN" ]; then
    # PATH/filesystem may still be settling right after install; re-check
    # briefly before declaring the CLI missing. A genuinely absent CLI cannot
    # be distinguished from that settling race, so it spends one retry budget
    # (set TOKENLESS_DETECT_RETRIES=0 to opt out).
    settle find_claude_bin || true
fi

line "${AGENT} detect"
if [ -n "$CLAUDE_BIN" ] && [ -x "$CLAUDE_BIN" ]; then
    CLAUDE_VER="$("$CLAUDE_BIN" --version 2>/dev/null | awk '{print $1}' || echo unknown)"
    field "claude CLI"        "present (${CLAUDE_BIN}, v${CLAUDE_VER})"
else
    field "claude CLI"        "missing"
    note_prereq_missing "claude CLI"
fi

# Informational only: claude creates ~/.claude on first run; absence is
# not a prerequisite failure. Check once, without settling retries: nothing in
# this script creates the directory before `claude plugin list` runs, and that
# call carries its own retry budget and initializes ~/.claude itself, so
# retrying this probe would only delay the report.
if [ -d "$HOME/.claude" ]; then
    field "claude config dir" "present ($HOME/.claude)"
else
    field "claude config dir" "missing (created on first claude run)"
fi

if [ -f "$PLUGIN_SRC/.claude-plugin/marketplace.json" ]; then
    field "marketplace.json"  "present"
else
    field "marketplace.json"  "missing"
    note_prereq_missing "marketplace.json"
fi

if [ -f "$PLUGIN_SRC/.claude-plugin/plugin.json" ]; then
    field "plugin.json"       "present"
else
    field "plugin.json"       "missing (run: make stamp-adapter-templates)"
fi

# claude_plugin_listed — probe the plugin registry, using the settle()
# exit-status contract: 0 = plugin listed; 1 = `claude plugin list` itself
# failed (the CLI may still be initializing ~/.claude on first run, so a
# retry may still succeed); 2 = `plugin list` ran successfully but did not
# list the plugin. That is a definitive absent result — no retry can change
# it — so settle() must return immediately instead of sleeping out the
# retry budget and invoking the CLI DETECT_RETRIES + 1 times.
claude_plugin_listed() {
    local listing
    if ! listing="$("$CLAUDE_BIN" plugin list 2>&1)"; then
        return 1
    fi
    if printf '%s\n' "$listing" | grep -qF "$PLUGIN_ID"; then
        return 0
    fi
    return 2
}

if [ -n "$CLAUDE_BIN" ] && [ -x "$CLAUDE_BIN" ]; then
    # First-run race: `claude plugin list` may transiently fail while the CLI
    # initializes ~/.claude; settle() retries while that failure (status 1)
    # persists. A successful list that simply omits the plugin is definitive
    # (status 2) and is reported as "not installed" without further retries.
    if settle claude_plugin_listed; then
        field "plugin install"    "installed ($PLUGIN_ID)"
    else
        field "plugin install"    "not installed"
        note_install_missing "$PLUGIN_ID"
    fi
fi

if [ -f "$PLUGIN_SRC/hooks/run-hook.sh" ]; then
    field "hook dispatcher"   "present"
else
    field "hook dispatcher"   "missing (hooks/run-hook.sh)"
    note_prereq_missing "hook dispatcher"
fi

if command -v python3 &>/dev/null; then
    field "python3"           "present ($(command -v python3))"
else
    field "python3"           "missing"
    note_prereq_missing "python3"
fi

# jq is required by tool_ready_hook.sh; absence disables that hook only
# (rewrite + compress-response still work). Treat as informational.
if command -v jq &>/dev/null; then
    field "jq"                "present ($(command -v jq))"
else
    field "jq"                "missing (tool-ready hook disabled)"
fi

runtime_bin="$(command -v tokenless 2>/dev/null || true)"
if [ -n "$runtime_bin" ]; then
    field "tokenless binary"  "present (${runtime_bin})"
else
    field "tokenless binary"  "missing"
    note_prereq_missing "tokenless binary"
fi

rtk_bin="$(command -v rtk 2>/dev/null || true)"
if [ -n "$rtk_bin" ]; then
    field "rtk binary"        "present (${rtk_bin})"
else
    field "rtk binary"        "missing"
    note_prereq_missing "rtk binary"
fi

# Shared hook scripts live under FHS; warn when missing so user knows to run
# `make install` (or install the RPM) before adapter actually fires.
SHARED_HOOKS_DIR=""
for d in /usr/local/share/anolisa/adapters/tokenless/common/hooks \
         /usr/share/anolisa/adapters/tokenless/common/hooks \
         "$HOME/.local/share/anolisa/adapters/tokenless/common/hooks"; do
    if [ -d "$d" ]; then SHARED_HOOKS_DIR="$d"; break; fi
done
if [ -n "$SHARED_HOOKS_DIR" ]; then
    field "shared hooks dir"  "present ($SHARED_HOOKS_DIR)"
else
    field "shared hooks dir"  "missing (run: make -C src/tokenless install)"
    note_prereq_missing "shared hooks dir"
fi

if [ ${#PREREQ_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: missing prerequisites (${PREREQ_MISSING[*]})"
    exit 2
fi
if [ ${#INSTALL_MISSING[@]} -gt 0 ]; then
    line "${AGENT}: not installed (ready to install)"
    exit 1
fi
line "${AGENT}: ready"
exit 0
