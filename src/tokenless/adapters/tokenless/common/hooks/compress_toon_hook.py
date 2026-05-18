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
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_CHARS = 200  # character count, not byte length
_TOKENLESS_FALLBACK = "/usr/bin/tokenless"

_SKIP_TOOLS = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "read", "glob", "notebookread",
}


# -- helpers -----------------------------------------------------------------


def _resolve_binary(name: str, fallback_path: str) -> str | None:
    path = shutil.which(name)
    if path:
        return path
    if os.path.isfile(fallback_path) and os.access(fallback_path, os.X_OK):
        return fallback_path
    return None


def _skip() -> None:
    print(json.dumps({}))
    sys.exit(0)


def _warn(msg: str) -> None:
    print(f"[tokenless] WARNING: {msg}", file=sys.stderr)


def _try_parse_json(data: str) -> object | None:
    try:
        return json.loads(data)
    except (json.JSONDecodeError, ValueError):
        return None


def _unwrap_string_json(raw: str) -> str | None:
    """If raw is a JSON-encoded string whose inner content is valid JSON,
    unwrap it into the inner JSON object."""
    if not raw.startswith('"'):
        return raw
    inner = _try_parse_json(raw)
    if isinstance(inner, str):
        inner_obj = _try_parse_json(inner)
        if inner_obj is not None and isinstance(inner_obj, (dict, list)):
            return json.dumps(inner_obj, separators=(",", ":"))
        return None  # Plain text, not JSON
    return raw


def _is_skill_file(text: str) -> bool:
    """Detect YAML frontmatter markdown (skill files) that must not be compressed."""
    if not text.startswith("---"):
        return False
    lines = text.split("\n", 20)
    for line in lines[1:]:
        if line.startswith("name:") or line.startswith("description:"):
            return True
    return False


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve tokenless binary
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        _warn("tokenless is not installed. TOON compression hook disabled.")
        _skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        _warn("failed to read PostToolUse payload. Passing through unchanged.")
        _skip()

    # 3. Skip content-retrieval tools
    tool_name = input_data.get("tool_name", "unknown")
    if tool_name in _SKIP_TOOLS:
        _skip()

    # 4. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        _skip()

    # 5. Skip skill files (YAML frontmatter)
    if isinstance(tool_response_raw, str) and _is_skill_file(tool_response_raw):
        _skip()

    # 6. Normalize: unwrap string-wrapped JSON
    if isinstance(tool_response_raw, str):
        tool_response = _unwrap_string_json(tool_response_raw)
        if tool_response is None:
            _skip()  # Plain text, not JSON
    elif isinstance(tool_response_raw, (dict, list)):
        tool_response = json.dumps(tool_response_raw, separators=(",", ":"))
    else:
        _skip()

    if not tool_response:
        _skip()

    # 7. Skip small responses (character count, not byte length)
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        _skip()

    # 8. Validate it's JSON
    parsed = _try_parse_json(tool_response)
    if parsed is None:
        _skip()

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
        _warn("TOON encoding failed. Passing through unchanged.")
        _skip()

    toon_output = proc.stdout.strip()
    if not toon_output:
        _warn("TOON encoding returned empty output. Passing through unchanged.")
        _skip()

    # 11. Size guard — skip if TOON output is not smaller
    before_chars = len(tool_response)
    after_chars = len(toon_output)
    if after_chars >= before_chars:
        _skip()

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