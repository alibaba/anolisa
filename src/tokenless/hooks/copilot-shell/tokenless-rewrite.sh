#!/usr/bin/env bash
# tokenless-hook-version: 1
# Token-Less copilot-shell hook — rewrites commands to use rtk for token savings.
# Requires: rtk >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth. To add or change rewrite rules,
# update the RTK registry — not this file.
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through (agent handles deny)
#   3 + stdout  Ask rule matched → rewrite but let agent prompt the user
#
# copilot-shell hook protocol:
#   stdin:  JSON with { tool_input: { command: "..." }, ... }
#   stdout: JSON with { hookSpecificOutput: { tool_input: { command: "..." } } }
#   Note: copilot-shell uses `tool_input` (not `updatedInput` like Claude Code)

# --- Dependency checks (fail-open: never block commands) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Hook cannot rewrite commands." >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[tokenless] WARNING: rtk is not installed or not in PATH. Hook disabled." >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo "[tokenless] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
    exit 0
  fi
fi

# --- Read input ---

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  exit 0
fi

# --- Delegate to rtk rewrite ---

REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

case $EXIT_CODE in
  0)
    # Rewrite found, no permission rules matched — safe to auto-allow.
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ;;
  1)
    # No RTK equivalent — pass through unchanged.
    exit 0
    ;;
  2)
    # Deny rule matched — let agent's native deny handle it.
    exit 0
    ;;
  3)
    # Ask rule matched — rewrite but do NOT auto-allow.
    ;;
  *)
    exit 0
    ;;
esac

# --- Build copilot-shell response ---
# Key difference from Claude Code: use `tool_input` instead of `updatedInput`

ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')

if [ "$EXIT_CODE" -eq 3 ]; then
  # Ask: rewrite the command, omit decision so agent prompts user.
  jq -n \
    --argjson updated "$UPDATED_INPUT" \
    '{
      "hookSpecificOutput": {
        "tool_input": $updated
      }
    }'
else
  # Allow: rewrite the command and auto-allow.
  jq -n \
    --argjson updated "$UPDATED_INPUT" \
    '{
      "decision": "allow",
      "reason": "RTK auto-rewrite",
      "hookSpecificOutput": {
        "tool_input": $updated
      }
    }'
fi
