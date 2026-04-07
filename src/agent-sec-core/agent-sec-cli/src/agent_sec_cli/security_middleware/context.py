"""RequestContext — per-invocation context propagated through the call chain."""

from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Optional


def _new_uuid() -> str:
    return str(uuid.uuid4())


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


@dataclass
class RequestContext:
    """Immutable context created at the start of every ``invoke()`` call.

    Attributes:
        action:      The requested action name (e.g. ``"sandbox_prehook"``).
        trace_id:    Correlation ID propagated to all ``SecurityEvent`` records.
                     Auto-generated UUID if not supplied.
        caller:      Identity of the caller (``"sandbox-guard"``, ``"cli"``, …).
        session_id:  Optional session-level correlation ID.
        timestamp:   ISO-8601 timestamp of request creation.  Auto-filled.
    """

    action: str
    trace_id: str = ""
    caller: str = ""
    session_id: Optional[str] = None
    timestamp: str = ""

    def __post_init__(self) -> None:
        if not self.trace_id:
            self.trace_id = _new_uuid()
        if not self.timestamp:
            self.timestamp = _now_iso()
