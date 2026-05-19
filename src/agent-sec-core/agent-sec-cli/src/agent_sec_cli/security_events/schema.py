"""SecurityEvent pydantic model — the canonical event envelope."""

import os
import uuid
from datetime import datetime, timezone
from typing import Any, Literal

from pydantic import BaseModel, Field


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _new_uuid() -> str:
    return str(uuid.uuid4())


class SecurityEvent(BaseModel):
    """Single security event to be persisted as a JSONL record.

    Required fields (caller must supply):
        event_type  — e.g. sandbox_prehook, hardening_scan, hardening_fix, …
        category    — sandbox | hardening | asset_verify | intent_security
        details     — backend-specific structured data

    Auto-filled fields:
        result      — succeeded (default) | failed
        trace_id    — injected by middleware (empty string until then)
        timestamp   — ISO-8601
        event_id    — UUID
        pid / uid   — current process identity
        session_id  — optional session correlation
        run_id      — optional agent run/turn correlation
        call_id     — optional LLM call correlation
        tool_call_id — optional tool call correlation
    """

    event_type: str
    category: str
    details: dict[str, Any]
    result: Literal["succeeded", "failed"] = "succeeded"
    trace_id: str = ""
    timestamp: str = Field(default_factory=_now_iso)
    event_id: str = Field(default_factory=_new_uuid)
    pid: int = Field(default_factory=os.getpid)
    uid: int = Field(default_factory=os.getuid)
    session_id: str | None = None
    run_id: str | None = None
    call_id: str | None = None
    tool_call_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        """Return a plain ``dict`` suitable for ``json.dumps``."""
        d = self.model_dump()
        # Return keys in the canonical order expected by callers.
        return {
            "event_id": d["event_id"],
            "event_type": d["event_type"],
            "category": d["category"],
            "result": d["result"],
            "timestamp": d["timestamp"],
            "trace_id": d["trace_id"],
            "pid": d["pid"],
            "uid": d["uid"],
            "session_id": d["session_id"],
            "run_id": d["run_id"],
            "call_id": d["call_id"],
            "tool_call_id": d["tool_call_id"],
            "details": d["details"],
        }
