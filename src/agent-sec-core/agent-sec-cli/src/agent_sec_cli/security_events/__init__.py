"""security_events — fire-and-forget JSONL security event logging.

Public API
----------
- ``log_event(event)``  — append a ``SecurityEvent`` to the JSONL log
- ``get_writer()``      — obtain the singleton ``SecurityEventWriter``
- ``SecurityEvent``     — the canonical event dataclass
"""

from __future__ import annotations

from typing import Optional

from agent_sec_cli.security_events.schema import SecurityEvent
from agent_sec_cli.security_events.writer import SecurityEventWriter

_writer: Optional[SecurityEventWriter] = None


def get_writer() -> SecurityEventWriter:
    """Return the module-level singleton writer (created lazily)."""
    global _writer  # noqa: PLW0603
    if _writer is None:
        _writer = SecurityEventWriter()
    return _writer


def log_event(event: SecurityEvent) -> None:
    """Persist *event* to the JSONL log.

    This is deliberately **fire-and-forget**: any failure is silently
    swallowed so that callers are never disrupted by logging issues.
    """
    try:
        get_writer().write(event)
    except Exception:  # noqa: BLE001
        pass


__all__ = ["log_event", "get_writer", "SecurityEvent"]
