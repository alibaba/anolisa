"""Lifecycle hooks — transparent pre/post/error logging via security_events."""

from __future__ import annotations

import copy
from typing import Any, Dict

from agent_sec_cli.security_events import SecurityEvent, log_event

from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult

# ---------------------------------------------------------------------------
# Action → SecurityEvent category mapping
# ---------------------------------------------------------------------------

_ACTION_CATEGORY: Dict[str, str] = {
    "sandbox_prehook": "sandbox",
    "harden":          "hardening",
    "verify":          "asset_verify",
    "summary":         "summary",
}


def _category_for(action: str) -> str:
    """Return the event category for *action*, defaulting to the action name."""
    return _ACTION_CATEGORY.get(action, action)


# ---------------------------------------------------------------------------
# Hooks
# ---------------------------------------------------------------------------


def pre_action(ctx: RequestContext, kwargs: Dict[str, Any]) -> None:
    """No-op — kept for future extensibility.

    Single-event model: we only emit one event per invocation, either on
    successful completion (post_action) or on failure (on_error).  Logging a
    separate ``<action>_request`` event here would produce two events per call,
    which conflicts with that policy.
    """
    # Intentionally empty — do not add log_event() here.


def post_action(ctx: RequestContext, result: ActionResult, kwargs: Dict[str, Any]) -> None:
    """Log the single completion event after the backend completes.

    Merges *kwargs* (request inputs) and *result.data* (backend outputs) into a
    single event so the full request/response context is captured in one record.
    """
    try:
        details: Dict[str, Any] = {
            "request": copy.deepcopy(kwargs),
            "result": copy.deepcopy(result.data),
        }
        event = SecurityEvent(
            event_type=ctx.action,
            category=_category_for(ctx.action),
            details=details,
            trace_id=ctx.trace_id,
            session_id=ctx.session_id,
        )
        log_event(event)
    except Exception:  # noqa: BLE001
        pass


def on_error(ctx: RequestContext, exception: Exception, kwargs: Dict[str, Any]) -> None:
    """Log the single error event when the backend raises.

    Merges *kwargs* (request inputs) and error details into a single event so
    the full request context is captured alongside the failure.
    """
    try:
        details: Dict[str, Any] = {
            "request": copy.deepcopy(kwargs),
            "error": str(exception),
            "error_type": type(exception).__name__,
        }
        event = SecurityEvent(
            event_type=f"{ctx.action}_error",
            category=_category_for(ctx.action),
            details=details,
            trace_id=ctx.trace_id,
            session_id=ctx.session_id,
        )
        log_event(event)
    except Exception:  # noqa: BLE001
        pass
