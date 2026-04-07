"""security_middleware — single entry point for all security capabilities.

Public API
----------
- ``invoke(action, **kwargs)``  — the sole entry point
- ``ActionResult``              — structured return type
- ``RequestContext``             — per-call context (usually internal)
"""

from __future__ import annotations

import inspect
import os
from typing import List

from . import lifecycle, router
from .context import RequestContext
from .result import ActionResult

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

    Returns a human-friendly string such as ``"sandbox-guard"`` or ``"cli"``.
    Falls back to ``"unknown"`` when no known entry point is found.
    """
    for frame_info in inspect.stack():
        basename = os.path.basename(frame_info.filename)
        if basename in _CALLER_MAP:
            return _CALLER_MAP[basename]
    return "unknown"


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def invoke(action: str, **kwargs) -> ActionResult:
    """Sole public entry point for all security capabilities.

    1. Builds a :class:`RequestContext` (auto ``trace_id``, ``timestamp``).
    2. Calls ``pre_action`` (no-op under the single-event model).
    3. Routes to the appropriate backend and calls ``execute(ctx, **kwargs)``.
    4. Logs a single ``<action>`` completion event (post-hook) — or a single
       ``<action>_error`` event on failure, each containing both the request
       kwargs and the result/error details.
    5. Returns the :class:`ActionResult` produced by the backend.

    Raises whatever exception the backend raises (after logging it).
    """
    # TODO: inherit trace_id and session_id from parent context, if any
    ctx = RequestContext(action=action, caller=_detect_caller())

    lifecycle.pre_action(ctx, kwargs)

    try:
        backend = router.get_backend(action)
        result = backend.execute(ctx, **kwargs)
    except Exception as exc:
        lifecycle.on_error(ctx, exc, kwargs)
        raise

    lifecycle.post_action(ctx, result, kwargs)
    return result


__all__: List[str] = ["invoke", "ActionResult", "RequestContext"]
