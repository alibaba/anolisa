#!/usr/bin/env bash
# tokenless-hook-version: 10
# Token-Less copilot-shell hook — compresses tool schema definitions.
# Stats are recorded automatically by tokenless compress-schema.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Schema compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. Schema compression hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read BeforeModel payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract tools array ---

TOOLS=$(echo "$INPUT" | jq -c '.llm_request.tools // empty' 2>/dev/null || echo '')

if [ -z "$TOOLS" ] || [ "$TOOLS" = "null" ] || [ "$TOOLS" = "[]" ]; then
  exit 0
fi

TOOLS_LENGTH=$(echo "$TOOLS" | jq 'length' 2>/dev/null || echo '0')
if [ "$TOOLS_LENGTH" -eq 0 ]; then
  exit 0
fi

# --- Extract caller context from raw payload ---

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // .toolCallId // empty' 2>/dev/null || echo '')

# --- Compress schemas ---

COMPRESSED=$(echo "$TOOLS" | tokenless compress-schema --batch \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  ${TOOL_USE_ID:+--tool-use-id "$TOOL_USE_ID"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: Schema compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate compressed output is valid JSON array
if ! echo "$COMPRESSED" | jq -e 'type == "array"' &>/dev/null 2>&1; then
  echo "[tokenless] WARNING: Schema compression returned invalid JSON. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---

jq -n \
  --argjson tools "$COMPRESSED" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "BeforeModel",
      "llm_request": {
        "tools": $tools
      }
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
