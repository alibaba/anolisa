"""security_middleware — single entry point for all security capabilities.

Public API
----------
- ``invoke(action, **kwargs)`` — CLI/local entry point with caller auto-detection
- ``invoke_with_context(...)`` — explicit context entry point for daemon/plugin paths
- ``ActionResult``             — structured return type
- ``RequestContext``           — per-call context (usually internal)
"""

import sys
from collections.abc import Mapping
from pathlib import PurePath
from typing import Any

from agent_sec_cli.correlation_context import (
    TraceContext,
    parse_trace_context_payload,
    reset_current_trace_context,
    set_current_trace_context,
)
from agent_sec_cli.security_middleware import lifecycle, router
from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult

# ---------------------------------------------------------------------------
# Caller auto-detection
# ---------------------------------------------------------------------------

# Basenames of known entry-point scripts → friendly caller names.
_CALLER_MAP = {
    "sandbox-guard.py": "sandbox-guard",
    "cli.py": "cli",
}


def _detect_caller() -> str:
    """Walk the call stack to identify the outermost known caller.

    Uses :func:`sys._getframe` instead of :func:`inspect.stack` to avoid
    the overhead of capturing locals and source context for every frame
    — important because this runs on every :func:`invoke` call.

    Returns a human-friendly string such as ``"sandbox-guard"`` or ``"cli"``.
    Falls back to ``"unknown"`` when no known entry point is found.
    """
    frame = sys._getframe()
    while frame is not None:
        basename = PurePath(frame.f_code.co_filename).name
        if basename in _CALLER_MAP:
            return _CALLER_MAP[basename]
        frame = frame.f_back
    return "unknown"


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def invoke(action: str, **kwargs: Any) -> ActionResult:
    """Sole public entry point for all security capabilities.

    1. Builds a :class:`RequestContext` (auto ``trace_id``, ``timestamp``).
    2. Routes to the appropriate backend.
    3. Calls ``pre_action`` (no-op under the single-event model), then
       ``execute(ctx, **kwargs)``.
    4. Logs a single ``<action>`` completion event (post-hook) with
       ``result="succeeded"``, or logs the same event type with
       ``result="failed"`` on failure (on_error). Each event contains both
       the request kwargs and the result/error details.
    5. Returns the :class:`ActionResult` produced by the backend.

    Raises whatever exception the backend raises (after logging it).
    """
    ctx = RequestContext(action=action, caller=_detect_caller())
    return _execute_action(ctx, kwargs)


def invoke_with_context(
    action: str,
    *,
    caller: str,
    trace_context: Mapping[str, Any] | TraceContext | None = None,
    **kwargs: Any,
) -> ActionResult:
    """Invoke a security action with explicit daemon/plugin request context.

    This entry point is intended for long-running processes, such as the daemon,
    where caller identity and trace metadata must come from the request instead
    of being inferred from the Python call stack or process-level CLI state.
    """
    normalized_caller = caller.strip() if caller.strip() else "unknown"
    token = set_current_trace_context(_coerce_trace_context(trace_context))
    try:
        ctx = RequestContext(action=action, caller=normalized_caller)
        return _execute_action(ctx, kwargs)
    finally:
        reset_current_trace_context(token)


def _coerce_trace_context(
    trace_context: Mapping[str, Any] | TraceContext | None,
) -> TraceContext | None:
    if isinstance(trace_context, TraceContext) or trace_context is None:
        return trace_context
    return parse_trace_context_payload(trace_context)


def _execute_action(ctx: RequestContext, kwargs: dict[str, Any]) -> ActionResult:
    backend = router.get_backend(ctx.action)

    lifecycle.pre_action(ctx, kwargs)

    try:
        result = backend.execute(ctx, **kwargs)
    except Exception as exc:
        lifecycle.on_error(ctx, exc, kwargs, backend)
        raise

    lifecycle.post_action(ctx, result, kwargs, backend)
    return result


__all__: list[str] = [
    "invoke",
    "invoke_with_context",
    "ActionResult",
    "RequestContext",
]
