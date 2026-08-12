#!/usr/bin/env python3
"""Cosh hook that tells the agent which session id filters its security events.

Every sec-core hook stamps the ``session_id`` from its own hook input onto the
security events it records, and that id is the provider session UUID — not the
``COSH_SESSION_ID`` of the PTY/evidence session. Without being told, an agent
that guesses wrong gets a silently empty event list, so this hook echoes the id
back as ``UserPromptSubmit`` ``additionalContext``.

Deliberately its own process: it performs no subprocess, no network, and no
filesystem work, so it always answers well inside the hook timeout. Folding this
into ``observability_hook.py`` would put the id behind up to three three-second
``agent-sec-cli`` calls, and cosh discards a timed-out hook's stdout entirely —
a slow CLI would withhold the session id no matter how early it was computed.
"""

from __future__ import annotations

import json
import sys
from typing import Any

# Read one extra byte below to distinguish an exact-limit payload from truncation.
_MAX_PAYLOAD_SIZE = 1024 * 1024
_SESSION_CONTEXT_EVENT = "UserPromptSubmit"
# Names the id, spells out the filter it belongs to, and rules out the lookalike
# COSH_SESSION_ID, because an agent that picks the wrong one gets a silently
# empty event list rather than an error.
_SESSION_CONTEXT_TEMPLATE = (
    "Security observability context:\n"
    "security_event_session_id={session_id}\n"
    "Use this exact value when filtering agent-sec-cli security events, e.g. "
    "agent-sec-cli events --session-id '<security_event_session_id>' "
    "--output json.\n"
    "COSH_SESSION_ID identifies the PTY/evidence session and must not be used "
    "as the security-event session ID."
)


def _noop() -> str:
    """Return an empty cosh HookOutput JSON string."""
    return json.dumps({})


def _read_stdin_payload() -> str | bytes | None:
    """Read one bounded hook payload, returning None when it exceeds the limit."""
    stream = getattr(sys.stdin, "buffer", sys.stdin)
    payload = stream.read(_MAX_PAYLOAD_SIZE + 1)
    if len(payload) > _MAX_PAYLOAD_SIZE:
        return None
    return payload


def _session_context(input_data: Any) -> str | None:
    """Return the agent-visible security session context, or None when absent.

    Only ``UserPromptSubmit`` output reaches the model as ``additionalContext``,
    and only a non-blank string id is usable as an ``agent-sec-cli`` filter, so
    every other event or session-id shape yields no context at all.
    """
    if not isinstance(input_data, dict):
        return None
    if input_data.get("hook_event_name") != _SESSION_CONTEXT_EVENT:
        return None
    session_id = input_data.get("session_id")
    if not isinstance(session_id, str) or not session_id.strip():
        return None
    # JSON string quoting keeps the id on one line and escapes quotes, control
    # characters, and non-ASCII, so a hostile id cannot forge extra context
    # lines or break out of the key=value shape.
    return _SESSION_CONTEXT_TEMPLATE.format(session_id=json.dumps(session_id.strip()))


def _hook_output(input_data: Any) -> str:
    """Render the cosh HookOutput JSON string for this hook input.

    Never raises: cosh counts a hook that emits no parseable output as a
    failure, so an unexpected input shape must still yield an empty HookOutput.
    """
    try:
        context = _session_context(input_data)
    except Exception:
        return _noop()
    if context is None:
        return _noop()
    return json.dumps(
        {
            "hookSpecificOutput": {
                "hookEventName": _SESSION_CONTEXT_EVENT,
                "additionalContext": context,
            }
        },
        ensure_ascii=False,
    )


def main() -> None:
    try:
        payload = _read_stdin_payload()
        if payload is None:
            print(_noop())
            return
        input_data = json.loads(payload)
    except (json.JSONDecodeError, EOFError, OSError, ValueError):
        print(_noop())
        return

    print(_hook_output(input_data))


if __name__ == "__main__":
    main()
