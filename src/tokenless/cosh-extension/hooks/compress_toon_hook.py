#!/usr/bin/env python3
"""Cosh hook for standalone TOON encoding.

Reads a cosh PostToolUse JSON from stdin, encodes the tool response
to TOON format via ``tokenless compress-toon``, and writes a cosh
HookOutput JSON to stdout.

This is a standalone TOON-only hook for users who want pure TOON
encoding without response compression.  The combined pipeline
(response compression + TOON) is in compress_response_hook.py.

Hook point: **PostToolUse**

This script is intentionally self-contained — it does NOT import any
tokenless package.  All it needs is the standard library and the
tokenless/toon binaries on $PATH.
"""

import json
import os
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_AGENT_ID = "copilot-shell"
_MIN_RESPONSE_LEN = 200
_TOKENLESS_FALLBACK = "/usr/bin/tokenless"
_TOON_FALLBACK = "/usr/libexec/tokenless/toon"


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


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve binaries
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        _warn("tokenless is not installed. TOON compression hook disabled.")
        _skip()

    toon_bin = _resolve_binary("toon", _TOON_FALLBACK)
    if not toon_bin:
        _warn("toon is not installed. TOON compression hook disabled.")
        _skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        _warn("failed to read PostToolUse payload. Passing through unchanged.")
        _skip()

    # 3. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        _skip()

    # 4. Normalize: unwrap string-wrapped JSON
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

    # 5. Skip small responses
    if len(tool_response) < _MIN_RESPONSE_LEN:
        _skip()

    # 6. Validate it's JSON
    parsed = _try_parse_json(tool_response)
    if parsed is None:
        _skip()

    # 7. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")
    tool_name = input_data.get("tool_name", "unknown")

    # 8. Encode to TOON via tokenless compress-toon
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

    # 9. Calculate savings metrics
    before_chars = len(tool_response)
    after_chars = len(toon_output)
    savings_pct = 0
    if before_chars > 0:
        savings_pct = (before_chars - after_chars) * 100 // before_chars

    # 10. Build response
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