#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — compresses tool call responses,
# then optionally re-encodes to TOON format for additional token savings.
#
# Pipeline: Response Compression → TOON Encoding (if JSON)
#   1. Strip debug fields, nulls, empty values; truncate long strings/arrays
#   2. If the compressed result is still valid JSON, encode to TOON format
#   3. Stats are recorded automatically by tokenless compress-response.
#
# Requires: tokenless, jq, toon (optional — TOON step disabled if missing)
#
# Hook event: PostToolUse

set -euo pipefail

# --- Dependency checks (fail-open) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Response compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed. Response compression hook disabled." >&2
  exit 0
fi

TOON_AVAILABLE=false
if command -v toon &>/dev/null; then
  TOON_AVAILABLE=true
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PostToolUse payload. Passing through unchanged." >&2
  exit 0
})

# --- Skip content-retrieval tools ---
# Tools that return content the agent explicitly requested must not be compressed
# because truncation would make the content incomplete and unusable.
SKIP_TOOLS="Read read_file Glob list_directory NotebookRead read glob notebookread"
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')

if [ "$TOOL_NAME" != "unknown" ] && echo "$SKIP_TOOLS" | grep -qw "$TOOL_NAME"; then
  exit 0
fi

# --- Extract tool_response ---

TOOL_RESPONSE_RAW=$(echo "$INPUT" | jq -c '.tool_response // empty' 2>/dev/null || echo '')

if [ -z "$TOOL_RESPONSE_RAW" ] || [ "$TOOL_RESPONSE_RAW" = "null" ] || [ "$TOOL_RESPONSE_RAW" = "{}" ]; then
  exit 0
fi
# --- Skip small responses ---

RESPONSE_LEN=${#TOOL_RESPONSE_RAW}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Skip skill files (YAML frontmatter markdown) ---
# Skill files (.md with YAML frontmatter) must not be compressed because
# truncation would break the skill metadata and make agent skills unusable.
# Detection failure is intentionally non-fatal (fail-open): if detection
# fails, we continue to compression rather than blocking the response.
SKILL_CHECK_RAW=$(echo "$INPUT" | jq -r '.tool_response // empty' 2>/dev/null || echo '')
if [ -n "$SKILL_CHECK_RAW" ]; then
  case "$SKILL_CHECK_RAW" in
    ---*)
      if echo "$SKILL_CHECK_RAW" | head -n 20 | grep -qE '^(name|description):'; then
        exit 0
      fi
      ;;
  esac
fi

# --- Unwrap string-wrapped JSON ---
# tool_response may be a JSON string (escaped shell output) or a raw JSON object.
# If it's a string whose inner content is JSON, parse it into an object for compression.
FIRST_CHAR=$(echo "$TOOL_RESPONSE_RAW" | head -c 1)
if [ "$FIRST_CHAR" = '"' ]; then
  # It's a JSON string — extract inner content
  INNER=$(echo "$TOOL_RESPONSE_RAW" | jq -r '.' 2>/dev/null || echo '')
  # Check if the inner content is valid JSON
  if echo "$INNER" | jq -e '.' &>/dev/null 2>&1; then
    TOOL_RESPONSE=$(echo "$INNER" | jq -c '.' 2>/dev/null || echo '')
  else
    # Not JSON inner content — keep the original JSON string for TOON
    TOOL_RESPONSE="$TOOL_RESPONSE_RAW"
  fi
else
  TOOL_RESPONSE="$TOOL_RESPONSE_RAW"
fi

if [ -z "$TOOL_RESPONSE" ] || [ "$TOOL_RESPONSE" = "null" ] || [ "$TOOL_RESPONSE" = "{}" ]; then
  exit 0
fi

# Recalculate length after unwrapping
RESPONSE_LEN=${#TOOL_RESPONSE}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Step 1: Response Compression ---
# tokenless compress-response expects a JSON object/array. If TOOL_RESPONSE
# is a JSON string (plain text), compression will fail — we skip to TOON.

COMPRESSED=""
USED_RESP_COMPRESSION=false
if echo "$TOOL_RESPONSE" | jq -e 'type == "object" or type == "array"' &>/dev/null 2>&1; then
  COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response 2>/dev/null) || true
  if [ -n "$COMPRESSED" ]; then
    USED_RESP_COMPRESSION=true
  fi
fi

# If response compression was skipped or failed, use original for TOON
if [ -z "$COMPRESSED" ]; then
  COMPRESSED="$TOOL_RESPONSE"
fi

# Calculate savings after response compression
AFTER_RESP_CHARS=${#COMPRESSED}

# --- Step 2: TOON Encoding (if compressed result is valid JSON) ---

TOON_OUTPUT=""
if [ "$TOON_AVAILABLE" = true ]; then
  IS_JSON=$(echo "$COMPRESSED" | jq -e '.' &>/dev/null && echo "yes" || echo "no")
  if [ "$IS_JSON" = "yes" ]; then
    TOON_OUTPUT=$(echo "$COMPRESSED" | toon -e 2>/dev/null) || true
    if [ -n "$TOON_OUTPUT" ]; then
      AFTER_TOON_CHARS=${#TOON_OUTPUT}
      if [ "$USED_RESP_COMPRESSION" = true ]; then
        SAVINGS_LABEL="response compressed + TOON encoded"
      else
        SAVINGS_LABEL="TOON encoded"
      fi
    fi
  fi
fi

# Set label if TOON didn't produce output
if [ -z "${SAVINGS_LABEL:-}" ]; then
  if [ "$USED_RESP_COMPRESSION" = true ]; then
    SAVINGS_LABEL="response compressed"
  else
    SAVINGS_LABEL="passed through"
  fi
fi

# Determine final output and metrics
if [ -n "$TOON_OUTPUT" ]; then
  FINAL_OUTPUT="$TOON_OUTPUT"
  AFTER_CHARS=$AFTER_TOON_CHARS
else
  FINAL_OUTPUT="$COMPRESSED"
  AFTER_CHARS=$AFTER_RESP_CHARS
fi

# --- Calculate combined savings ---

BEFORE_CHARS=$RESPONSE_LEN
BEFORE_TOKENS=$(( (BEFORE_CHARS + 3) / 4 ))
AFTER_TOKENS=$(( (AFTER_CHARS + 3) / 4 ))

# --- Record statistics ---

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty' 2>/dev/null || echo '')
AGENT_ID="copilot-shell"
AGENT_PID=$$

if command -v tokenless &>/dev/null; then
  RECORD_CMD=(
    tokenless stats record
    --operation compress-response
    --agent-id "$AGENT_ID"
    --before-chars "$BEFORE_CHARS"
    --before-tokens "$BEFORE_TOKENS"
    --after-chars "$AFTER_CHARS"
    --after-tokens "$AFTER_TOKENS"
    --pid "$AGENT_PID"
    --before-text "$TOOL_RESPONSE"
    --after-text "$FINAL_OUTPUT"
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
  --arg context "$FINAL_OUTPUT" \
  --arg tool "$TOOL_NAME" \
  --arg savings "$SAVINGS_PCT" \
  --arg label "$SAVINGS_LABEL" \
  '{
    "suppressOutput": true,
    "hookSpecificOutput": {
      "hookEventName": "PostToolUse",
      "additionalContext": (
        "[tokenless] Tool response from " + $tool + " " + $label + " (" + $savings + "% token savings).\n" +
        (if $label == "response compressed + TOON encoded" then
          "TOON is a compact notation for structured data. Parse it as key-value pairs and tabular data.\n\n"
        else
          ""
        end) +
        $context
      )
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
