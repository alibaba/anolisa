#!/usr/bin/env python3
"""Tokenless command rewriting hook via rtk.

Reads a PreToolUse JSON from stdin, extracts the shell command,
invokes ``rtk rewrite`` via subprocess, and writes a HookOutput
JSON to stdout.

Hook point: **PreToolUse** — matcher: ``Shell``

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  Fallback paths follow the ANOLISA
FHS spec: /usr/libexec/anolisa/tokenless/rtk.
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import (
    _RTK_FALLBACK,
    _RTK_LOCAL_LIB,
    _RTK_LOCAL_SHARE,
    _TOKENLESS_FALLBACK,
    _TOKENLESS_LOCAL_LIB,
    _TOKENLESS_LOCAL_SHARE,
    forward_stderr,
    parse_version,
    resolve_binary,
    resolve_tool_call_id,
    skip,
    warn,
    write_context,
)

# -- constants ---------------------------------------------------------------

_MIN_RTK_VERSION = (0, 35, 0)
_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve rtk binary
    rtk_bin = resolve_binary(
        "rtk", _RTK_FALLBACK, _RTK_LOCAL_SHARE, _RTK_LOCAL_LIB
    )
    if not rtk_bin:
        warn("rtk is not installed or not in PATH. Hook disabled.")
        skip()

    # 2. Version guard
    try:
        result = subprocess.run(
            [rtk_bin, "--version"],
            capture_output=True,
            text=True,
            timeout=3,
        )
        ver = parse_version(result.stdout)
        if ver and ver < _MIN_RTK_VERSION:
            warn(f"rtk {result.stdout.strip()} is too old (need >= 0.35.0).")
            skip()
    except Exception as e:
        warn(f"rtk version check failed: {e}")

    # 3. Check tokenless binary (for stats)
    if not resolve_binary(
        "tokenless",
        _TOKENLESS_FALLBACK,
        _TOKENLESS_LOCAL_SHARE,
        _TOKENLESS_LOCAL_LIB,
    ):
        warn("tokenless is not installed. Hook disabled.")
        skip()

    # 4. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        skip()

    # 5. Extract command
    tool_input = input_data.get("tool_input", {})
    cmd = tool_input.get("command", "")
    if not cmd:
        skip()

    # 6. Rewrite via rtk
    env = os.environ.copy()
    env["TOKENLESS_AGENT_ID"] = _AGENT_ID
    session_id = input_data.get("session_id", "")
    tool_use_id = resolve_tool_call_id(_AGENT_ID, input_data)
    if session_id:
        env["TOKENLESS_SESSION_ID"] = session_id
    if tool_use_id:
        env["TOKENLESS_TOOL_USE_ID"] = tool_use_id

    write_context(_AGENT_ID, session_id, tool_use_id)

    try:
        proc = subprocess.run(
            [rtk_bin, "rewrite", cmd],
            capture_output=True,
            text=True,
            timeout=5,
            env=env,
        )
    except Exception as e:
        warn(f"rtk rewrite subprocess failed: {e}")
        skip()

    # Exit code protocol (from rtk rewrite_cmd.rs):
    #   0 = rewrite available, Allow verdict (auto-allow by permission rule)
    #   1 = no RTK equivalent (passthrough)
    #   2 = deny rule matched (let hook handle)
    #   3 = Ask/Default verdict (rewrite available but permission model requires
    #       user confirmation; in non-interactive hook context, treat as valid
    #       rewrite since the intent is token optimization, not permission gating)
    if proc.returncode not in (0, 1, 2, 3):
        forward_stderr(proc)
        warn(f"rtk rewrite exited with unexpected code {proc.returncode}")
        skip()
    if proc.returncode in (1, 2):
        skip()
    rewritten = proc.stdout.strip()
    if not rewritten or rewritten == cmd:
        skip()

    # 7. Build response
    updated_input = dict(tool_input)
    updated_input["command"] = rewritten

    output = {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": updated_input,
        },
    }
    print(json.dumps(output))


if __name__ == "__main__":
    main()
