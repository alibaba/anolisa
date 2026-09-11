#!/usr/bin/env bash
# Behavioral tests for scripts/openclaw/install-openclaw.sh: capability gating
# plus the tools.alsoAllow pre-write through `openclaw config set`.

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

ALL_TOOLS_JSON='["ws-ckpt-checkpoint","ws-ckpt-rollback","ws-ckpt-list","ws-ckpt-delete","ws-ckpt-diff","ws-ckpt-config","ws-ckpt-status"]'

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

# Arrange the on-disk openclaw.json the pre-write merge will read.
setup_config() {
    local mode="$1"
    local cfg="$TMPDIR_TEST/state/openclaw.json"
    rm -rf "$TMPDIR_TEST/state"
    case "$mode" in
        absent) : ;;
        complete)
            mkdir -p "$TMPDIR_TEST/state"
            printf '%s\n' "{\"tools\":{\"alsoAllow\":$ALL_TOOLS_JSON}}" >"$cfg"
            ;;
        partial)
            mkdir -p "$TMPDIR_TEST/state"
            printf '%s\n' '{"tools":{"alsoAllow":["custom-tool","ws-ckpt-list"]}}' >"$cfg"
            ;;
        malformed)
            mkdir -p "$TMPDIR_TEST/state"
            printf '%s\n' '{not json' >"$cfg"
            ;;
        json5)
            mkdir -p "$TMPDIR_TEST/state"
            cat >"$cfg" <<'JSON5'
{
  // hand-edited comment
  "tools": {
    "alsoAllow": [
      "custom-tool",
      "https://example.com/a//b", /* URL keeps its // */
    ],
  },
}
JSON5
            ;;
        *) echo "FAIL: unknown setup_config mode: $mode" >&2; exit 1 ;;
    esac
}

# Exact-line comparison against the full invocation sequence: catches a
# missing, reordered, or duplicated call (config set, install, enable).
assert_calls() {
    local desc="$1"; shift
    local expected=("$@")
    mapfile -t calls <"$ARGV_LOG"

    if [ "${#calls[@]}" -ne "${#expected[@]}" ]; then
        echo "FAIL ($desc): expected ${#expected[@]} openclaw invocations, got ${#calls[@]}:" >&2
        printf '  %s\n' "${calls[@]}" >&2
        exit 1
    fi
    local i
    for i in "${!expected[@]}"; do
        if [ "${calls[$i]}" != "${expected[$i]}" ]; then
            echo "FAIL ($desc): call #$((i + 1)) mismatch" >&2
            echo "  expected: ${expected[$i]}" >&2
            echo "  actual:   ${calls[$i]}" >&2
            exit 1
        fi
    done
}

# Case 1: no config file yet — the pre-write must config-set all 7 tools
# between the capability probe and the plugin install, in every help mode.
for mode in modern legacy near-miss failure; do
    setup_config absent
    run_case "$mode"

    expected=(
        "plugins install --help"
        "config set tools.alsoAllow $ALL_TOOLS_JSON --strict-json"
    )
    install_expected="plugins install $PLUGIN_SRC --force"
    if [ "$mode" = "modern" ]; then
        install_expected="$install_expected --accept-capabilities"
    fi
    expected+=("$install_expected" "plugins enable ws-ckpt")
    assert_calls "absent/$mode" "${expected[@]}"
done

# Case 2: all tools already allowlisted — pre-write is skipped entirely.
setup_config complete
run_case modern
assert_calls "complete" \
    "plugins install --help" \
    "plugins install $PLUGIN_SRC --force --accept-capabilities" \
    "plugins enable ws-ckpt"

# Case 3: existing user entries are preserved; missing ws-ckpt tools append.
setup_config partial
run_case modern
assert_calls "partial" \
    "plugins install --help" \
    "config set tools.alsoAllow [\"custom-tool\",\"ws-ckpt-list\",\"ws-ckpt-checkpoint\",\"ws-ckpt-rollback\",\"ws-ckpt-delete\",\"ws-ckpt-diff\",\"ws-ckpt-config\",\"ws-ckpt-status\"] --strict-json" \
    "plugins install $PLUGIN_SRC --force --accept-capabilities" \
    "plugins enable ws-ckpt"

# Case 4: malformed config — pre-write is skipped (never clobber a config we
# cannot parse), install still proceeds.
setup_config malformed
run_case modern
assert_calls "malformed" \
    "plugins install --help" \
    "plugins install $PLUGIN_SRC --force --accept-capabilities" \
    "plugins enable ws-ckpt"

# Case 5: JSON5 config (comments + trailing commas) — openclaw.json is JSON5,
# so the merge must tolerate it; string contents (the URL) stay intact.
setup_config json5
run_case modern
assert_calls "json5" \
    "plugins install --help" \
    "config set tools.alsoAllow [\"custom-tool\",\"https://example.com/a//b\",\"ws-ckpt-checkpoint\",\"ws-ckpt-rollback\",\"ws-ckpt-list\",\"ws-ckpt-delete\",\"ws-ckpt-diff\",\"ws-ckpt-config\",\"ws-ckpt-status\"] --strict-json" \
    "plugins install $PLUGIN_SRC --force --accept-capabilities" \
    "plugins enable ws-ckpt"

echo "OpenClaw install script tests passed"
