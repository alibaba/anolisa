#!/usr/bin/env bash
# Pin the capability-consent negotiation of scripts/install.sh against a
# stub openclaw CLI: probe form (help captured, not piped to grep), whole-
# token flag matching, consent opt-out, and per-outcome log lines. No real
# plugin is installed.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/../adapters/agent-memory/openclaw/scripts/install.sh"
SANDBOX="$(mktemp -d -t agent-memory-openclaw-install.XXXXXX)"
trap 'rm -r -- "$SANDBOX"' EXIT
# The space in "adapter root" pins argv quoting through PLUGIN_DIR.
mkdir -p "$SANDBOX/adapter root/openclaw/dist"
: > "$SANDBOX/adapter root/openclaw/dist/index.js"

cat > "$SANDBOX/openclaw" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[ -z "${OPENCLAW_HOME+x}" ]
[ "$OPENCLAW_STATE_DIR" = "$TEST_STATE_DIR" ]
printf '%s\n' "$*" >> "$TEST_ARGV_LOG"
if [ "$1" = plugins ] && [ "$2" = install ] && [ "$3" = --help ]; then
    case "${TEST_HELP:?}" in
        modern)
            echo 'Usage: openclaw plugins install [options] <path>'
            echo 'Options:'
            echo "  --accept-capabilities  Accept the plugin's declared capabilities (default: false)"
            echo '  --force  Overwrite an existing installed plugin (default: false)' ;;
        legacy)
            echo 'Options:'
            echo '  --dangerously-force-unsafe-install  Bypass built-in dangerous-code install blocking'
            echo '  --force  Overwrite an existing installed plugin' ;;
        near_match)
            echo 'Options:'
            echo '  --accept-capabilities-only          unrelated option (default: false)'
            echo '  --no-accept-capabilities             reverse switch (default: false)' ;;
        colon)
            echo '  --accept-capabilities: accept declared capabilities' ;;
        paren)
            echo 'Options (--accept-capabilities):'
            echo '  --force  overwrite' ;;
        big)
            echo "  --accept-capabilities  Accept the plugin's declared capabilities"
            # Exceeds any pipe buffer: if the probe is ever reverted from
            # capture-into-variable to `cmd | grep -q`, grep's early exit
            # SIGPIPEs this writer under pipefail and the match is lost.
            printf 'filler line %.0s\n' {1..200000} ;;
        failed)
            echo 'OpenClaw could not start: --accept-capabilities requires a TTY' >&2
            exit 3 ;;
    esac
    exit 0
fi
if [ "$1" = config ] && [ "$2" = set ]; then
    exit 0
fi
[ "$1" = plugins ] && [ "$2" = install ]
[ "$3" = "$ANOLISA_ADAPTER_DIR/openclaw" ]
accepted=0
for arg in "$@"; do
    [ "$arg" != --accept-capabilities ] || accepted=1
done
if [ "${TEST_GATE:?}" = new ]; then
    [ "$accepted" = 1 ] || {
        echo 'Plugin "memory-anolisa" requires capability consent. Use --accept-capabilities, then retry.' >&2
        # Large trailing output: if the consent-signal check is ever
        # reverted to a short-circuit pipe (printf | grep -q), grep's early
        # exit SIGPIPEs this writer under pipefail and rc=3 degrades to 1.
        if [ "${TEST_INSTALL_BIG:-0}" = 1 ]; then
            printf 'filler line %.0s\n' {1..200000} >&2
        fi
        exit 1; }
elif [ "$TEST_GATE" = unrelated ]; then
    # Unconditional: an environment failure (e.g. EACCES) hits regardless
    # of whether the consent flag was passed, so the stream contract is
    # observable on the default path too.
    echo 'EACCES: permission denied, open plugin manifest' >&2; exit 1
else
    [ "$accepted" = 0 ] || { echo 'OpenClaw does not recognize option "--accept-capabilities".' >&2; exit 1; }
fi
: > "$TEST_INSTALLED"
STUB
chmod +x "$SANDBOX/openclaw"

