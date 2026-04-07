"""SecurityEvent dataclass — the canonical event envelope."""

from __future__ import annotations

import os
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, Optional


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _new_uuid() -> str:
    return str(uuid.uuid4())


@dataclass
class SecurityEvent:
    """Single security event to be persisted as a JSONL record.

    Required fields (caller must supply):
        event_type  — e.g. sandbox_prehook, hardening_scan, hardening_fix, …
        category    — sandbox | hardening | asset_verify | intent_security
        details     — backend-specific structured data

    Auto-filled fields:
        trace_id    — injected by middleware (empty string until then)
        timestamp   — ISO-8601
        event_id    — UUID
        pid / uid   — current process identity
        session_id  — optional session correlation
    """

    event_type: str
    category: str
    details: Dict[str, Any]
    trace_id: str = ""
    timestamp: str = field(default_factory=_now_iso)
    event_id: str = field(default_factory=_new_uuid)
    pid: int = field(default_factory=os.getpid)
    uid: int = field(default_factory=os.getuid)
    session_id: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        """Return a plain ``dict`` suitable for ``json.dumps``."""
        return {
            "event_id": self.event_id,
            "event_type": self.event_type,
            "category": self.category,
            "timestamp": self.timestamp,
            "trace_id": self.trace_id,
            "pid": self.pid,
            "uid": self.uid,
            "session_id": self.session_id,
            "details": self.details,
        }
