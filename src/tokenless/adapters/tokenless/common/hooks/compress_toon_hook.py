#!/usr/bin/env python3
"""Tokenless standalone TOON encoding hook.

Reads a PostToolUse JSON from stdin, encodes the tool response
to TOON format via ``tokenless compress-toon``, and writes a
HookOutput JSON to stdout.

This is a standalone TOON-only hook for users who want pure TOON
encoding without response compression.  The combined pipeline
(response compression + TOON) is in compress_response_hook.py.

Hook point: **PostToolUse**

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import resolve_binary, skip, warn, try_parse_json, unwrap_string_json, is_skill_file, _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_CHARS = 200

_SKIP_TOOLS = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "read", "glob", "notebookread",
}


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve binaries
    tokenless_bin = resolve_binary("tokenless", _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB)
    if not tokenless_bin:
        warn("tokenless is not installed. TOON compression hook disabled.")
        skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        warn("failed to read PostToolUse payload. Passing through unchanged.")
        skip()

    # 3. Skip content-retrieval tools
    tool_name = input_data.get("tool_name", "unknown")
    if tool_name in _SKIP_TOOLS:
        skip()

    # 4. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        skip()

    # 5. Skip skill files (YAML frontmatter)
    if isinstance(tool_response_raw, str) and is_skill_file(tool_response_raw):
        skip()

    # 6. Normalize: unwrap string-wrapped JSON
    if isinstance(tool_response_raw, str):
        tool_response = unwrap_string_json(tool_response_raw)
        if tool_response is None:
            skip()  # Plain text, not JSON
    elif isinstance(tool_response_raw, (dict, list)):
        tool_response = json.dumps(tool_response_raw, separators=(",", ":"))
    else:
        skip()

    if not tool_response:
        skip()

    # 7. Skip small responses (character count, not byte length)
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        skip()

    # 8. Validate it's JSON
    parsed = try_parse_json(tool_response)
    if parsed is None:
        skip()

    # 9. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")

    # 10. Encode to TOON via tokenless compress-toon
    cmd = [tokenless_bin, "compress-toon", "--agent-id", _AGENT_ID]
    if session_id:
        cmd.extend(["--session-id", session_id])
    if tool_use_id:
        cmd.extend(["--tool-use-id", tool_use_id])

    try:
        proc = subprocess.run(
            cmd,
            input=tool_response,
            capture_output=True, text=True, timeout=10,
        )
    except Exception:
        warn("TOON encoding failed. Passing through unchanged.")
        skip()

    toon_output = proc.stdout.strip()
    if not toon_output:
        warn("TOON encoding returned empty output. Passing through unchanged.")
        skip()

    # 11. Size guard — skip if TOON output is not smaller
    before_chars = len(tool_response)
    after_chars = len(toon_output)
    if after_chars >= before_chars:
        skip()

    savings_pct = (before_chars - after_chars) * 100 // before_chars if before_chars > 0 else 0

    # 12. Build response
    context = (
        f"[tokenless] {tool_name} → TOON encoded ({savings_pct}% savings)\n"
        f"{toon_output}"
    )

    output = {
        "suppressOutput": True,
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        },
    }
    print(json.dumps(output, ensure_ascii=False))


if __name__ == "__main__":
    main()
