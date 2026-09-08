#!/usr/bin/env bash
# Check capability-consent negotiation without installing a real plugin.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/../adapters/tokenless/openclaw/scripts/install.sh"
SANDBOX="$(mktemp -d -t tokenless-openclaw-install.XXXXXX)"
trap 'rm -r -- "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/adapter root/openclaw/dist"
: > "$SANDBOX/adapter root/openclaw/dist/index.js"

cat > "$SANDBOX/openclaw" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[ -z "${OPENCLAW_HOME+x}" ]
[ "$OPENCLAW_STATE_DIR" = "$TEST_STATE_DIR" ]
printf '%s\n' "$*" >> "$TEST_ARGV_LOG"
if [ "$*" = "plugins install --help" ]; then
    echo "--force"
    case "$TEST_SUPPORT" in
        modern) echo "--accept-capabilities" ;;
        near_match) echo "--accept-capabilities-only" ;;
        failed) echo "cannot inspect --accept-capabilities" >&2; exit 3 ;;
    esac
    exit 0
fi
[ "$1" = plugins ] && [ "$2" = install ]
[ "$3" = "$ANOLISA_ADAPTER_DIR/openclaw" ]
accepted=0
for arg in "$@"; do
    [ "$arg" != --accept-capabilities ] || accepted=1
done
if [ "$TEST_SUPPORT" = modern ]; then
    [ "$accepted" = 1 ] || { echo 'Plugin requires capability consent' >&2; exit 1; }
else
    [ "$accepted" = 0 ] || { echo 'unknown option --accept-capabilities' >&2; exit 2; }
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
export ANOLISA_DRY_RUN=0

for TEST_SUPPORT in modern legacy near_match; do
    export TEST_SUPPORT
    : > "$TEST_ARGV_LOG"
    bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1 || { cat "$SANDBOX/output" >&2; exit 1; }
    [ -f "$TEST_INSTALLED" ]
    [ "$(grep -c '^plugins install --help$' "$TEST_ARGV_LOG")" = 1 ]
    rm "$TEST_INSTALLED"
    echo "PASS: $TEST_SUPPORT installer"
done

export TEST_SUPPORT=failed
if bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1; then
    echo 'FAIL: failed help probe allowed installation' >&2
    exit 1
fi
[ ! -f "$TEST_INSTALLED" ]
echo 'PASS: failed help probe stops installation'

: > "$TEST_ARGV_LOG"
ANOLISA_DRY_RUN=1 bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1
[ ! -s "$TEST_ARGV_LOG" ]
[ ! -f "$TEST_INSTALLED" ]
grep -q -- '--accept-capabilities' "$SANDBOX/output"
echo 'PASS: dry-run describes consent without invoking OpenClaw'

OPENCLAW_BIN="$SANDBOX/missing-openclaw" bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1
grep -q 'skipping plugin installation' "$SANDBOX/output"
echo 'PASS: missing CLI preserves the existing skip behavior'
