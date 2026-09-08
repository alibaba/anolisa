#!/usr/bin/env bash
# Check installer flag negotiation (capability consent and the legacy
# unsafe-install bypass) without installing a real plugin.
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
        modern|modern_unsafe) echo "--accept-capabilities" ;;
        near_match) echo "--accept-capabilities-only" ;;
        failed) echo "cannot inspect --accept-capabilities" >&2; exit 3 ;;
    esac
    case "$TEST_SUPPORT" in
        unsafe_effective|modern_unsafe)
            echo "--dangerously-force-unsafe-install  Bypass the install safety scan" ;;
        unsafe_noop|noop_rejected)
            echo "--dangerously-force-unsafe-install  Deprecated no-op; governed by security.installPolicy" ;;
        unsafe_noop_upper)
            echo "--dangerously-force-unsafe-install  Deprecated NO-OP; governed by security.installPolicy" ;;
        unsafe_near) echo "--dangerously-force-unsafe-install-only" ;;
    esac
    exit 0
fi
[ "$1" = plugins ] && [ "$2" = install ]
[ "$3" = "$ANOLISA_ADAPTER_DIR/openclaw" ]
accepted=0
unsafe=0
for arg in "$@"; do
    [ "$arg" != --accept-capabilities ] || accepted=1
    [ "$arg" != --dangerously-force-unsafe-install ] || unsafe=1
done
case "$TEST_SUPPORT" in
    modern|modern_unsafe)
        [ "$accepted" = 1 ] || { echo 'Plugin requires capability consent' >&2; exit 1; }
        ;;
    *)
        [ "$accepted" = 0 ] || { echo 'unknown option --accept-capabilities' >&2; exit 2; }
        ;;
esac
case "$TEST_SUPPORT" in
    unsafe_effective|modern_unsafe)
        [ "$unsafe" = 1 ] || { echo 'install safety scan blocks child_process plugins' >&2; exit 4; }
        ;;
    *)
        [ "$unsafe" = 0 ] || { echo 'unknown option --dangerously-force-unsafe-install' >&2; exit 5; }
        ;;
esac
[ "$TEST_SUPPORT" != noop_rejected ] || { echo 'install blocked by security.installPolicy' >&2; exit 6; }
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

for TEST_SUPPORT in modern legacy near_match unsafe_effective unsafe_noop unsafe_noop_upper unsafe_near modern_unsafe; do
    export TEST_SUPPORT
    : > "$TEST_ARGV_LOG"
    bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1 || { cat "$SANDBOX/output" >&2; exit 1; }
    [ -f "$TEST_INSTALLED" ]
    [ "$(grep -c '^plugins install --help$' "$TEST_ARGV_LOG")" = 1 ]
    case "$TEST_SUPPORT" in
        unsafe_effective|modern_unsafe)
            grep -q -- '--dangerously-force-unsafe-install is required' "$SANDBOX/output" ;;
        *)
            ! grep -q -- '--dangerously-force-unsafe-install is required' "$SANDBOX/output" ;;
    esac
    rm "$TEST_INSTALLED"
    echo "PASS: $TEST_SUPPORT installer"
done

export TEST_SUPPORT=noop_rejected
if bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1; then
    echo 'FAIL: policy-rejected install reported success' >&2
    exit 1
fi
[ ! -f "$TEST_INSTALLED" ]
grep -q 'security.installPolicy' "$SANDBOX/output"
grep -q 'deprecated no-op' "$SANDBOX/output"
echo 'PASS: noop host failure points at security.installPolicy'

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
grep -q -- '--dangerously-force-unsafe-install' "$SANDBOX/output"
echo 'PASS: dry-run describes consent and bypass gating without invoking OpenClaw'

OPENCLAW_BIN="$SANDBOX/missing-openclaw" bash "$INSTALL_SH" > "$SANDBOX/output" 2>&1
grep -q 'skipping plugin installation' "$SANDBOX/output"
echo 'PASS: missing CLI preserves the existing skip behavior'
