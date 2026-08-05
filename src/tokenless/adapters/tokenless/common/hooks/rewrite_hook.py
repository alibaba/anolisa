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
import shlex
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
    resolve_agent_id,
    resolve_binary,
    resolve_tool_call_id,
    skip,
    warn,
    write_context,
)

# -- constants ---------------------------------------------------------------

_MIN_RTK_VERSION = (0, 35, 0)
_AGENT_ID = resolve_agent_id()

# Shell connectives that terminate a command-list / pipeline segment.
# A bare `rtk` wrapper can appear at command start or right after one.
_SEGMENT_OPS = frozenset({"&&", "||", ";", "|", "&"})


def _is_env_assignment(token: str) -> bool:
    """Return True for a leading shell variable assignment (NAME=value)."""
    name, sep, _ = token.partition("=")
    if not sep or not name:
        return False
    if not (name[0].isalpha() or name[0] == "_"):
        return False
    return all(c.isalnum() or c == "_" for c in name)


def _anchor_rtk_prefix(rewritten: str, rtk_bin: str) -> str:
    """Replace bare `rtk` wrapper tokens with the resolved binary path.

    rtk prints rewrites with a literal `rtk` prefix, which only resolves
    when the shell executing the tool call has the rtk location on its
    PATH. Agent runtimes with a trimmed PATH (e.g. IDE tool environments
    without ~/.local/bin) would fail every rewritten command with exit
    127 even though the hook resolved rtk successfully. Anchoring makes
    the rewritten command self-contained.

    The pass swaps the first unquoted `rtk` token of each segment — at
    command start or right after `&&` / `||` / `;` / `|` / `&`,
    optionally behind leading environment assignments or wrappers such
    as `sudo`. It lexes with `posix=False` and no punctuation splitting,
    so every token keeps its original surface: quoted patterns like
    `grep -E 'foo|rtk bar'` are never modified, unquoted globs like
    `*.txt` are not re-quoted into literals, fd redirections (`2>&1`,
    `2>/dev/null`) and command substitutions (`$(date)`) stay intact,
    and comment stripping is disabled so a `#` argument cannot truncate
    the command. Connectives are recognized as whitespace-delimited
    tokens; an unspaced `cmd1;cmd2` is not split, which only leaves
    that segment's `rtk` unanchored (conservative, never corrupts).
    Unparseable input is returned untouched.

    Known limitation: a bare `rtk` token that is another command's
    argument (e.g. `echo rtk done`) is indistinguishable from a wrapper
    position inside the rtk-rewrite output domain and gets anchored.
    Real rtk rewrites do not produce that shape, so this is accepted.
    """
    lexer = shlex.shlex(rewritten, posix=False)
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        tokens = list(lexer)
    except ValueError:
        return rewritten

    quoted = shlex.quote(rtk_bin)
    result = list(tokens)
    wrapped = False
    for i, token in enumerate(tokens):
        if token in _SEGMENT_OPS:
            wrapped = False
            continue
        if _is_env_assignment(token):
            continue
        if not wrapped and token == "rtk":
            result[i] = quoted
            wrapped = True

    return " ".join(result)


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

    rewritten = _anchor_rtk_prefix(rewritten, rtk_bin)

    # 7. Build response
    # Emit both formats for runtime compatibility:
    # - ``tool_input``: Cosh-NG partial patch (merges with original params)
    # - ``updatedInput``: copilot-shell full replacement (legacy)
    updated_input = dict(tool_input)
    updated_input["command"] = rewritten

    output = {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "tool_input": {"command": rewritten},
            "updatedInput": updated_input,
        },
    }
    print(json.dumps(output))


if __name__ == "__main__":
    main()
