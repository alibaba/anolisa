#!/usr/bin/env bash
# tokenless-hook-version: 3
# Token-Less copilot-shell hook — compresses JSON tool responses to TOON format.
# Requires: toon, jq
#
# Hook event: PostToolUse
# Records: timestamp, Agent(pid), sessionID, toolCallID, before/after chars & tokens, before/after text

set -euo pipefail

# --- Dependency checks (fail-open) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. TOON compression hook disabled." >&2
  exit 0
fi

if ! command -v toon &>/dev/null; then
  echo "[tokenless] WARNING: toon is not installed or not in PATH. TOON compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed. Stats recording disabled." >&2
  TOKENLESS_AVAILABLE=false
else
  TOKENLESS_AVAILABLE=true
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PostToolUse payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract tool_response ---

TOOL_RESPONSE=$(echo "$INPUT" | jq -c '.tool_response // empty' 2>/dev/null || echo '')

if [ -z "$TOOL_RESPONSE" ] || [ "$TOOL_RESPONSE" = "null" ] || [ "$TOOL_RESPONSE" = "{}" ]; then
  exit 0
fi

# If tool_response is a JSON-encoded string, unwrap it
if echo "$TOOL_RESPONSE" | jq -e 'type == "string"' &>/dev/null 2>&1; then
  UNWRAPPED=$(echo "$TOOL_RESPONSE" | jq -r '.' 2>/dev/null)
  if echo "$UNWRAPPED" | jq -e '.' &>/dev/null 2>&1; then
    TOOL_RESPONSE=$(echo "$UNWRAPPED" | jq -c '.' 2>/dev/null)
  else
    # Inner content is not valid JSON — skip plain text responses
    exit 0
  fi
fi

# --- Skip small responses ---

RESPONSE_LEN=${#TOOL_RESPONSE}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Verify it's valid JSON ---

if ! echo "$TOOL_RESPONSE" | jq -e '.' &>/dev/null 2>&1; then
  exit 0
fi

# --- Calculate before metrics ---

BEFORE_CHARS=$RESPONSE_LEN
BEFORE_TOKENS=$(( (BEFORE_CHARS + 3) / 4 ))

# --- Encode JSON to TOON ---

START_TIME=$(date +%s%3N 2>/dev/null || echo "0")

TOON_OUTPUT=$(echo "$TOOL_RESPONSE" | toon -e 2>/dev/null) || {
  echo "[tokenless] WARNING: TOON encoding failed. Passing through unchanged." >&2
  exit 0
}

END_TIME=$(date +%s%3N 2>/dev/null || echo "0")

# Validate non-empty output
if [ -z "$TOON_OUTPUT" ]; then
  echo "[tokenless] WARNING: TOON encoding returned empty output. Passing through unchanged." >&2
  exit 0
fi

# --- Calculate after metrics ---

AFTER_CHARS=${#TOON_OUTPUT}
AFTER_TOKENS=$(( (AFTER_CHARS + 3) / 4 ))

# --- Record statistics ---

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty' 2>/dev/null || echo '')
AGENT_ID="copilot-shell"
AGENT_PID=$$

if [ "$TOKENLESS_AVAILABLE" = true ]; then
  RECORD_CMD=(
    tokenless stats record
    --operation compress-toon
    --agent-id "$AGENT_ID"
    --before-chars "$BEFORE_CHARS"
    --before-tokens "$BEFORE_TOKENS"
    --after-chars "$AFTER_CHARS"
    --after-tokens "$AFTER_TOKENS"
    --pid "$AGENT_PID"
    --before-text "$TOOL_RESPONSE"
    --after-text "$TOON_OUTPUT"
  )

  [ -n "$SESSION_ID" ] && RECORD_CMD+=(--session-id "$SESSION_ID")
  [ -n "$TOOL_USE_ID" ] && RECORD_CMD+=(--tool-use-id "$TOOL_USE_ID")

  "${RECORD_CMD[@]}" 2>/dev/null || true
fi

# --- Build copilot-shell response ---

SAVINGS_PCT=0
if [ "$BEFORE_CHARS" -gt 0 ]; then
  SAVINGS_PCT=$(( (BEFORE_CHARS - AFTER_CHARS) * 100 / BEFORE_CHARS ))
fi

jq -n \
  --arg toon "$TOON_OUTPUT" \
  --arg tool "$TOOL_NAME" \
  --arg savings "$SAVINGS_PCT" \
  '{
    "suppressOutput": true,
    "hookSpecificOutput": {
      "hookEventName": "PostToolUse",
      "additionalContext": (
        "[tokenless] Tool response from " + $tool + " compressed to TOON format (" + $savings + "% token savings).\n" +
        "TOON is a compact notation for structured data. Parse it as key-value pairs and tabular data.\n\n" +
        $toon
      )
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
