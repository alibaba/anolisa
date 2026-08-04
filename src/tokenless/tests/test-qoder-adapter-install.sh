#!/usr/bin/env bash
# Regression coverage for Qoder's native plugin lifecycle and legacy cleanup.
set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'
PASS=0
FAIL=0
pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)); return 0; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)); return 0; }
info() { echo -e "${BLUE}[INFO]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_ADAPTER_DIR="$(cd "$SCRIPT_DIR/../adapters/tokenless" && pwd)"
INSTALL_OVERRIDE="${1:-}"
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

SANDBOX="$(mktemp -d -t tokenless-qoder-native-test.XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
ADAPTER_DIR="$SANDBOX/adapters/tokenless"
mkdir -p "$(dirname "$ADAPTER_DIR")"
cp -a "$SOURCE_ADAPTER_DIR" "$ADAPTER_DIR"
PLUGIN_TEMPLATE="$ADAPTER_DIR/qoder/.qoder-plugin/plugin.json.in"
PLUGIN_MANIFEST="$ADAPTER_DIR/qoder/.qoder-plugin/plugin.json"
sed 's/@VERSION@/0.0.0-test/g' "$PLUGIN_TEMPLATE" > "$PLUGIN_MANIFEST"
INSTALL_SH="${INSTALL_OVERRIDE:-$ADAPTER_DIR/qoder/scripts/install.sh}"
UNINSTALL_SH="$ADAPTER_DIR/qoder/scripts/uninstall.sh"
FAKE_HOME="$SANDBOX/home"
QODER_CONFIG="$SANDBOX/qoder-config"
FAKE_BIN="$SANDBOX/bin/qodercli"
FAKE_STATE="$SANDBOX/installed"
FAKE_CACHE="$SANDBOX/cache/tokenless/0.7.2"
FAKE_MODE="$SANDBOX/list-mode"
mkdir -p "$FAKE_HOME/.qoder" "$QODER_CONFIG" "$(dirname "$FAKE_BIN")"

cat > "$FAKE_BIN" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1" != "plugins" ]; then exit 1; fi
mode="$(cat "$FAKE_QODER_MODE" 2>/dev/null || true)"
if [ "${3:-}" = "--help" ]; then
    [ "$mode" = "missing-install-capability" ] && [ "$2" = "install" ] && exit 1
    exit 0
fi
case "$2" in
  validate)
    test -f "$3/hooks/hooks.json"
    test -f "$3/commands/tokenless-stats.md"
    echo 'Convention components found: commands/ hooks/hooks.json'
    ;;
  install)
    [ "$mode" = "fail-install" ] && { echo 'install boom' >&2; exit 1; }
    src="$3"
    rm -rf "$FAKE_QODER_CACHE"
    mkdir -p "$FAKE_QODER_CACHE"
    cp -RL "$src"/. "$FAKE_QODER_CACHE"/
    : > "$FAKE_QODER_STATE"
    QODER_SETTINGS="$QODER_CONFIG_DIR/settings.json" python3 - <<'PY'
import json, os
path = os.environ['QODER_SETTINGS']
cfg = json.load(open(path)) if os.path.exists(path) else {}
cfg.setdefault('enabledPlugins', {})['tokenless@local'] = True
with open(path, 'w') as stream:
    json.dump(cfg, stream, indent=2)
PY
    echo 'Plugin "tokenless@local" installed successfully.'
    ;;
  list)
    [ "$mode" = "invalid-json" ] && { echo 'not-json'; exit 0; }
    [ "$mode" = "error-json" ] && { echo '{"error":"not ready"}'; exit 0; }
    [ "$mode" = "stderr-warning" ] && echo 'qodercli warning' >&2
    if [ ! -f "$FAKE_QODER_STATE" ]; then echo '[]'; exit 0; fi
    if [ "$mode" = "distractor-resources" ]; then
        echo '[{"id":"tokenless@local","enabled":true,"resources":{"commands":[{"name":"not-tokenless-stats","description":"tokenless-stats"}],"hooks":[{"event":"PreToolUse"},{"event":"Other","description":"PreToolUse"},{"event":"Other","description":"PostToolUse"}]}}]'
        exit 0
    fi
    enabled=true
    [ "$mode" = "disabled" ] && enabled=false
    printf '[{"id":"tokenless@local","enabled":%s,"resources":{"commands":[{"name":"tokenless:tokenless-stats"}],"hooks":[{"event":"PreToolUse"},{"event":"PreToolUse"},{"event":"PostToolUse"}]}}]\n' "$enabled"
    ;;
  uninstall)
    [ -f "$FAKE_QODER_STATE" ] || { echo 'Plugin "tokenless" is not installed.'; exit 1; }
    rm -f "$FAKE_QODER_STATE"
    rm -rf "$FAKE_QODER_CACHE"
    echo 'Plugin "tokenless@local" fully uninstalled.'
    ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$FAKE_BIN"

