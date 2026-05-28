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
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import resolve_binary, skip, warn, _RTK_FALLBACK, _RTK_LOCAL_SHARE, _RTK_LOCAL_LIB, _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB

# -- constants ---------------------------------------------------------------

_MIN_RTK_VERSION = (0, 35, 0)
_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")

_CONTEXT_DIR = os.path.join(os.path.expanduser("~"), ".tokenless")
_CONTEXT_FILE = os.path.join(_CONTEXT_DIR, ".rewrite-context")


# -- helpers -----------------------------------------------------------------


def _parse_version(version_str: str) -> tuple | None:
    m = re.search(r"(\d+)\.(\d+)\.(\d+)", version_str)
    if m:
        return (int(m.group(1)), int(m.group(2)), int(m.group(3)))
    return None


def _write_context(agent_id: str, session_id: str, tool_use_id: str) -> None:
    os.makedirs(_CONTEXT_DIR, mode=0o700, exist_ok=True)
    if os.path.islink(_CONTEXT_FILE):
        os.unlink(_CONTEXT_FILE)
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(_CONTEXT_FILE, flags, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(f"{agent_id}\n")
        f.write(f"{session_id}\n")
        f.write(f"{tool_use_id}\n")


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve rtk binary
    rtk_bin = resolve_binary("rtk", _RTK_FALLBACK, _RTK_LOCAL_SHARE, _RTK_LOCAL_LIB)
    if not rtk_bin:
        warn("rtk is not installed or not in PATH. Hook disabled.")
        skip()

    # 2. Version guard
    try:
        result = subprocess.run(
            [rtk_bin, "--version"],
            capture_output=True, text=True, timeout=3,
        )
        ver = _parse_version(result.stdout)
        if ver and ver < _MIN_RTK_VERSION:
            warn(f"rtk {result.stdout.strip()} is too old (need >= 0.35.0).")
            skip()
    except Exception:
        pass  # version check non-fatal

    # 3. Check tokenless binary (for stats)
    if not resolve_binary("tokenless", _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB):
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
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")
    if session_id:
        env["TOKENLESS_SESSION_ID"] = session_id
    if tool_use_id:
        env["TOKENLESS_TOOL_USE_ID"] = tool_use_id

    _write_context(_AGENT_ID, session_id, tool_use_id)

    try:
        proc = subprocess.run(
            [rtk_bin, "rewrite", cmd],
            capture_output=True, text=True, timeout=5, env=env,
        )
    except Exception:
        skip()

    # Exit code protocol (from rtk rewrite_cmd.rs):
    #   0 = rewrite available, Allow verdict (auto-allow by permission rule)
    #   1 = no RTK equivalent (passthrough)
    #   2 = deny rule matched (let hook handle)
    #   3 = Ask/Default verdict (rewrite available but permission model requires
    #       user confirmation; in non-interactive hook context, treat as valid
    #       rewrite since the intent is token optimization, not permission gating)
    if proc.returncode in (1, 2):
        skip()
    rewritten = proc.stdout.strip()
    if rewritten == cmd:
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
