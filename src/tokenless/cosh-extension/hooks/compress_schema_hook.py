#!/usr/bin/env python3
"""Cosh hook for schema compression.

Reads a cosh BeforeModel JSON from stdin, extracts the tools array,
invokes ``tokenless compress-schema --batch`` via subprocess, and
writes a cosh HookOutput JSON to stdout.

Hook point: **BeforeModel**

This script is intentionally self-contained — it does NOT import any
tokenless package.  All it needs is the standard library and the
tokenless binary on $PATH.
"""

import json
import os
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_AGENT_ID = "copilot-shell"


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


def _is_json_array(data: str) -> bool:
    try:
        obj = json.loads(data)
        return isinstance(obj, list)
    except (json.JSONDecodeError, ValueError):
        return False


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Check tokenless binary
    tokenless_bin = _resolve_binary("tokenless", "/usr/bin/tokenless")
    if not tokenless_bin:
        _warn("tokenless is not installed or not in PATH. Schema compression hook disabled.")
        _skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        _warn("failed to read BeforeModel payload. Passing through unchanged.")
        _skip()

    # 3. Extract tools array
    llm_request = input_data.get("llm_request", {})
    tools = llm_request.get("tools")
    if not tools:
        _skip()

    tools_json = json.dumps(tools, separators=(",", ":"))

    # 4. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")

    # 5. Compress schemas via tokenless compress-schema --batch
    cmd = [tokenless_bin, "compress-schema", "--batch", "--agent-id", _AGENT_ID]
    if session_id:
        cmd.extend(["--session-id", session_id])
    if tool_use_id:
        cmd.extend(["--tool-use-id", tool_use_id])

    try:
        proc = subprocess.run(
            cmd,
            input=tools_json,
            capture_output=True, text=True, timeout=10,
        )
    except Exception:
        _warn("Schema compression failed. Passing through unchanged.")
        _skip()

    compressed = proc.stdout.strip()
    if not compressed or not _is_json_array(compressed):
        _warn("Schema compression returned invalid JSON. Passing through unchanged.")
        _skip()

    # 6. Build response
    output = {
        "hookSpecificOutput": {
            "hookEventName": "BeforeModel",
            "llm_request": {
                "tools": json.loads(compressed),
            },
        },
    }
    print(json.dumps(output))


if __name__ == "__main__":
    main()