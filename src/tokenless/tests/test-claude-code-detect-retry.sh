#!/usr/bin/env bash
# Regression test for the detect.sh first-run settling retries.
#
# Background: GH #2512 reported test_detect_installed_framework_ready
# [claude-code] flaking in nightly runs. The reported cause was a
# filesystem/PATH initialization timing race on the first detect.sh
# execution right after provisioning: the claude binary under
# $HOME/.local/bin and the $HOME/.claude state dir are transiently
# invisible. #2512 itself was closed as a test-side flaky misclassification
# (issuecomment-5288391676), but the detect.sh-side weakness it exposed is
# real, and only one exit-status-affecting check can actually race: the
# claude binary lookup. A binary that is not yet visible makes detect.sh
# exit 2 ("missing prerequisites"), which is exactly what turns a
# "framework ready" assertion into a failure. (The $HOME/.claude config-dir
# probe is informational only and never changes the exit status.)
#
# Scenario 1 therefore reproduces that real failure path: the claude binary
# becomes visible in $HOME/.local/bin only after a delay (simulated with a
# background provisioner), and $HOME/.claude does not exist until the CLI's
# first `plugin list`. With settling retries, detect.sh must ride out the
# race and report ready (exit 0); without retries (scenario 2) the same
# first execution must fail with exit 2, proving the retries are what fix
# it. Scenarios 3-4 pin the plugin probe's retry semantics: a successful
# `plugin list` that omits the plugin is a definitive result and must not
# consume the retry budget, while a failing `plugin list` is transient and
# is retried.

set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
DETECT="$SCRIPT_DIR/../adapters/tokenless/claude-code/scripts/detect.sh"
TEST_DIR="$(mktemp -d)"

FAKE_HOME="$TEST_DIR/home"
FAKE_BIN="$FAKE_HOME/.local/bin"
SHARED_HOOKS="$FAKE_HOME/.local/share/anolisa/adapters/tokenless/common/hooks"
PLUGIN_ID="tokenless@anolisa-tokenless"
CALL_LOG="$TEST_DIR/claude-calls.log"
STUB_MODE_FILE="$TEST_DIR/stub-mode"
FLAKY_MARKER="$TEST_DIR/flaky-marker"
PROVISIONER_PID=""

cleanup() {
    if [ -n "$PROVISIONER_PID" ]; then
        kill "$PROVISIONER_PID" 2>/dev/null || true
        wait "$PROVISIONER_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN" "$SHARED_HOOKS"

# detect.sh also requires `tokenless` and `rtk` on PATH (their absence is a
# prerequisite failure). Provide isolated stubs so this test is
# self-contained: run_detect restricts PATH to the fake home plus system
# directories, and a fresh runner or detached worktree may not have either
# binary installed globally.
printf '#!/bin/sh\nexit 0\n' >"$FAKE_BIN/tokenless"
printf '#!/bin/sh\nexit 0\n' >"$FAKE_BIN/rtk"
chmod +x "$FAKE_BIN/tokenless" "$FAKE_BIN/rtk"

fail() {
    echo "FAIL: $1" >&2
    printf '%s\n' "${2:-}" >&2
    exit 1
}

# Stub claude CLI. `plugin list` behaviour is steered via $STUB_MODE_FILE:
#   ready  — the list succeeds and contains the tokenless plugin (default)
#   absent — the list succeeds but does not contain the plugin
#   flaky  — the first `plugin list` call fails while the registry
#            initializes; later calls succeed
# Like the real CLI, `plugin list` creates $HOME/.claude when it first
# runs; the config dir does not exist before that. Every invocation is
# appended to $CALL_LOG so the tests can count CLI calls.
install_claude_stub() {
    cat >"$FAKE_BIN/claude" <<'STUB'
#!/bin/sh
printf '%s\n' "${1:-}" >>"$CALL_LOG"
mode=ready
[ -f "$STUB_MODE_FILE" ] && mode=$(cat "$STUB_MODE_FILE")
case "${1:-}" in
--version)
    echo "claude 9.9.9-test"
    ;;
plugin)
    if [ "$mode" = "absent" ]; then
        echo "NAME                          STATUS"
        exit 0
    fi
    if [ "$mode" = "flaky" ] && [ ! -f "$FLAKY_MARKER" ]; then
        : >"$FLAKY_MARKER"
        echo "initializing plugin registry" >&2
        exit 1
    fi
    mkdir -p "$HOME/.claude"
    echo "NAME                          STATUS"
    echo "tokenless@anolisa-tokenless   enabled"
    ;;
*)
    exit 0
    ;;
esac
STUB
    chmod +x "$FAKE_BIN/claude"
}

schedule_claude() { # schedule_claude <delay-seconds>
    # Simulate provisioning: the binary becomes visible only after the delay.
    (
        sleep "$1"
        install_claude_stub
    ) &
    PROVISIONER_PID=$!
}

finish_provisioner() {
    wait "$PROVISIONER_PID"
    PROVISIONER_PID=""
}

cancel_provisioner() {
    kill "$PROVISIONER_PID" 2>/dev/null || true
    wait "$PROVISIONER_PID" 2>/dev/null || true
    PROVISIONER_PID=""
}

