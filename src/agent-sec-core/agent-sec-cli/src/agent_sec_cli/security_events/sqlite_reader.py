"""SQLAlchemy-backed reader for querying security events."""

import json
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path

from agent_sec_cli.security_events.config import get_db_path
from agent_sec_cli.security_events.orm_store import (
    SecurityEventRecord,
    create_sqlite_engine,
    normalize_sqlite_path,
)
from agent_sec_cli.security_events.schema import SecurityEvent
from sqlalchemy import Select, func, select
from sqlalchemy.engine import Engine
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.orm import Session, sessionmaker


class SqliteEventReader:
    """Read-only SQLAlchemy reader for security events.

    Uses pooled SQLAlchemy connections and per-call sessions. The engine applies
    SQLite query-only mode for reader connections.
    """

    _COUNT_BY_COLUMNS = {
        "category": SecurityEventRecord.category,
        "event_type": SecurityEventRecord.event_type,
        "trace_id": SecurityEventRecord.trace_id,
    }

    def __init__(self, path: str | Path | None = None) -> None:
        self._path = normalize_sqlite_path(path or get_db_path())
        self._engine_lock = threading.Lock()
        self._engine: Engine | None = None
        self._session_factory: sessionmaker[Session] | None = None
        self._db_identity: tuple[int, int] | None = None

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _ensure_session_factory(self) -> sessionmaker[Session] | None:
        """Return a lazily initialized read-only session factory."""
        db_identity = self._current_db_identity()
        if db_identity is None:
            with self._engine_lock:
                self._dispose_engine()
            return None

        if self._has_current_session_factory(db_identity):
            return self._session_factory

        with self._engine_lock:
            db_identity = self._current_db_identity()
            if db_identity is None:
                self._dispose_engine()
                return None

            if self._has_current_session_factory(db_identity):
                return self._session_factory

            self._dispose_engine()

            try:
                engine = create_sqlite_engine(self._path, read_only=True)
                self._engine = engine
                self._db_identity = db_identity
                self._session_factory = sessionmaker(
                    bind=engine,
                    expire_on_commit=False,
                    future=True,
                )
            except SQLAlchemyError:
                self._dispose_engine()
                return None

        return self._session_factory

    def _has_current_session_factory(self, db_identity: tuple[int, int]) -> bool:
        """Return True when cached reader state matches the current DB file."""
        return self._session_factory is not None and self._db_identity == db_identity

    def _current_db_identity(self) -> tuple[int, int] | None:
        """Return the current DB file identity, or None if it is unavailable."""
        try:
            stat_result = self._path.stat()
        except OSError:
            return None
        return (stat_result.st_dev, stat_result.st_ino)

    def _dispose_engine(self) -> None:
        """Dispose SQLAlchemy engine state and clear session factory."""
        if self._engine is not None:
            try:
                self._engine.dispose()
            except Exception:  # noqa: BLE001
                pass
        self._engine = None
        self._session_factory = None
        self._db_identity = None

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
    ) -> list[object]:
        """Build SQLAlchemy filter expressions from non-None filters."""
        conditions: list[object] = []
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

    def _record_to_event(self, record: SecurityEventRecord) -> SecurityEvent | None:
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
                details=json.loads(record.details),
            )
        except (json.JSONDecodeError, TypeError, ValueError) as exc:
            print(f"[security_events] malformed row skipped: {exc}", file=sys.stderr)
            return None

    # ------------------------------------------------------------------
    # Public query API
    # ------------------------------------------------------------------

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
        """Query security events with optional filters.

        Parameters
        ----------
        event_type : str, optional
            Filter by event type.
        category : str, optional
            Filter by category.
        trace_id : str, optional
            Filter by trace ID.
        since : str, optional
            Inclusive lower bound (ISO-8601 timestamp).
        until : str, optional
            Exclusive upper bound (ISO-8601 timestamp).
        limit : int
            Maximum number of results (default 1000).
        offset : int
            Number of results to skip (default 0).

        Returns
        -------
        list[SecurityEvent]
            Matching events ordered by timestamp descending.
        """
        conditions = self._build_filters(
            event_type=event_type,
            category=category,
            trace_id=trace_id,
            since=since,
            until=until,
        )
        session_factory = self._ensure_session_factory()
        if session_factory is None:
            return []

        try:
            stmt = (
                select(SecurityEventRecord)
                .where(*conditions)
                .order_by(SecurityEventRecord.timestamp_epoch.desc())
                .limit(limit)
                .offset(offset)
            )
            with session_factory() as session:
                records = session.scalars(stmt).all()
        except SQLAlchemyError:
            return []

        events: list[SecurityEvent] = []
        for record in records:
            event = self._record_to_event(record)
            if event is not None:
                events.append(event)
        return events

    def count(
        self,
        event_type: str | None = None,
        category: str | None = None,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> int:
        """Count events matching the given filters.

        Parameters
        ----------
        event_type : str, optional
            Filter by event type.
        category : str, optional
            Filter by category.
        since : str, optional
            Inclusive lower bound (ISO-8601 timestamp).
        until : str, optional
            Exclusive upper bound (ISO-8601 timestamp).
        offset : int
            Number of results to skip (default 0).

        Returns
        -------
        int
            Number of matching events after applying offset.
        """
        conditions = self._build_filters(
            event_type=event_type,
            category=category,
            since=since,
            until=until,
        )
        session_factory = self._ensure_session_factory()
        if session_factory is None:
            return 0

        try:
            if offset == 0:
                stmt: Select[tuple[int]] = (
                    select(func.count())
                    .select_from(SecurityEventRecord)
                    .where(*conditions)
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

            with session_factory() as session:
                result = session.execute(stmt).scalar_one()
                return int(result)
        except SQLAlchemyError:
            return 0

    def count_by(
        self,
        group_field: str,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> dict[str, int]:
        """Count events grouped by a specific field.

        Parameters
        ----------
        group_field : str
            Field to group by. Must be one of: category, event_type, trace_id.
        since : str, optional
            Inclusive lower bound (ISO-8601 timestamp).
        until : str, optional
            Exclusive upper bound (ISO-8601 timestamp).
        offset : int
            Number of results to skip (default 0).

        Returns
        -------
        dict[str, int]
            Mapping of field value to event count after applying offset.

        Raises
        ------
        ValueError
            If group_field is not in the allowlist.
        """
        column = self._COUNT_BY_COLUMNS.get(group_field)
        if column is None:
            raise ValueError(
                f"Invalid group_field: {group_field!r}. "
                "Must be one of: category, event_type, trace_id"
            )

        conditions = self._build_filters(since=since, until=until)
        session_factory = self._ensure_session_factory()
        if session_factory is None:
            return {}

        try:
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

            with session_factory() as session:
                rows = session.execute(stmt).all()
                return {row[0]: int(row[1]) for row in rows}
        except SQLAlchemyError:
            return {}
