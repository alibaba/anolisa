#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/../adapters/tokenless/common/hooks/tool_ready_hook.sh"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

SPEC="$TEST_DIR/tool-ready-spec.json"
FIXER="$TEST_DIR/tokenless-env-fix.sh"
MARKER="$TEST_DIR/fixer-called"

cat > "$SPEC" <<'EOF'
{"TestReady":{"required":[],"recommended":[],"permissions":[]},"TestMissing":{"required":[{"binary":"tokenless-missing-for-test","package":"tokenless-missing-for-test","manager":"rpm"}],"recommended":[],"permissions":[]}}
EOF

cat > "$FIXER" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

[ "${1:-}" = "fix-all" ]
cat >/dev/null
echo "[tokenless-env-fix] simulated fixer output"
touch "$TOKENLESS_FIX_MARKER"
EOF
chmod 0644 "$FIXER"

EMPTY_PATH="$TEST_DIR/empty-path"
mkdir "$EMPTY_PATH"
NO_JQ_OUTPUT=$(PATH="$EMPTY_PATH" /bin/bash "$HOOK" </dev/null)
[ "$NO_JQ_OUTPUT" = '{}' ]

PASSTHROUGH_OUTPUT=$(
    echo '{"tool_name":"TestReady","tool_input":{"command":"test"}}' \
        | TOKENLESS_TOOL_READY_SPEC="$SPEC" \
          TOKENLESS_ENV_FIX_SCRIPT="$FIXER" \
          bash "$HOOK"
)
[ "$PASSTHROUGH_OUTPUT" = '{}' ]

UNKNOWN_TOOL_OUTPUT=$(
    echo '{"tool_name":"UnknownTool","tool_input":{}}' \
        | TOKENLESS_TOOL_READY_SPEC="$SPEC" \
          TOKENLESS_ENV_FIX_SCRIPT="$FIXER" \
          bash "$HOOK"
)
[ "$UNKNOWN_TOOL_OUTPUT" = '{}' ]

OUTPUT=$(
    echo '{"tool_name":"TestMissing","tool_input":{"command":"test"}}' \
        | TOKENLESS_TOOL_READY_SPEC="$SPEC" \
          TOKENLESS_ENV_FIX_SCRIPT="$FIXER" \
          TOKENLESS_FIX_MARKER="$MARKER" \
          bash "$HOOK"
)

[ -f "$MARKER" ]
if ! jq -e -s 'length == 1 and (.[0] | type == "object")' \
    <<<"$OUTPUT" >/dev/null; then
    echo "expected one JSON object without fixer output, got: $OUTPUT" >&2
    exit 1
fi
grep -q "NOT_READY" <<<"$OUTPUT"
grep -q "Skip retry" <<<"$OUTPUT"

echo "tool-ready readable fixer test passed"
