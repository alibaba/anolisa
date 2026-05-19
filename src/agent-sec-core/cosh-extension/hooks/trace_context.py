"""Shared trace-context helpers for cosh hook scripts."""

import json
from typing import Any


def _first_string(*values: Any) -> str | None:
    for value in values:
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def trace_context(input_data: dict[str, Any]) -> dict[str, str] | None:
    """Build canonical trace context from fields directly present on hook input."""
    context = {
        "trace_id": _first_string(
            input_data.get("trace_id"),
            input_data.get("traceId"),
        ),
        "session_id": _first_string(
            input_data.get("session_id"),
            input_data.get("sessionId"),
        ),
        "run_id": _first_string(
            input_data.get("run_id"),
            input_data.get("runId"),
        ),
        "call_id": _first_string(
            input_data.get("call_id"),
            input_data.get("callId"),
        ),
        "tool_call_id": _first_string(
            input_data.get("tool_call_id"),
            input_data.get("toolCallId"),
            input_data.get("tool_use_id"),
            input_data.get("toolUseId"),
        ),
    }
    return {key: value for key, value in context.items() if value} or None


def with_trace_context(args: list[str], input_data: dict[str, Any]) -> list[str]:
    """Prepend hidden agent-sec-cli trace-context args when hook input has tracing."""
    context = trace_context(input_data)
    if context is None:
        return args
    return [
        args[0],
        "--trace-context",
        json.dumps(context, ensure_ascii=False, separators=(",", ":")),
        *args[1:],
    ]