export ANOLISA_ADAPTER_DIR="$SANDBOX/adapter root"
export OPENCLAW_BIN="$SANDBOX/openclaw"
export OPENCLAW_STATE_DIR="$SANDBOX/state root"
export OPENCLAW_HOME="$SANDBOX/ignored home"
export TEST_STATE_DIR="$OPENCLAW_STATE_DIR"
export TEST_ARGV_LOG="$SANDBOX/argv"
export TEST_INSTALLED="$SANDBOX/installed"

fail() {
    echo "FAIL: $1" >&2
    sed 's/^/    /' "$SANDBOX/output" >&2
    exit 1
}

# Full expected install argv for the given flag combination.
argv() {
    local a="plugins install $ANOLISA_ADAPTER_DIR/openclaw --force"
    if [ "$1" = yes ]; then a="$a --dangerously-force-unsafe-install"; fi
    if [ "$2" = yes ]; then a="$a --accept-capabilities"; fi
    printf '%s' "$a"
}

# scenario <label> <want-rc> <want-argv> [KEY=VAL ...] — run install.sh with
# the current TEST_HELP/TEST_GATE gating and assert rc, argv, and the
# config-set follow-up. The two installer switches are unset first so
# ambient values cannot leak into baseline scenarios; per-scenario KEY=VAL
# overrides are applied after the unsets.
scenario() {
    local label="$1" want_rc="$2" want_argv="$3"
    shift 3
    local rc=0
    : > "$TEST_ARGV_LOG"
    rm -f "$TEST_INSTALLED"
    env -u AGENT_MEMORY_SAFE_INSTALL -u AGENT_MEMORY_ACCEPT_CAPABILITIES \
        "$@" bash "$INSTALL_SH" >"$SANDBOX/output" 2>&1 || rc=$?
    [ "$rc" = "$want_rc" ] || fail "$label: rc=$rc, want $want_rc"
    [ "$(grep -c '^plugins install --help$' "$TEST_ARGV_LOG")" = 1 ] \
        || fail "$label: expected exactly one help probe"
    # awk (always exit 0) instead of grep -v | wc -l: a failing pipeline in
    # an assignment kills the whole suite under `set -euo pipefail` before
    # the fail() below can print its diagnostic.
    local install_calls
    install_calls="$(awk '/^plugins install / && !/--help$/ {n++} END {print n+0}' "$TEST_ARGV_LOG")"
    [ "$install_calls" = 1 ] \
        || fail "$label: expected exactly one install invocation, got $install_calls"
    local install_argv
    install_argv="$(awk '/^plugins install / && !/--help$/ {print}' "$TEST_ARGV_LOG")"
    [ "$install_argv" = "$want_argv" ] \
        || fail "$label: install argv '$install_argv' != '$want_argv'"
    if [ "$want_rc" = 0 ]; then
        [ -f "$TEST_INSTALLED" ] || fail "$label: install did not run"
        [ "$(grep -c '^config set plugins.entries.memory-anolisa.hooks.allowConversationAccess true$' "$TEST_ARGV_LOG")" = 1 ] \
            || fail "$label: allowConversationAccess config-set missing"
    else
        [ ! -f "$TEST_INSTALLED" ] || fail "$label: install ran despite failure"
    fi
    echo "PASS: $label"
}

expect_log() { grep -q -- "$1" "$SANDBOX/output" || fail "expected log line: $1"; }
expect_no_log() { ! grep -q -- "$1" "$SANDBOX/output" || fail "unexpected log line: $1"; }

