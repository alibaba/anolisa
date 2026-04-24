#!/usr/bin/env bash
# tokenless-hook-version: 6
# Token-Less copilot-shell hook — rewrites commands via rtk.
# Requires: rtk >= 0.35.0, jq
#
# Hook event: PreToolUse

# --- Dependency checks (fail-open) ---

if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. Hook cannot rewrite commands." >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[tokenless] WARNING: rtk is not installed or not in PATH. Hook disabled." >&2
  exit 0
fi

# Version guard
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 35 ]; then
    echo "[tokenless] WARNING: rtk $RTK_VERSION is too old (need >= 0.35.0)." >&2
    exit 0
  fi
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed. Hook disabled." >&2
  exit 0
fi

# --- Read input ---

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  exit 0
fi

# --- Use rtk to rewrite ---

REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
REWRITE_EXIT=$?

# Handle rewrite result: exit 1/2 = no rewrite, exit 0 = same or rewritten
case $REWRITE_EXIT in
  1|2) exit 0 ;;
  0) [ "$CMD" = "$REWRITTEN" ] && exit 0 ;;
esac

# --- Extract caller context and export for RTK stats ---
# Environment variables are inherited by child processes (rtk)
# and read by RTK's record_to_tokenless_stats (takes precedence over file).

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')
TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty' 2>/dev/null || echo '')

export TOKENLESS_AGENT_ID="copilot-shell"
if [ -n "$SESSION_ID" ]; then export TOKENLESS_SESSION_ID="$SESSION_ID"; fi
if [ -n "$TOOL_USE_ID" ]; then export TOKENLESS_TOOL_USE_ID="$TOOL_USE_ID"; fi

# --- Build copilot-shell response ---

ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')

jq -n \
  --argjson updated "$UPDATED_INPUT" \
  '{
    "decision": "allow",
    "reason": "RTK auto-rewrite",
    "hookSpecificOutput": {
      "tool_input": $updated
    }
  }'
