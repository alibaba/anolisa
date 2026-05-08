#!/usr/bin/env python3
"""Cosh hook for command rewriting via rtk.

Reads a cosh PreToolUse JSON from stdin, extracts the shell command,
invokes ``rtk rewrite`` via subprocess, and writes a cosh HookOutput
JSON to stdout.

Hook point: **PreToolUse** — matcher: ``Shell``

This script is intentionally self-contained — it does NOT import any
tokenless package.  All it needs is the standard library and the
rtk/tokenless binaries on $PATH.
"""

import json
import os
import re
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_MIN_RTK_VERSION = (0, 35, 0)
_RTK_FALLBACK = "/usr/libexec/tokenless/rtk"
_AGENT_ID = "copilot-shell"


# -- helpers -----------------------------------------------------------------


def _resolve_binary(name: str, fallback_path: str) -> str | None:
    path = shutil.which(name)
    if path:
        return path
    if os.path.isfile(fallback_path) and os.access(fallback_path, os.X_OK):
        return fallback_path
    return None


def _parse_version(version_str: str) -> tuple | None:
    m = re.search(r"(\d+)\.(\d+)\.(\d+)", version_str)
    if m:
        return (int(m.group(1)), int(m.group(2)), int(m.group(3)))
    return None


def _skip() -> None:
    print(json.dumps({}))
    sys.exit(0)


def _warn(msg: str) -> None:
    print(f"[tokenless] WARNING: {msg}", file=sys.stderr)


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve rtk binary
    rtk_bin = _resolve_binary("rtk", _RTK_FALLBACK)
    if not rtk_bin:
        _warn("rtk is not installed or not in PATH. Hook disabled.")
        _skip()

    # 2. Version guard
    try:
        result = subprocess.run(
            [rtk_bin, "--version"],
            capture_output=True, text=True, timeout=3,
        )
        ver = _parse_version(result.stdout)
        if ver and ver < _MIN_RTK_VERSION:
            _warn(f"rtk {result.stdout.strip()} is too old (need >= 0.35.0).")
            _skip()
    except Exception:
        pass  # version check non-fatal

    # 3. Check tokenless binary (for stats)
    if not shutil.which("tokenless"):
        _warn("tokenless is not installed. Hook disabled.")
        _skip()

    # 4. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        _skip()

    # 5. Extract command
    tool_input = input_data.get("tool_input", {})
    cmd = tool_input.get("command", "")
    if not cmd:
        _skip()

    # 6. Rewrite via rtk
    env = os.environ.copy()
    env["TOKENLESS_AGENT_ID"] = _AGENT_ID
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")
    if session_id:
        env["TOKENLESS_SESSION_ID"] = session_id
    if tool_use_id:
        env["TOKENLESS_TOOL_USE_ID"] = tool_use_id

    try:
        proc = subprocess.run(
            [rtk_bin, "rewrite", cmd],
            capture_output=True, text=True, timeout=5, env=env,
        )
    except Exception:
        _skip()

    # exit 1/2 = no rewrite; exit 0 = same or rewritten
    if proc.returncode in (1, 2):
        _skip()
    rewritten = proc.stdout.strip()
    if rewritten == cmd:
        _skip()

    # 7. Build response
    updated_input = dict(tool_input)
    updated_input["command"] = rewritten

    output = {
        "decision": "allow",
        "reason": "RTK auto-rewrite",
        "hookSpecificOutput": {
            "tool_input": updated_input,
        },
    }
    print(json.dumps(output))


if __name__ == "__main__":
    main()