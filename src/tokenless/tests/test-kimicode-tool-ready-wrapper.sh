#!/usr/bin/env bash
# Regression test for kimicode tool-ready wrapper.
# Verifies that a blocking tool-ready decision is translated into Kimi Code's
# expected PreToolUse protocol (permissionDecision=deny, exit 2) and that a
# ready/pass-through decision is translated into an empty allow response.
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
WRAPPER="$SCRIPT_DIR/../adapters/tokenless/kimicode/hooks/tool-ready-kimi-wrapper.sh"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

SPEC_FILE="$TEST_DIR/tool-ready-spec.json"
cat >"$SPEC_FILE" <<'EOF'
{
  "TestReady": {
    "aliases": ["TestReady"],
    "required": [{ "binary": "bash", "package": "bash", "manager": "rpm" }],
    "recommended": []
  },
  "TestBlocked": {
    "aliases": ["TestBlocked"],
    "required": [{ "binary": "__missing_binary_42__", "package": "missing", "manager": "rpm" }],
    "recommended": []
  }
}
EOF

# --- Pass-through case: tool is ready ---
output=$(TOKENLESS_TOOL_READY_SPEC="$SPEC_FILE" bash "$WRAPPER" <<'JSON'
{"tool_name":"TestReady","tool_input":{"command":"echo ok"}}
JSON
)
[ "$output" = "{}" ]

# --- Blocking case: required dependency missing ---
STDERR_FILE="$TEST_DIR/stderr.txt"
set +e
output=$(TOKENLESS_TOOL_READY_SPEC="$SPEC_FILE" bash "$WRAPPER" 2>"$STDERR_FILE" <<'JSON'
{"tool_name":"TestBlocked","tool_input":{"command":"echo ok"}}
JSON
)
status=$?
set -e

[ "$status" -eq 2 ]
printf '%s' "$output" | jq -e '.permissionDecision == "deny"' >/dev/null
printf '%s' "$output" | jq -e '.reason != ""' >/dev/null
printf '%s' "$output" | jq -e '.hookSpecificOutput.additionalContext != ""' >/dev/null

# Verify the reason was also written to stderr (Kimi runner reads from stderr).
if ! grep -q 'tokenless' "$STDERR_FILE"; then
    echo "FAIL: stderr missing deny reason (Kimi runner needs it for Agent diagnostic)"
    exit 1
fi

echo "kimicode tool-ready wrapper test passed"
