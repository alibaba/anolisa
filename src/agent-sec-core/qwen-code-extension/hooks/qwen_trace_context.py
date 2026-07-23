"""Shared trace-context helpers for Qwen Code policy and observability hooks."""

import json
import os
from typing import Any

_MAX_CORRELATION_ID_LENGTH = 256


def _correlation_value(value: Any) -> str | None:
    """Return one bounded correlation identifier from trusted hook fields."""
    if not isinstance(value, str):
        return None
    normalized = value.strip()
    if not normalized:
        return None
    return normalized[:_MAX_CORRELATION_ID_LENGTH]


def trace_context(input_data: dict[str, Any]) -> dict[str, str]:
    """Build canonical CLI trace context from Qwen Code hook identifiers."""
    context: dict[str, str] = {"agent_name": "qwen-code"}

    field_values = {
        "trace_id": input_data.get("trace_id"),
        "session_id": input_data.get("session_id")
        or os.environ.get("QWEN_CODE_SESSION_ID"),
        "run_id": input_data.get("run_id"),
        "call_id": input_data.get("call_id"),
        "tool_call_id": input_data.get("tool_call_id") or input_data.get("tool_use_id"),
    }
    for field_name, value in field_values.items():
        normalized = _correlation_value(value)
        if normalized is not None:
            context[field_name] = normalized
    return context


def with_trace_context(args: list[str], input_data: dict[str, Any]) -> list[str]:
    """Prepend hidden agent-sec-cli trace-context args to one command."""
    return [
        args[0],
        "--trace-context",
        json.dumps(
            trace_context(input_data), ensure_ascii=False, separators=(",", ":")
        ),
        *args[1:],
    ]