# channel_scenario <label> <want-rc> <want-streams: split|merged> <marker> [KEY=VAL ...]
# — like scenario() but with separated capture (stdout and stderr to
# distinct files), pinning the stream contract itself: on the default path
# the CLI's stderr must stay on stderr (base behavior), while the withheld
# path deliberately merges the transcript into stdout. <marker> is the
# CLI-authored text to track across the two channels.
channel_scenario() {
    local label="$1" want_rc="$2" want_streams="$3" marker="$4"
    shift 4
    local rc=0
    : > "$TEST_ARGV_LOG"
    rm -f "$TEST_INSTALLED"
    env -u AGENT_MEMORY_SAFE_INSTALL -u AGENT_MEMORY_ACCEPT_CAPABILITIES \
        "$@" bash "$INSTALL_SH" >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
    [ "$rc" = "$want_rc" ] || { echo "FAIL: $label: rc=$rc, want $want_rc" >&2; channel_dump; exit 1; }
    local install_calls
    install_calls="$(awk '/^plugins install / && !/--help$/ {n++} END {print n+0}' "$TEST_ARGV_LOG")"
    [ "$install_calls" = 1 ] \
        || { echo "FAIL: $label: expected exactly one install invocation, got $install_calls" >&2; exit 1; }
    if [ "$want_streams" = merged ]; then
        grep -qF -- "$marker" "$SANDBOX/out" \
            || { echo "FAIL: $label: CLI text '$marker' missing from stdout (merged transcript expected)" >&2; channel_dump; exit 1; }
    else
        ! grep -qF -- "$marker" "$SANDBOX/out" \
            || { echo "FAIL: $label: CLI text '$marker' leaked into stdout on the split-stream path" >&2; channel_dump; exit 1; }
        grep -qF -- "$marker" "$SANDBOX/err" \
            || { echo "FAIL: $label: CLI text '$marker' missing from stderr (split streams expected)" >&2; channel_dump; exit 1; }
    fi
    echo "PASS: $label"
}

channel_dump() {
    echo "    --- captured stdout ---" >&2
    sed 's/^/    /' "$SANDBOX/out" >&2
    echo "    --- captured stderr ---" >&2
    sed 's/^/    /' "$SANDBOX/err" >&2
}

export TEST_HELP=modern TEST_GATE=new
scenario 'modern host, default' 0 "$(argv yes yes)"
expect_log 'Passing --accept-capabilities'
scenario 'modern host, safe install' 0 "$(argv no yes)" AGENT_MEMORY_SAFE_INSTALL=1
scenario 'modern host, explicit opt-in' 0 "$(argv yes yes)" AGENT_MEMORY_ACCEPT_CAPABILITIES=1
# Opting out must be a visible refusal: no flag, no consent log, and the
# gated install fails with the refusal conclusion (rc=3) instead of
# silently succeeding.
scenario 'modern host, consent opt-out' 3 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0
expect_log 'AGENT_MEMORY_ACCEPT_CAPABILITIES=0'
expect_no_log 'Passing --accept-capabilities'
expect_log 'install failed with consent withheld'
# Pins the merged-stream decision: the CLI's own rejection text stays
# visible in the script's output (streamed live via tee).
expect_log 'requires capability consent'
# The token table accepts the full boolean vocabulary, not just literal 0.
scenario 'modern host, consent opt-out via false' 3 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=false
scenario 'modern host, consent opt-in via TRUE' 0 "$(argv yes yes)" AGENT_MEMORY_ACCEPT_CAPABILITIES=TRUE
# Leading/trailing whitespace is trimmed, matching Rust env_bool().
scenario 'modern host, consent opt-out with padding' 3 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=' 0 '
# A consent rejection buried in large CLI output must still be attributed
# (rc=3): the signal check must never be a short-circuit pipe.
export TEST_INSTALL_BIG=1
scenario 'consent rejection with large install output' 3 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0
expect_log 'install failed with consent withheld'
unset TEST_INSTALL_BIG
# An install failure under opt-out that does not carry the consent-rejection
# phrase must stay a generic rc=1 failure with an opt-out note — never a
# misattributed policy refusal.
export TEST_GATE=unrelated
scenario 'opt-out with unrelated install error' 1 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0
expect_log 'does not look like a consent rejection'
expect_log 'EACCES'
expect_no_log 'install failed with consent withheld'
export TEST_GATE=new