run_install() {
    HOME="$FAKE_HOME" \
    QODER_CONFIG_DIR="$QODER_CONFIG" \
    QODERCLI_BIN="$FAKE_BIN" \
    FAKE_QODER_STATE="$FAKE_STATE" \
    FAKE_QODER_CACHE="$FAKE_CACHE" \
    FAKE_QODER_MODE="$FAKE_MODE" \
    ANOLISA_ADAPTER_DIR="$ADAPTER_DIR" \
    bash "$INSTALL_SH" 2>&1
}

run_uninstall() {
    HOME="$FAKE_HOME" \
    QODER_CONFIG_DIR="$QODER_CONFIG" \
    QODERCLI_BIN="$FAKE_BIN" \
    FAKE_QODER_STATE="$FAKE_STATE" \
    FAKE_QODER_CACHE="$FAKE_CACHE" \
    FAKE_QODER_MODE="$FAKE_MODE" \
    ANOLISA_ADAPTER_DIR="$ADAPTER_DIR" \
    bash "$UNINSTALL_SH" 2>&1
}

# Historical tokenless installers ignored QODER_CONFIG_DIR. Keep the legacy
# path separate so this test also proves native state uses the custom root.
LEGACY_SETTINGS="$FAKE_HOME/.qoder/settings.json"
cat > "$LEGACY_SETTINGS" <<EOF
{
  "theme": "dark",
  "enabledPlugins": {"user-plugin@local": true},
  "plugins": {"enabled": ["other@local", "tokenless@local"]},
  "hooks": {
    "PreToolUse": [
      {"matcher":"","sequential":true,"hooks":[{"type":"command","name":"tokenless-tool-ready","description":"Pre-checks tool environment readiness, auto-fixes, and provides skip-retry guidance","command":"bash $ADAPTER_DIR/common/hooks/tool_ready_hook.sh","timeout":10000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]},
      {"matcher":"^(Bash|Shell|run_shell_command|terminal|execute_command)$","hooks":[{"type":"command","name":"tokenless-rewrite","description":"Rewrites shell commands via rtk for token savings","command":"python3 $SANDBOX/common/hooks/rewrite_hook.py","timeout":5000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]},
      {"matcher":"^(Bash|Shell|run_shell_command|terminal|execute_command)$","hooks":[{"type":"command","name":"tokenless-rewrite","description":"Rewrites shell commands via rtk for token savings","command":"python3 /usr/local/share/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py","timeout":5000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]},
      {"matcher":"Bash","hooks":[{"type":"command","name":"tokenless-custom","command":"audit"}]}
    ],
    "PostToolUse": [
      {"matcher":"","hooks":[{"type":"command","name":"tokenless-compress-response","description":"Compresses tool responses and encodes to TOON format","command":"python3 $ADAPTER_DIR/common/hooks/compress_response_hook.py --agent-id qoder-cli","timeout":10000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]}
    ]
  }
}
EOF

info "native plugin install and exact legacy migration"
output="$(run_install)"
rc=$?
echo "$output" | sed 's/^/    /'
[ "$rc" -eq 0 ] && pass "installer succeeds through qodercli plugins" \
    || fail "installer failed with rc=$rc"

[ -f "$FAKE_CACHE/hooks/hooks.json" ] && pass "native hooks/hooks.json is cached" \
    || fail "native hooks file missing from cache"
[ -x "$FAKE_CACHE/hooks/run-hook.sh" ] && [ ! -L "$FAKE_CACHE/hooks/run-hook.sh" ] \
    && pass "hook dispatcher is cached as an executable file" \
    || fail "cached hook dispatcher is missing, non-executable, or still a symlink"
[ -f "$FAKE_CACHE/commands/tokenless-stats.md" ] \
    && [ ! -e "$FAKE_CACHE/commands/tokenless-stats.toml" ] \
    && grep -q 'tokenless stats summary' "$FAKE_CACHE/commands/tokenless-stats.md" \
    && pass "Markdown command replaces the legacy TOML command" \
    || fail "Qoder command layout is incorrect"

if SETTINGS="$LEGACY_SETTINGS" python3 - <<'PY'
import json, os, sys
cfg = json.load(open(os.environ['SETTINGS']))
assert cfg['theme'] == 'dark'
assert cfg['enabledPlugins'] == {'user-plugin@local': True}
assert cfg['plugins']['enabled'] == ['other@local']
entries = cfg['hooks']['PreToolUse']
assert len(entries) == 2
assert [entry['hooks'][0]['name'] for entry in entries] == [
    'tokenless-rewrite', 'tokenless-custom'
]
assert 'PostToolUse' not in cfg['hooks']
PY
then
    pass "legacy migration handles known old prefixes and preserves custom entries"
else
    fail "legacy migration altered unrelated settings"
fi

if QODER_SETTINGS="$QODER_CONFIG/settings.json" python3 - <<'PY'
import json, os
cfg = json.load(open(os.environ['QODER_SETTINGS']))
assert cfg == {'enabledPlugins': {'tokenless@local': True}}
PY
then
    pass "custom QODER_CONFIG_DIR retains only Qoder-owned native state"
else
    fail "native Qoder settings contain adapter-written fields"
fi

info "legacy migration is idempotent and rejects symlinks"
before="$(cat "$LEGACY_SETTINGS")"
python3 "$ADAPTER_DIR/qoder/scripts/migrate-legacy-settings.py" \
    --legacy-hooks-root "$ADAPTER_DIR/common/hooks" "$LEGACY_SETTINGS" >/dev/null
[ "$(cat "$LEGACY_SETTINGS")" = "$before" ] && pass "repeated migration is byte-stable" \
    || fail "repeated migration unexpectedly rewrote settings"
victim="$SANDBOX/settings-victim.json"
symlink_settings="$FAKE_HOME/.qoder/symlink-settings.json"
printf '{"theme":"victim"}\n' > "$victim"
ln -s "$victim" "$symlink_settings"
victim_before="$(cat "$victim")"
if python3 "$ADAPTER_DIR/qoder/scripts/migrate-legacy-settings.py" \
    --legacy-hooks-root "$ADAPTER_DIR/common/hooks" "$symlink_settings" >/dev/null 2>&1; then
    fail "legacy migration unexpectedly followed a settings symlink"
else
    pass "legacy migration rejects settings symlinks"
fi
[ "$(cat "$victim")" = "$victim_before" ] && pass "symlink target remains unchanged" \
    || fail "symlink target was modified"

info "unsafe legacy JSON triggers rollback"
rm -f "$FAKE_STATE"
printf '[1,2,3]\n' > "$LEGACY_SETTINGS"
before="$(cat "$LEGACY_SETTINGS")"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && pass "installer fails closed on unsafe legacy settings" \
    || fail "installer unexpectedly accepted unsafe legacy settings"
[ ! -f "$FAKE_STATE" ] && pass "failed migration rolls back the new plugin" \
    || fail "plugin remained installed after migration failure"
[ "$(cat "$LEGACY_SETTINGS")" = "$before" ] && pass "unsafe settings remain byte-for-byte unchanged" \
    || fail "unsafe settings were modified"

info "pre-existing disabled plugin is never removed by rollback"
: > "$FAKE_STATE"
printf 'disabled\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && pass "disabled native plugin fails health verification" \
    || fail "disabled native plugin unexpectedly passed verification"
[ -f "$FAKE_STATE" ] && pass "rollback preserves a pre-existing disabled plugin" \
    || fail "rollback removed a pre-existing disabled plugin"

info "Qoder PreToolUse rewrite uses only official updatedInput"
HOOK_BIN="$SANDBOX/hook-bin"
mkdir -p "$HOOK_BIN"
cat > "$HOOK_BIN/rtk" <<'EOF'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then echo 'rtk 0.43.0'; exit 0; fi
if [ "${1:-}" = "rewrite" ]; then echo "rtk ${2:-}"; exit 0; fi
exit 1
EOF
cat > "$HOOK_BIN/tokenless" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$HOOK_BIN/rtk" "$HOOK_BIN/tokenless"
rewrite_out="$(printf '%s\n' '{"tool_input":{"command":"ls -la"}}' | \
    HOME="$FAKE_HOME" PATH="$HOOK_BIN:$PATH" TOKENLESS_AGENT_ID=qoder-cli \
    bash "$FAKE_CACHE/hooks/run-hook.sh" rewrite_hook.py)"
if REWRITE_OUT="$rewrite_out" HOOK_BIN="$HOOK_BIN" python3 - <<'PY'
import json, os
output = json.loads(os.environ['REWRITE_OUT'])['hookSpecificOutput']
assert output['updatedInput']['command'] == f"{os.environ['HOOK_BIN']}/rtk ls -la"
assert 'tool_input' not in output
PY
then
    pass "cached Qoder dispatcher emits updatedInput without private patch fields"
else
    fail "Qoder rewrite output does not follow updatedInput protocol"
fi

info "already-absent native plugin still triggers legacy cleanup"
rm -f "$FAKE_STATE" "$FAKE_MODE"
cat > "$LEGACY_SETTINGS" <<EOF
{"plugins":{"enabled":["tokenless@local"]},"hooks":{"PostToolUse":[{"matcher":"","hooks":[{"type":"command","name":"tokenless-compress-response","description":"Compresses tool responses and encodes to TOON format","command":"python3 $ADAPTER_DIR/common/hooks/compress_response_hook.py","timeout":10000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]}]}}
EOF
output="$(run_uninstall)"
rc=$?
if [ "$rc" -eq 0 ] && SETTINGS="$LEGACY_SETTINGS" python3 - <<'PY'
import json, os
assert json.load(open(os.environ['SETTINGS'])) == {}
PY
then
    pass "already-absent uninstall completes exact legacy cleanup"
else
    fail "already-absent uninstall did not complete legacy cleanup: $output"
fi

info "stderr diagnostics stay separate from JSON inventory"
printf 'stderr-warning\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -eq 0 ] && [ -f "$FAKE_STATE" ] \
    && echo "$output" | grep -q 'qodercli warning' \
    && pass "stderr warnings remain visible without corrupting JSON verification" \
    || fail "stderr warning corrupted native plugin installation: $output"
rm -f "$FAKE_STATE"
output="$(run_uninstall)"
rc=$?
[ "$rc" -eq 0 ] && echo "$output" | grep -q 'qodercli warning' \
    && pass "already-absent verification tolerates stderr warnings" \
    || fail "stderr warning corrupted already-absent verification: $output"

info "missing qodercli still performs exact legacy cleanup"
cat > "$LEGACY_SETTINGS" <<EOF
{"hooks":{"PostToolUse":[{"matcher":"","hooks":[{"type":"command","name":"tokenless-compress-response","description":"Compresses tool responses and encodes to TOON format","command":"python3 $ADAPTER_DIR/common/hooks/compress_response_hook.py","timeout":10000,"env":{"TOKENLESS_AGENT_ID":"qoder-cli"}}]}]}}
EOF
NO_QODER_BIN="$SANDBOX/no-qoder-bin"
mkdir -p "$NO_QODER_BIN"
PYTHON3_BIN="$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')"
for utility in ls sort tail; do
    ln -s "$(command -v "$utility")" "$NO_QODER_BIN/$utility"
done
ln -s "$PYTHON3_BIN" "$NO_QODER_BIN/python3"
output="$(HOME="$FAKE_HOME" \
    PATH="$NO_QODER_BIN" \
    QODER_CONFIG_DIR="$QODER_CONFIG" \
    QODERCLI_BIN="$SANDBOX/missing-qodercli" \
    ANOLISA_ADAPTER_DIR="$ADAPTER_DIR" \
    /bin/bash "$UNINSTALL_SH" 2>&1)"
rc=$?
if [ "$rc" -ne 0 ] && SETTINGS="$LEGACY_SETTINGS" python3 - <<'PY'
import json, os
assert json.load(open(os.environ['SETTINGS'])) == {}
PY
then
    pass "missing qodercli leaves native state uncertain after cleaning legacy hooks"
else
    fail "missing qodercli skipped legacy cleanup or reported success: $output"
fi

info "capability and JSON inventory failures stop before mutation"
rm -f "$FAKE_STATE"
printf 'invalid-json\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && [ ! -f "$FAKE_STATE" ] \
    && pass "invalid JSON inventory is rejected before install" \
    || fail "invalid JSON inventory did not fail closed"
printf 'error-json\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && [ ! -f "$FAKE_STATE" ] \
    && echo "$output" | grep -q 'unrecognized inventory structure' \
    && pass "valid error JSON is rejected before install" \
    || fail "valid error JSON was treated as an absent plugin"
printf 'missing-install-capability\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && [ ! -f "$FAKE_STATE" ] \
    && pass "missing native plugin capability is rejected before install" \
    || fail "missing native plugin capability did not fail closed"
printf 'fail-install\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && [ ! -f "$FAKE_STATE" ] \
    && pass "native install failure is surfaced without mutation" \
    || fail "native install failure was hidden"
printf 'distractor-resources\n' > "$FAKE_MODE"
output="$(run_install)"
rc=$?
[ "$rc" -ne 0 ] && [ ! -f "$FAKE_STATE" ] \
    && pass "resource verification ignores unrelated inventory strings" \
    || fail "distractor strings falsely satisfied resource verification"

echo ""
echo "============================================"
echo "  Summary: $PASS passed, $FAIL failed"
echo "============================================"
[ "$FAIL" -gt 0 ] && exit 1
echo -e "\n${GREEN}All tests passed!${NC}"
