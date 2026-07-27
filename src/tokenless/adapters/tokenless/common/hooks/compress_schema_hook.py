#!/usr/bin/env python3
"""Tokenless schema compression hook.

Reads a BeforeModel JSON from stdin, extracts the tools array,
invokes ``tokenless compress-schema --batch`` via subprocess, and
writes a HookOutput JSON to stdout.

Hook point: **BeforeModel**

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import (
    _TOKENLESS_FALLBACK,
    _TOKENLESS_LOCAL_LIB,
    _TOKENLESS_LOCAL_SHARE,
    resolve_binary,
    resolve_tool_call_id,
    skip,
    warn,
)

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")


# -- helpers -----------------------------------------------------------------


def _is_json_array(data: str) -> bool:
    try:
        obj = json.loads(data)
        return isinstance(obj, list)
    except (json.JSONDecodeError, ValueError):
        return False


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Check tokenless binary
    tokenless_bin = resolve_binary(
        "tokenless",
        _TOKENLESS_FALLBACK,
        _TOKENLESS_LOCAL_SHARE,
        _TOKENLESS_LOCAL_LIB,
    )
    if not tokenless_bin:
        warn(
            "tokenless is not installed or not in PATH. Schema compression hook disabled."
        )
        skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        warn("failed to read BeforeModel payload. Passing through unchanged.")
        skip()

    # 3. Extract tools array
    llm_request = input_data.get("llm_request", {})
    tools = llm_request.get("tools")
    if not tools:
        skip()

    tools_json = json.dumps(tools, separators=(",", ":"))

    # 4. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = resolve_tool_call_id(_AGENT_ID, input_data)

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
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        warn("Schema compression subprocess failed. Passing through unchanged.")
        skip()

    if proc.returncode != 0:
        detail = (proc.stderr or "").strip()[:200]
        warn(
            f"Schema compression failed with exit code {proc.returncode}: {detail}"
            if detail
            else f"Schema compression failed with exit code {proc.returncode}. Passing through unchanged."
        )
        skip()

    compressed = proc.stdout.strip()
    if not compressed or not _is_json_array(compressed):
        warn(
            "Schema compression returned invalid JSON. Passing through unchanged."
        )
        skip()

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
