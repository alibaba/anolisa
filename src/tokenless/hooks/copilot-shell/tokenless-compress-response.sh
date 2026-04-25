#!/usr/bin/env bash
# tokenless-hook-version: 6
# Token-Less copilot-shell hook — compresses tool call responses.
# Stats are recorded automatically by tokenless compress-response.
# Requires: jq
#
# Hook event: PostToolUse
#
# Compresses the tool_response field from PostToolUse events using
# `tokenless compress-response`. The compressed output replaces the original
# verbose response via suppressOutput + additionalContext.
#
# copilot-shell settings.json configuration:
#   {
#     "hooks": {
#       "PostToolUse": [
#         {
#           "type": "command",
#           "command": "/path/to/Token-Less/hooks/copilot-shell/tokenless-compress-response.sh",
#           "name": "tokenless-compress-response",
#           "description": "Compress tool responses to save tokens",
#           "timeout": 10000
#         }
#       ]
#     }
#   }
#
# copilot-shell hook protocol:
#   stdin:  JSON with { tool_name, tool_input, tool_response, ... }
#   stdout: JSON with { suppressOutput: true, hookSpecificOutput: { additionalContext: "..." } }
#
# Design: fail-open — if compression fails or dependencies are missing,
# the original response passes through unchanged.

set -euo pipefail

# --- Dependency checks (fail-open: never block tool responses) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Response compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. Response compression hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---

INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PostToolUse payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract tool_response (fail-open) ---

TOOL_RESPONSE=$(echo "$INPUT" | jq -c '.tool_response // empty' 2>/dev/null || echo '')

if [ -z "$TOOL_RESPONSE" ] || [ "$TOOL_RESPONSE" = "null" ] || [ "$TOOL_RESPONSE" = "{}" ]; then
  exit 0
fi

# --- Skip content-retrieval tools ---
# Tools that return content the agent explicitly requested must not be compressed
# because truncation would make the content incomplete and unusable.
SKIP_TOOLS="Read read_file Glob list_directory NotebookRead read glob notebookread"
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')

if [ "$TOOL_NAME" != "unknown" ] && echo "$SKIP_TOOLS" | grep -qw "$TOOL_NAME"; then
  exit 0
fi

# --- Skip small responses ---

RESPONSE_LEN=${#TOOL_RESPONSE}
if [ "$RESPONSE_LEN" -lt 200 ]; then
  exit 0
fi

# --- Skip skill files (YAML frontmatter markdown) ---
# Skill files (.md with YAML frontmatter) must not be compressed because
# truncation would break the skill metadata and make agent skills unusable.
# Detection failure is intentionally non-fatal (fail-open): if detection
# fails, we continue to compression rather than blocking the response.
TOOL_RESPONSE_RAW=$(echo "$INPUT" | jq -r '.tool_response // empty' 2>/dev/null || echo '')
if [ -n "$TOOL_RESPONSE_RAW" ]; then
  case "$TOOL_RESPONSE_RAW" in
    ---*)
      # Extract first 20 lines and check for typical skill metadata fields
      if echo "$TOOL_RESPONSE_RAW" | head -n 20 | grep -qE '^(name|description):'; then
        exit 0
      fi
      ;;
  esac
fi

# --- Compress response via tokenless ---
# tool_response may not be valid JSON in all cases (e.g., shell output wrapped as string).
# tokenless compress-response expects JSON input; if it fails, fall through gracefully.

COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response 2>/dev/null) || {
  echo "[tokenless] WARNING: Response compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate compressed output is non-empty
if [ -z "$COMPRESSED" ]; then
  echo "[tokenless] WARNING: Response compression returned empty output. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
# suppressOutput: true  — hides the original verbose tool output from the agent
# additionalContext      — injects the compressed content instead

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null || echo 'unknown')

jq -n \
  --arg context "$COMPRESSED" \
  --arg tool "$TOOL_NAME" \
  '{
    "suppressOutput": true,
    "hookSpecificOutput": {
      "hookEventName": "PostToolUse",
      "additionalContext": ("[tokenless] compressed response from " + $tool + ":\n" + $context)
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
