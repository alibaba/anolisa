#!/usr/bin/env bash
# tokenless-hook-version: 1
# Token-Less copilot-shell hook — compresses tool schema definitions to save ~57% schema tokens.
# Requires: tokenless, jq
#
# Hook event: BeforeModel
#
# The BeforeModel event exposes `llm_request` which contains the full LLM request.
# However, as of the current anolisa copilot-shell protocol, the decoupled
# LLMRequest type does NOT include a `tools` array (tool schema definitions).
# It only exposes: model, messages, config, and toolConfig (mode + allowedFunctionNames).
#
# LIMITATION: Schema compression cannot be performed via hooks until the
# anolisa hook protocol is extended to include tool definitions in LLMRequest.
# This script is a placeholder that will activate automatically once the
# protocol adds tools support. Until then it exits cleanly (no-op).
#
# When tools support is added, the expected LLMRequest.tools format would be:
#   "tools": [ { "name": "...", "description": "...", "parameters": { ... } }, ... ]
#
# copilot-shell settings.json configuration:
#   {
#     "hooks": {
#       "BeforeModel": [
#         {
#           "type": "command",
#           "command": "/path/to/Token-Less/hooks/copilot-shell/tokenless-compress-schema.sh",
#           "name": "tokenless-compress-schema",
#           "description": "Compress tool schema definitions to save tokens",
#           "timeout": 10000
#         }
#       ]
#     }
#   }
#
# copilot-shell hook protocol:
#   stdin:  JSON with { llm_request: { model, messages, config, toolConfig, tools? } }
#   stdout: JSON with { hookSpecificOutput: { llm_request: { tools: [...] } } }

set -euo pipefail

# --- Dependency checks (fail-open: never block LLM calls) ---

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

# --- Extract tools array from llm_request (fail-open) ---
# The tools field is not yet part of the decoupled LLMRequest protocol.
# Check if it exists; if not, exit cleanly (no-op).

TOOLS=$(echo "$INPUT" | jq -c '.llm_request.tools // empty' 2>/dev/null || echo '')

if [ -z "$TOOLS" ] || [ "$TOOLS" = "null" ] || [ "$TOOLS" = "[]" ]; then
  # No tools in the request — nothing to compress.
  # This is the expected path until the protocol adds tools support.
  exit 0
fi

# --- Validate tools is an array ---

TOOLS_LENGTH=$(echo "$TOOLS" | jq 'length' 2>/dev/null || echo '0')
if [ "$TOOLS_LENGTH" -eq 0 ]; then
  exit 0
fi

# --- Compress schemas via tokenless ---

COMPRESSED=$(echo "$TOOLS" | tokenless compress-schema --batch 2>/dev/null) || {
  echo "[tokenless] WARNING: Schema compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate compressed output is valid JSON array
if ! echo "$COMPRESSED" | jq -e 'type == "array"' &>/dev/null 2>&1; then
  echo "[tokenless] WARNING: Schema compression returned invalid JSON. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
# Return a partial llm_request with only the tools field modified.

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