reset_env() {
    rm -rf "$FAKE_HOME/.claude"
    rm -f "$FAKE_BIN/claude" "$FLAKY_MARKER"
    echo ready >"$STUB_MODE_FILE"
}

run_detect() { # run_detect <retries> <retry-delay>
    : >"$CALL_LOG"
    rm -f "$FLAKY_MARKER"
    # Inherit only /usr/local/bin:/usr/bin:/bin (detect.sh itself prepends
    # $HOME/.local/bin): a claude installed elsewhere in the CI PATH must
    # not leak into the window while the stub is not yet provisioned.
    HOME="$FAKE_HOME" \
    PATH="/usr/local/bin:/usr/bin:/bin" \
    CALL_LOG="$CALL_LOG" \
    STUB_MODE_FILE="$STUB_MODE_FILE" \
    FLAKY_MARKER="$FLAKY_MARKER" \
    TOKENLESS_DETECT_RETRIES="$1" \
    TOKENLESS_DETECT_RETRY_DELAY="$2" \
        bash "$DETECT" 2>&1
}

plugin_list_calls() {
    grep -c '^plugin$' "$CALL_LOG" || true
}

# --- Scenario 1: the #2512 failure path, with settling retries ------------
# The claude binary is not yet visible in $HOME/.local/bin when detect.sh
# starts (first execution right after provisioning); it appears ~0.2s later
# while detect.sh is still settling (retry delay 0.5s). $HOME/.claude does
# not exist until the CLI's first `plugin list`. Expect ready (exit 0).
reset_env
schedule_claude 0.2
if ! out="$(run_detect 3 0.5)"; then
    fail "detect.sh should exit 0 (ready) once the settling retries find the claude binary" "$out"
fi
finish_provisioner
grep -qF "installed ($PLUGIN_ID)" <<<"$out" \
    || fail "plugin should be reported installed after the race settles" "$out"
grep -qF "claude-code: ready" <<<"$out" \
    || fail "claude-code should be reported ready after the race settles" "$out"
# Self-containment: detect.sh must have resolved the isolated stubs above,
# not any runner-installed binaries.
grep -qF "present ($FAKE_BIN/tokenless)" <<<"$out" \
    || fail "detect.sh should resolve the isolated tokenless stub, not a host binary" "$out"
grep -qF "present ($FAKE_BIN/rtk)" <<<"$out" \
    || fail "detect.sh should resolve the isolated rtk stub, not a host binary" "$out"
# The $HOME/.claude half of the reported race: the config dir is still
# invisible at probe time. detect.sh must report it as missing without
# letting that affect readiness (the probe is informational only).
grep -qF "missing (created on first claude run)" <<<"$out" \
    || fail "the config-dir probe should have observed the not-yet-created ~/.claude" "$out"

# --- Scenario 2 (control): same first execution, retries disabled ---------
# Without settling retries detect.sh checks once, does not see the binary
# (provisioning completes only after the check), and must fail exactly like
# the nightly framework-ready check did: exit 2, missing prerequisites.
reset_env
schedule_claude 2
set +e
out="$(run_detect 0 0)"
rc=$?
set -e
cancel_provisioner
[ "$rc" -eq 2 ] \
    || fail "without retries the first execution should exit 2 (missing prerequisites), got $rc" "$out"
grep -qF "claude CLI" <<<"$out" \
    || fail "the first execution without retries should mention the claude CLI" "$out"
grep -qF "missing prerequisites" <<<"$out" \
    || fail "the first execution without retries should report missing prerequisites" "$out"

# --- Scenario 3: definitive plugin-absent must not retry (PR review P2) ---
# The CLI is installed and `plugin list` works, but the tokenless plugin is
# not registered — the ordinary pre-install state. A successful list that
# omits the plugin is definitive: detect.sh must report "not installed"
# after exactly one `plugin list` invocation, without sleeping out the
# retry budget.
reset_env
install_claude_stub
echo absent >"$STUB_MODE_FILE"
mkdir -p "$FAKE_HOME/.claude"
set +e
out="$(run_detect 3 1)"
rc=$?
set -e
[ "$rc" -eq 1 ] \
    || fail "plugin-absent detection should exit 1 (installable), got $rc" "$out"
grep -qF "not installed" <<<"$out" \
    || fail "plugin should be reported not installed" "$out"
calls="$(plugin_list_calls)"
[ "$calls" -eq 1 ] \
    || fail "definitive plugin-absent must not retry: plugin list ran $calls times" "$out"

# --- Scenario 4: transient plugin-list failures are still retried ---------
# A `plugin list` call that fails outright (as opposed to one that succeeds
# without listing the plugin) is not definitive — the CLI may still be
# initializing — so settle() retries it.
reset_env
install_claude_stub
echo flaky >"$STUB_MODE_FILE"
if ! out="$(run_detect 3 0)"; then
    fail "detect.sh should exit 0 after retrying a transient plugin-list failure" "$out"
fi
grep -qF "installed ($PLUGIN_ID)" <<<"$out" \
    || fail "plugin should be reported installed after the transient failure" "$out"
calls="$(plugin_list_calls)"
[ "$calls" -eq 2 ] \
    || fail "the transient plugin-list failure should be retried once (saw $calls calls)" "$out"

echo "claude-code detect retry test passed"
