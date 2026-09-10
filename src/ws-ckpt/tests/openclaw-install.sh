#!/usr/bin/env bash
# Behavioral tests for scripts/openclaw/install-openclaw.sh capability gating.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_SCRIPT="$PROJECT_ROOT/scripts/openclaw/install-openclaw.sh"
REPO_ROOT="$(cd "$PROJECT_ROOT/../.." && pwd)"
PLUGIN_SRC="$PROJECT_ROOT/src/plugins/openclaw"
TMPDIR_TEST="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_TEST"' EXIT

FAKE_OPENCLAW="$TMPDIR_TEST/openclaw"
ARGV_LOG="$TMPDIR_TEST/argv.log"

# A staged runtime dir that really exists: a leaked ANOLISA_TARGET_DIR makes
# plugin discovery resolve here, so the install-path assertion fails loudly.
FAKE_TARGET_DIR="$TMPDIR_TEST/staged-target"
mkdir -p "$FAKE_TARGET_DIR/share/anolisa/runtime/ws-ckpt/plugins/openclaw"

# Hostile ambient values every run_case must strip. DRY_RUN=1 would divert the
# installer to its dry-run branch and leave the argv log empty.
export ANOLISA_DRY_RUN=1
export ANOLISA_TARGET_DIR="$FAKE_TARGET_DIR"

cat >"$FAKE_OPENCLAW" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"$ARGV_LOG"

if [ "$*" = "plugins install --help" ]; then
    case "${INSTALL_HELP_MODE:-modern}" in
        modern) printf '%s\n' 'Usage: openclaw plugins install [--force] [--accept-capabilities]' ;;
        legacy) printf '%s\n' 'Usage: openclaw plugins install [--force]' ;;
        near-miss) printf '%s\n' 'Usage: openclaw plugins install [--accept-capabilities-only]' ;;
        failure) exit 2 ;;
    esac
fi
exit 0
EOF
chmod +x "$FAKE_OPENCLAW"

run_case() {
    local mode="$1"
    : >"$ARGV_LOG"
    env -u ANOLISA_DRY_RUN -u ANOLISA_TARGET_DIR \
        ARGV_LOG="$ARGV_LOG" \
        INSTALL_HELP_MODE="$mode" \
        OPENCLAW_BIN="$FAKE_OPENCLAW" \
        OPENCLAW_STATE_DIR="$TMPDIR_TEST/state" \
        ANOLISA_PROJECT_ROOT="$REPO_ROOT" \
        "$INSTALL_SCRIPT" >/dev/null
}

# The installer must invoke openclaw exactly three times, in order: capability
# probe, plugin install, plugin enable. Exact-line comparison catches a
# missing, trailing, or duplicated --accept-capabilities token and any extra
# or repeated install call.
assert_install() {
    local with_flag="$1"  # "flag" or "noflag"
    mapfile -t calls <"$ARGV_LOG"

    if [ "${#calls[@]}" -ne 3 ]; then
        echo "FAIL: expected 3 openclaw invocations (probe/install/enable), got ${#calls[@]}:" >&2
        printf '  %s\n' "${calls[@]}" >&2
        exit 1
    fi
    if [ "${calls[0]}" != "plugins install --help" ]; then
        echo "FAIL: unexpected probe call: ${calls[0]}" >&2
        exit 1
    fi
    if [ "${calls[2]}" != "plugins enable ws-ckpt" ]; then
        echo "FAIL: unexpected enable call: ${calls[2]}" >&2
        exit 1
    fi

    local expected="plugins install $PLUGIN_SRC --force"
    if [ "$with_flag" = "flag" ]; then
        expected="$expected --accept-capabilities"
    fi
    if [ "${calls[1]}" != "$expected" ]; then
        echo "FAIL: install invocation mismatch" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   ${calls[1]}" >&2
        exit 1
    fi
}

run_case modern
assert_install flag

for mode in legacy near-miss failure; do
    run_case "$mode"
    assert_install noflag
done

echo "OpenClaw install script tests passed"
