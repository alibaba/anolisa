"""Typed repository for observability SQLite indexing."""

import json
import sys
from datetime import datetime, timezone
from typing import Any

from agent_sec_cli.observability.config import OBSERVABILITY_LOG_PREFIX
from agent_sec_cli.observability.models import ObservabilityEventRecord
from agent_sec_cli.observability.schema import ObservabilityRecord
from agent_sec_cli.security_events.orm_store import SqliteStore
from sqlalchemy import delete, func, select, text
from sqlalchemy.exc import SQLAlchemyError


class ObservabilityEventRepository:
    """Repository for observability insert/count/prune operations."""

    def __init__(self, store: SqliteStore) -> None:
        self._store = store

    def insert(self, record: ObservabilityRecord) -> bool:
        """Insert an observability record. Returns False for skipped writes."""
        try:
            values = self._record_values(record)
        except (ValueError, TypeError) as exc:
            print(
                f"{OBSERVABILITY_LOG_PREFIX} invalid event params: {exc}",
                file=sys.stderr,
            )
            return False

        session_factory = self._store.session_factory()
        if session_factory is None:
            return False

        with session_factory.begin() as session:
            session.add(ObservabilityEventRecord(**values))
        return True

    def count(self) -> int:
        """Return the number of indexed observability records."""
        session_factory = self._store.session_factory()
        if session_factory is None:
            return 0

        try:
            with session_factory() as session:
                return int(
                    session.execute(
                        select(func.count()).select_from(ObservabilityEventRecord)
                    ).scalar_one()
                )
        except SQLAlchemyError:
            self._store.dispose()
            return 0

    def prune(
        self,
        max_age_days: int,
        *,
        now: datetime | None = None,
    ) -> None:
        """Delete rows older than max_age_days by observed_at_epoch."""
        session_factory = self._store.session_factory()
        if session_factory is None:
            return

        cutoff = _epoch(now or datetime.now(timezone.utc)) - (max_age_days * 86400)
        try:
            with session_factory.begin() as session:
                session.execute(
                    delete(ObservabilityEventRecord).where(
                        ObservabilityEventRecord.observed_at_epoch < cutoff
                    )
                )
        except SQLAlchemyError:
            pass

    def checkpoint(self) -> None:
        """Run a best-effort WAL checkpoint on the current engine."""
        engine = self._store.engine
        if engine is None:
            return
        try:
            with engine.connect() as conn:
                conn.execute(text("PRAGMA wal_checkpoint(TRUNCATE)"))
        except Exception:  # noqa: BLE001
            pass

    @staticmethod
    def _record_values(record: ObservabilityRecord) -> dict[str, object]:
        """Build the ORM values dict for INSERT."""
        wire_record = record.to_record()
        metrics = _ensure_mapping(wire_record["metrics"], "metrics")
        metadata = _ensure_mapping(wire_record["metadata"], "metadata")

        return {
            "hook": record.hook,
            "observed_at": str(wire_record["observedAt"]),
            "observed_at_epoch": record.observed_at.timestamp(),
            "session_id": str(metadata["sessionId"]),
            "run_id": str(metadata["runId"]),
            "metrics_json": json.dumps(metrics, ensure_ascii=False),
            "metadata_json": json.dumps(metadata, ensure_ascii=False),
            "call_id": _optional_str(metadata.get("callId")),
            "tool_call_id": _optional_str(metadata.get("toolCallId")),
        }


def _ensure_mapping(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TypeError(f"{name} must be an object")
    return value


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    return str(value)


def _epoch(value: datetime) -> float:
    if value.tzinfo is None or value.tzinfo.utcoffset(value) is None:
        value = value.replace(tzinfo=timezone.utc)
    return value.timestamp()


__all__ = ["ObservabilityEventRepository"]