export TEST_HELP=legacy TEST_GATE=old
scenario 'legacy host, default' 0 "$(argv yes no)"
expect_log 'did not advertise --accept-capabilities'
scenario 'legacy host, safe install' 0 "$(argv no no)" AGENT_MEMORY_SAFE_INSTALL=1
# Nothing to refuse when the host does not gate consent.
scenario 'legacy host, consent opt-out' 0 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0

export TEST_HELP=near_match
scenario 'near-miss options only' 0 "$(argv yes no)"
expect_log 'did not advertise --accept-capabilities'

# Alternate help layouts pin the whole-token match: a prefix-only regex
# misses these, a substring regex over-matches near-miss options.
export TEST_HELP=colon TEST_GATE=new
scenario 'colon help style' 0 "$(argv yes yes)"
export TEST_HELP=paren
scenario 'paren help style' 0 "$(argv yes yes)"
export TEST_HELP=big
scenario 'large help output' 0 "$(argv yes yes)"

# A failing probe whose error text names the flag must not be mistaken for
# an advertised option; the install degrades to base flags with a WARNING.
export TEST_HELP=failed TEST_GATE=old
scenario 'failed help probe' 0 "$(argv yes no)"
expect_log 'WARNING: cannot inspect OpenClaw installer options (rc=3)'
expect_no_log 'did not advertise --accept-capabilities'
# The same probe failure with the opt-out active must surface the switch —
# the advertised branch (and its refusal line) never runs. On a gating host
# the install then fails generically (rc=1, not 3): the host's gating
# status was never confirmed, so the refusal is not attributed.
scenario 'failed probe with opt-out' 0 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0
expect_log 'WARNING: cannot inspect OpenClaw installer options (rc=3)'
expect_log 'AGENT_MEMORY_ACCEPT_CAPABILITIES=0 is active'
export TEST_GATE=new
scenario 'failed probe with opt-out, gating host' 1 "$(argv yes no)" AGENT_MEMORY_ACCEPT_CAPABILITIES=0
expect_log 'AGENT_MEMORY_ACCEPT_CAPABILITIES=0 is active'
expect_no_log 'install failed with consent withheld'
export TEST_GATE=old

# Stream contract under separated capture: a default (consent-granted)
# install keeps the CLI's own stderr text on stderr — identical to base —
# while the withheld path deliberately merges the refusal transcript into
# stdout. The default-path case uses the unrelated-failure gate so the CLI
# actually writes stderr text to track.
export TEST_HELP=modern TEST_GATE=unrelated
channel_scenario 'default path keeps CLI stderr on stderr' 1 split 'EACCES'
export TEST_GATE=new
channel_scenario 'withheld path merges transcript into stdout' 3 merged 'requires capability consent' AGENT_MEMORY_ACCEPT_CAPABILITIES=0

# An unparseable switch value aborts before any OpenClaw invocation —
# including on hosts without the CLI at all (the validation precedes the
# missing-CLI early exit).
: > "$TEST_ARGV_LOG"
rc=0
env AGENT_MEMORY_ACCEPT_CAPABILITIES=maybe bash "$INSTALL_SH" >"$SANDBOX/output" 2>&1 || rc=$?
[ "$rc" = 2 ] || fail "invalid switch value: expected rc 2, got $rc"
[ ! -s "$TEST_ARGV_LOG" ] || fail 'invalid switch value: openclaw must not be invoked'
grep -q 'is not a boolean' "$SANDBOX/output" || fail 'invalid switch value: expected the boolean error'
echo 'PASS: invalid switch value aborts before any OpenClaw call'

# Interior whitespace must never be normalized into a valid token ('t rue'
# must not become a grant), and a whitespace-only value is "another value"
# per the documented contract — both abort before any CLI call.
for bad in 't rue' 'tr ue' '   '; do
    : > "$TEST_ARGV_LOG"
    rc=0
    env AGENT_MEMORY_ACCEPT_CAPABILITIES="$bad" bash "$INSTALL_SH" >"$SANDBOX/output" 2>&1 || rc=$?
    [ "$rc" = 2 ] || fail "switch value '$bad': expected rc 2, got $rc"
    [ ! -s "$TEST_ARGV_LOG" ] || fail "switch value '$bad': openclaw must not be invoked"
    grep -q 'is not a boolean' "$SANDBOX/output" || fail "switch value '$bad': expected the boolean error"
