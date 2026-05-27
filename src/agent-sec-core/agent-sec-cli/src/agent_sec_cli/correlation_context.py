"""Caller-provided tracing context for agent-sec-cli security events."""

import json
import os
import uuid
from collections.abc import Mapping
from contextvars import ContextVar, Token
from dataclasses import dataclass
from typing import Any

MAX_CORRELATION_ID_LENGTH = 256
TRUNCATED_CORRELATION_ID_SUFFIX = "...[truncated]"

_FIELD_ALIASES: dict[str, tuple[str, str]] = {
    "trace_id": ("trace_id", "traceId"),
    "session_id": ("session_id", "sessionId"),
    "run_id": ("run_id", "runId"),
    "call_id": ("call_id", "callId"),
    "tool_call_id": ("tool_call_id", "toolCallId"),
}


def truncate_correlation_id(_field_name: str, value: str) -> str:
    """Return *value* capped to the persisted correlation ID length."""
    if len(value) <= MAX_CORRELATION_ID_LENGTH:
        return value

    prefix_len = MAX_CORRELATION_ID_LENGTH - len(TRUNCATED_CORRELATION_ID_SUFFIX)
    return value[:prefix_len] + TRUNCATED_CORRELATION_ID_SUFFIX


@dataclass(frozen=True)
class TraceContext:
    """Normalized caller-provided tracing fields."""

    trace_id: str | None = None
    session_id: str | None = None
    run_id: str | None = None
    call_id: str | None = None
    tool_call_id: str | None = None


# ---------------------------------------------------------------------------
# Hybrid storage: process-level singleton + request-local ContextVar override.
#
# `_PROCESS_TRACE_CONTEXT` is set in `cli.main()` and read by every thread,
# including ThreadPoolExecutor workers in `prompt_scanner`. A pure ContextVar
# would default to empty in newly-spawned threads and break the invariant
# that all records in one CLI process share the same trace context.
#
# `_trace_context_override` is intentionally unused in the short-lived CLI;
# it is reserved for a future daemon mode where one process handles multiple
# concurrent requests, each needing its own per-request context. Do not
# delete — removing it forces a redesign of every consumer when daemon mode
# lands.
# ---------------------------------------------------------------------------
_PROCESS_TRACE_CONTEXT: TraceContext | None = None


class _UnsetTraceContext:
    """Sentinel distinguishing "no override set" from "override explicitly None".

    A daemon-mode handler may legitimately call ``set_current_trace_context(None)``
    to suppress the process-level fallback for a specific request; using
    ``None`` itself as the ContextVar default would conflate the two states.
    """


_UNSET_TRACE_CONTEXT = _UnsetTraceContext()
_TraceContextOverride = TraceContext | None | _UnsetTraceContext

_trace_context_override: ContextVar[_TraceContextOverride] = ContextVar(
    "trace_context_override",
    default=_UNSET_TRACE_CONTEXT,
)

_PROCESS_INVOCATION_ID: str = ""
_invocation_id_override: ContextVar[str] = ContextVar(
    "invocation_id_override",
    default="",
)


def _clean_string(field_name: str, value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    stripped = value.strip()
    if not stripped:
        return None
    return truncate_correlation_id(field_name, stripped)


def _normalized_fields(payload: Mapping[str, Any]) -> dict[str, str]:
    fields: dict[str, str] = {}
    for field_name, aliases in _FIELD_ALIASES.items():
        snake_key, camel_key = aliases
        value = _clean_string(field_name, payload.get(snake_key))
        if value is None:
            value = _clean_string(field_name, payload.get(camel_key))
        if value is not None:
            fields[field_name] = value
    return fields


def parse_trace_context(value: str | None) -> TraceContext | None:
    """Parse a JSON trace context string into normalized snake_case fields."""
    if value is None or not value.strip():
        return None

    try:
        payload = json.loads(value)
    except json.JSONDecodeError as exc:
        raise ValueError("invalid trace context JSON") from exc

    if not isinstance(payload, dict):
        raise ValueError("trace context must be a JSON object")

    return TraceContext(**_normalized_fields(payload))


def init_process_trace_context(ctx: TraceContext | None) -> None:
    """Set the process-level trace context visible to all threads.

    The CLI calls this once per invocation from ``cli.main()`` via the argv
    bootstrap path, before Typer executes callbacks. For tests that need a
    clean slate between scenarios, call ``clear_process_trace_context()`` first.

    Calling this again intentionally replaces the previous value, but normal
    CLI execution should keep a single process-level initialization point.
    """
    global _PROCESS_TRACE_CONTEXT  # noqa: PLW0603
    _PROCESS_TRACE_CONTEXT = ctx


def clear_process_trace_context() -> None:
    """Clear the process-level trace context."""
    global _PROCESS_TRACE_CONTEXT  # noqa: PLW0603
    _PROCESS_TRACE_CONTEXT = None


def set_current_trace_context(
    ctx: TraceContext | None,
) -> Token[_TraceContextOverride]:
    """Set a request-local trace context override."""
    return _trace_context_override.set(ctx)


def reset_current_trace_context(token: Token[_TraceContextOverride]) -> None:
    """Reset a request-local trace context override."""
    _trace_context_override.reset(token)


def get_current_trace_context() -> TraceContext | None:
    """Return request-local trace context, falling back to process-level context."""
    override = _trace_context_override.get()
    if not isinstance(override, _UnsetTraceContext):
        return override
    return _PROCESS_TRACE_CONTEXT


def init_invocation_context() -> None:
    """Initialize the process-level invocation ID once.

    Caller-supplied values via ``AGENT_SEC_INVOCATION_ID`` are stripped and
    truncated to ``MAX_CORRELATION_ID_LENGTH`` so one malformed env value
    cannot inflate every log record. Empty or whitespace-only values fall
    through to a freshly generated UUID.
    """
    global _PROCESS_INVOCATION_ID  # noqa: PLW0603
    if _PROCESS_INVOCATION_ID:
        return
    env_value = _clean_string(
        "invocation_id", os.environ.get("AGENT_SEC_INVOCATION_ID")
    )
    _PROCESS_INVOCATION_ID = env_value if env_value is not None else str(uuid.uuid4())


def clear_invocation_context_for_tests() -> None:
    """Clear invocation context state for in-process tests."""
    global _PROCESS_INVOCATION_ID  # noqa: PLW0603
    _PROCESS_INVOCATION_ID = ""
    _invocation_id_override.set("")


def set_current_invocation_id(invocation_id: str) -> Token[str]:
    """Set a request-local invocation ID override.

    Input is normalized the same way as the env value: stripped and truncated
    to ``MAX_CORRELATION_ID_LENGTH``. Whitespace-only input clears the override.
    """
    cleaned = _clean_string("invocation_id", invocation_id)
    return _invocation_id_override.set(cleaned or "")


def reset_current_invocation_id(token: Token[str]) -> None:
    """Reset a request-local invocation ID override."""
    _invocation_id_override.reset(token)


def get_invocation_id() -> str:
    """Return request-local invocation ID, falling back to the process value."""
    override = _invocation_id_override.get()
    if override:
        return override
    if not _PROCESS_INVOCATION_ID:
        init_invocation_context()
    return _PROCESS_INVOCATION_ID
