"""Typed repositories backed by the shared SQLite store."""

import json
import logging
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Sequence

from agent_sec_cli.security_events.models import SecurityEventRecord
from agent_sec_cli.security_events.orm_store import SqliteStore
from agent_sec_cli.security_events.schema import SecurityEvent
from sqlalchemy import Select, delete, func, select, text
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.exc import SQLAlchemyError

logger = logging.getLogger(__name__)

_CORRELATION_CANDIDATE_LIMIT = 1000


@dataclass(frozen=True)
class CorrelationCandidate:
    """Security event row plus the original epoch used for correlation sorting."""

    event: SecurityEvent
    timestamp_epoch: float


class SecurityEventRepository:
    """Repository for security event insert/query/count/prune operations."""

    _COUNT_BY_COLUMNS = {
        "category": SecurityEventRecord.category,
        "event_type": SecurityEventRecord.event_type,
        "trace_id": SecurityEventRecord.trace_id,
    }

    def __init__(self, store: SqliteStore) -> None:
        self._store = store

    def insert(self, event: SecurityEvent) -> bool:
        """Insert an event. Returns False for invalid or skipped writes."""
        try:
            values = self._event_values(event)
        except (ValueError, TypeError) as exc:
            print(
                f"[security_events] invalid event params: {exc}",
                file=sys.stderr,
            )
            return False
        session_factory = self._store.session_factory(raise_on_error=True)
        if session_factory is None:
            return False

        stmt = (
            sqlite_insert(SecurityEventRecord)
            .values(**values)
            .on_conflict_do_nothing(index_elements=[SecurityEventRecord.event_id])
        )
        with session_factory.begin() as session:
            session.execute(stmt)
        return True

    @staticmethod
    def _event_values(event: SecurityEvent) -> dict[str, object]:
        """Build the ORM values dict for INSERT."""
        return {
            "event_id": event.event_id,
            "event_type": event.event_type,
            "category": event.category,
            "result": event.result,
            "timestamp": event.timestamp,
            "timestamp_epoch": datetime.fromisoformat(event.timestamp).timestamp(),
            "trace_id": event.trace_id,
            "pid": event.pid,
            "uid": event.uid,
            "session_id": event.session_id,
            "run_id": event.run_id,
            "call_id": event.call_id,
            "tool_call_id": event.tool_call_id,
            "details": json.dumps(event.details, ensure_ascii=False),
        }

    def query(
        self,
        event_type: str | None = None,
        category: str | None = None,
        trace_id: str | None = None,
        since: str | None = None,
        until: str | None = None,
        limit: int = 1000,
        offset: int = 0,
    ) -> list[SecurityEvent]:
        """Query security events with optional filters."""
        conditions = self._build_filters(
            event_type=event_type,
            category=category,
            trace_id=trace_id,
            since=since,
            until=until,
        )
        stmt = (
            select(SecurityEventRecord)
            .where(*conditions)
            .order_by(SecurityEventRecord.timestamp_epoch.desc())
            .limit(limit)
            .offset(offset)
        )

        session_factory = self._store.session_factory()
        if session_factory is None:
            return []

        try:
            with session_factory() as session:
                records = list(session.scalars(stmt).all())
        except SQLAlchemyError:
            self._store.dispose()
            return []

        events: list[SecurityEvent] = []
        for record in records:
            event = self._record_to_event(record)
            if event is not None:
                events.append(event)
        return events

    def query_correlation_candidates(
        self,
        *,
        session_id: str,
        categories: Sequence[str],
        run_id: str | None = None,
        tool_call_id: str | None = None,
        tool_call_ids: Sequence[str] | None = None,
        since_epoch: float | None = None,
        until_epoch: float | None = None,
    ) -> list[CorrelationCandidate]:
        """Query up to 1000 read-only candidates for observability correlation."""
        if not categories:
            return []

        conditions: list[Any] = [
            SecurityEventRecord.session_id == session_id,
            SecurityEventRecord.category.in_(tuple(categories)),
        ]
        if run_id is not None:
            conditions.append(SecurityEventRecord.run_id == run_id)
        if tool_call_ids is not None:
            normalized_tool_call_ids = tuple(value for value in tool_call_ids if value)
            if not normalized_tool_call_ids:
                return []
            conditions.append(
                SecurityEventRecord.tool_call_id.in_(normalized_tool_call_ids)
            )
        elif tool_call_id is not None:
            conditions.append(SecurityEventRecord.tool_call_id == tool_call_id)
        if since_epoch is not None:
            conditions.append(SecurityEventRecord.timestamp_epoch >= since_epoch)
        if until_epoch is not None:
            conditions.append(SecurityEventRecord.timestamp_epoch <= until_epoch)

        stmt = (
            select(SecurityEventRecord)
            .where(*conditions)
            .order_by(
                SecurityEventRecord.timestamp_epoch.asc(),
                SecurityEventRecord.event_id.asc(),
            )
            .limit(_CORRELATION_CANDIDATE_LIMIT)
        )

        session_factory = self._store.session_factory()
        if session_factory is None:
            return []

        try:
            with session_factory() as session:
                records = list(session.scalars(stmt).all())
        except SQLAlchemyError as exc:
            logger.warning(
                "correlation candidate query failed",
                extra={
                    "session_id": session_id,
                    "run_id": run_id,
                    "data": {"error_type": type(exc).__name__},
                },
            )
            self._store.dispose()
            return []

        candidates: list[CorrelationCandidate] = []
        for record in records:
            event = self._record_to_event(record)
            if event is not None:
                candidates.append(
                    CorrelationCandidate(
                        event=event,
                        timestamp_epoch=record.timestamp_epoch,
                    )
                )
        return candidates

    def count(
        self,
        event_type: str | None = None,
        category: str | None = None,
        trace_id: str | None = None,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> int:
        """Count events matching the given filters."""
        conditions = self._build_filters(
            event_type=event_type,
            category=category,
            trace_id=trace_id,
            since=since,
            until=until,
        )
        if offset == 0:
            stmt: Select[tuple[int]] = (
                select(func.count()).select_from(SecurityEventRecord).where(*conditions)
            )
        else:
            subquery = (
                select(SecurityEventRecord.event_id)
                .where(*conditions)
                .limit(-1)
                .offset(offset)
                .subquery()
            )
            stmt = select(func.count()).select_from(subquery)

        session_factory = self._store.session_factory()
        if session_factory is None:
            return 0

        try:
            with session_factory() as session:
                return int(session.execute(stmt).scalar_one())
        except SQLAlchemyError:
            self._store.dispose()
            return 0

    def count_by(
        self,
        group_field: str,
        event_type: str | None = None,
        category: str | None = None,
        trace_id: str | None = None,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> dict[str, int]:
        """Count events grouped by a specific allowlisted field."""
        column = self._COUNT_BY_COLUMNS.get(group_field)
        if column is None:
            raise ValueError(
                f"Invalid group_field: {group_field!r}. "
                "Must be one of: category, event_type, trace_id"
            )

        conditions = self._build_filters(
            event_type=event_type,
            category=category,
            trace_id=trace_id,
            since=since,
            until=until,
        )
        if offset == 0:
            stmt = select(column, func.count()).where(*conditions).group_by(column)
        else:
            subquery = (
                select(column.label(group_field))
                .where(*conditions)
                .limit(-1)
                .offset(offset)
                .subquery()
            )
            subquery_column = getattr(subquery.c, group_field)
            stmt = select(subquery_column, func.count()).group_by(subquery_column)

        session_factory = self._store.session_factory()
        if session_factory is None:
            return {}

        try:
            with session_factory() as session:
                rows = session.execute(stmt).all()
                return {row[0]: int(row[1]) for row in rows}
        except SQLAlchemyError:
            self._store.dispose()
            return {}

    def prune(self, max_age_days: int) -> None:
        """Delete rows older than max_age_days."""
        session_factory = self._store.session_factory()
        if session_factory is None:
            return

        cutoff = time.time() - (max_age_days * 86400)

        try:
            with session_factory.begin() as session:
                session.execute(
                    delete(SecurityEventRecord).where(
                        SecurityEventRecord.timestamp_epoch < cutoff
                    )
                )
        except SQLAlchemyError:
            self._store.dispose()

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
    def _timestamp_epoch(value: str) -> float:
        """Parse an ISO timestamp as UTC when timezone information is absent."""
        dt = datetime.fromisoformat(value)
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt.timestamp()

    def _build_filters(
        self,
        *,
        event_type: str | None = None,
        category: str | None = None,
        trace_id: str | None = None,
        since: str | None = None,
        until: str | None = None,
    ) -> list[Any]:
        """Build SQLAlchemy filter expressions from non-None filters."""
        conditions: list[Any] = []
        if event_type is not None:
            conditions.append(SecurityEventRecord.event_type == event_type)
        if category is not None:
            conditions.append(SecurityEventRecord.category == category)
        if trace_id is not None:
            conditions.append(SecurityEventRecord.trace_id == trace_id)
        if since is not None:
            conditions.append(
                SecurityEventRecord.timestamp_epoch >= self._timestamp_epoch(since)
            )
        if until is not None:
            conditions.append(
                SecurityEventRecord.timestamp_epoch < self._timestamp_epoch(until)
            )
        return conditions

    @staticmethod
    def _record_to_event(record: SecurityEventRecord) -> SecurityEvent | None:
        """Convert an ORM record to SecurityEvent. Returns None on parse error."""
        try:
            return SecurityEvent(
                event_id=record.event_id,
                event_type=record.event_type,
                category=record.category,
                result=record.result,
                timestamp=record.timestamp,
                trace_id=record.trace_id,
                pid=record.pid,
                uid=record.uid,
                session_id=record.session_id,
                run_id=record.run_id,
                call_id=record.call_id,
                tool_call_id=record.tool_call_id,
                details=json.loads(record.details),
            )
        except (json.JSONDecodeError, TypeError, ValueError) as exc:
            print(f"[security_events] malformed row skipped: {exc}", file=sys.stderr)
            return None