done
echo 'PASS: interior-whitespace and blank values abort before any OpenClaw call'

rc=0
env OPENCLAW_BIN="$SANDBOX/missing-openclaw" AGENT_MEMORY_ACCEPT_CAPABILITIES=maybe \
    bash "$INSTALL_SH" >"$SANDBOX/output" 2>&1 || rc=$?
[ "$rc" = 2 ] || fail "invalid switch value without CLI: expected rc 2, got $rc"
grep -q 'is not a boolean' "$SANDBOX/output" \
    || fail 'invalid switch value without CLI: expected the boolean error'
echo 'PASS: invalid switch value aborts even when the CLI is missing'

: > "$TEST_ARGV_LOG"
rc=0
OPENCLAW_BIN="$SANDBOX/missing-openclaw" bash "$INSTALL_SH" >"$SANDBOX/output" 2>&1 || rc=$?
[ "$rc" = 0 ] || fail "missing CLI: expected rc 0, got $rc"
[ ! -s "$TEST_ARGV_LOG" ] || fail 'missing CLI: openclaw must not be invoked'
grep -q 'skipping plugin installation' "$SANDBOX/output" \
    || fail 'missing CLI: expected skip behavior'
echo 'PASS: missing CLI preserves the existing skip behavior'

# Diagnosability pin: a regressed install.sh that probes but never invokes
# install must produce a FAIL diagnostic — not a silent set -e death from
# a failing pipeline inside an assignment. The scenario runs in a child
# bash -c: an OR-list subshell would suppress errexit inside it and hide
# exactly the silent death this pin guards against.
export TEST_HELP=modern TEST_GATE=new
fake_install="$SANDBOX/fake-install.sh"
cat > "$fake_install" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
echo "[agent-memory] Installing openclaw plugin..."
env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" plugins install --help >/dev/null 2>&1
# Regressed: exits successfully without invoking the install.
exit 0
FAKE
pin_rc=0
env SANDBOX="$SANDBOX" INSTALL_SH="$fake_install" \
    bash -c "set -euo pipefail; $(declare -f scenario fail); scenario 'harness pin: install never invoked' 0 'plugins install never-ran'" \
    >"$SANDBOX/pin-output" 2>&1 || pin_rc=$?
[ "$pin_rc" -ne 0 ] || fail 'harness pin: scenario should have failed'
grep -q 'expected exactly one install invocation' "$SANDBOX/pin-output" \
    || fail 'harness pin: no diagnosable FAIL line (silent death)'
echo 'PASS: missing install invocation fails with a diagnostic'

# remote-test is the only test path for macOS/Windows contributors; pin
# that its ssh command includes this suite (dry-run — nothing executes).
if command -v make >/dev/null 2>&1; then
    make -C "$SCRIPT_DIR/.." -n remote-test 2>/dev/null \
        | grep -q 'tests/test-openclaw-adapter-install.sh' \
        || fail 'remote-test does not run the installer test'
    echo 'PASS: remote-test includes the installer test'
fi

# The suite must be hermetic: hostile ambient switch values must neither
# leak into baseline scenarios nor break the suite itself.
if [ -z "${TEST_HOSTILE_AMBIENT:-}" ]; then
    rc=0
    env AGENT_MEMORY_ACCEPT_CAPABILITIES=0 AGENT_MEMORY_SAFE_INSTALL=1 TEST_HOSTILE_AMBIENT=1 \
        bash "$0" >"$SANDBOX/output" 2>&1 || rc=$?
    [ "$rc" = 0 ] || { sed 's/^/    /' "$SANDBOX/output" >&2; fail "hostile ambient environment broke the suite (rc=$rc)"; }
    echo 'PASS: hostile ambient environment does not leak into scenarios'
fi
