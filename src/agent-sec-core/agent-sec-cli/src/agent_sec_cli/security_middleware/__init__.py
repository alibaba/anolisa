"""security_middleware — single entry point for all security capabilities.

Public API
----------
- ``invoke(action, **kwargs)``  — the sole entry point
- ``ActionResult``              — structured return type
- ``RequestContext``             — per-call context (usually internal)
"""

import logging
import sys
import time
from pathlib import PurePath
from typing import Any

from agent_sec_cli.security_middleware import lifecycle, router
from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult

logger = logging.getLogger(__name__)

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
    started_at = time.perf_counter()

    backend = router.get_backend(action)

    logger.debug(
        "action started",
        extra={
            "trace_id": ctx.trace_id,
            "data": {"action": action, "caller": ctx.caller},
        },
    )
    lifecycle.pre_action(ctx, kwargs)

    try:
        result = backend.execute(ctx, **kwargs)
    except Exception as exc:
        duration_ms = (time.perf_counter() - started_at) * 1000
        logger.error(
            "backend raised an exception",
            exc_info=True,
            extra={
                "trace_id": ctx.trace_id,
                "data": {
                    "action": action,
                    "caller": ctx.caller,
                    "duration_ms": duration_ms,
                },
            },
        )
        lifecycle.on_error(ctx, exc, kwargs, backend)
        raise

    lifecycle.post_action(ctx, result, kwargs, backend)
    duration_ms = (time.perf_counter() - started_at) * 1000
    log_level = logging.INFO if result.exit_code == 0 else logging.WARNING
    logger.log(
        log_level,
        "action completed with exit code %d",
        result.exit_code,
        extra={
            "trace_id": ctx.trace_id,
            "data": {
                "action": action,
                "caller": ctx.caller,
                "duration_ms": duration_ms,
                "exit_code": result.exit_code,
            },
        },
    )
    return result


__all__: list[str] = ["invoke", "ActionResult", "RequestContext"]
