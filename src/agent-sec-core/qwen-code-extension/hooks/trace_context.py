"""Shared trace-context helpers for Qwen Code hook scripts."""

import json
from typing import Any

# Canonical output fields and the hook-input keys that map to each.
# Aliases are tried in order; the first present, non-empty value wins.
_FIELD_ALIASES = {
    "trace_id": ("trace_id",),
    "session_id": ("session_id",),
    "run_id": ("turn_id", "run_id"),
    "call_id": ("call_id",),
    "tool_call_id": ("tool_use_id", "tool_call_id"),
}


def trace_context(input_data: dict[str, Any]) -> dict[str, str]:
    """Build canonical trace context from fields directly present on hook input.

    Always includes ``agent_name`` so agent-sec-cli can attribute the request
    even when no tracing fields are present.
    """
    context: dict[str, str] = {"agent_name": "qwen"}
    for output_key, input_keys in _FIELD_ALIASES.items():
        for input_key in input_keys:
            value = input_data.get(input_key)
            if isinstance(value, str) and value.strip():
                context[output_key] = value.strip()
                break
    return context


def with_trace_context(args: list[str], input_data: dict[str, Any]) -> list[str]:
    """Prepend hidden agent-sec-cli trace-context args to *args*.

    Inserts ``--trace-context <json>`` immediately after the command name
    (``args[0]``) so the flag is parsed as a top-level option.
    """
    context = trace_context(input_data)
    return [
        args[0],
        "--trace-context",
        json.dumps(context, ensure_ascii=False, separators=(",", ":")),
        *args[1:],
    ]
