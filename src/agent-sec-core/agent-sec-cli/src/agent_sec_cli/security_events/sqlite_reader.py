"""SQLAlchemy-backed reader for querying security events."""

from pathlib import Path

from agent_sec_cli.security_events.config import get_db_path
from agent_sec_cli.security_events.orm_store import SqliteStore
from agent_sec_cli.security_events.repositories import SecurityEventRepository
from agent_sec_cli.security_events.schema import SecurityEvent
from sqlalchemy.engine import Engine
from sqlalchemy.orm import Session, sessionmaker


class SqliteEventReader:
    """Read-only SQLAlchemy reader for security events."""

    def __init__(self, path: str | Path | None = None) -> None:
        self._store = SqliteStore(path or get_db_path(), read_only=True)
        self._repository = SecurityEventRepository(self._store)

    @property
    def _engine(self) -> Engine | None:
        return self._store.engine

    @property
    def _session_factory(self) -> sessionmaker[Session] | None:
        return self._store.cached_session_factory

    def _ensure_session_factory(self) -> sessionmaker[Session] | None:
        """Return a lazily initialized read-only session factory."""
        return self._store.session_factory()

    def _dispose_engine(self) -> None:
        """Dispose SQLAlchemy engine state and clear session factory."""
        self._store.dispose()

    def close(self) -> None:
        """Dispose cached read-only connections."""
        self._store.close()

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
        return self._repository.query(
            event_type=event_type,
            category=category,
            trace_id=trace_id,
            since=since,
            until=until,
            limit=limit,
            offset=offset,
        )

    def count(
        self,
        event_type: str | None = None,
        category: str | None = None,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> int:
        """Count events matching the given filters."""
        return self._repository.count(
            event_type=event_type,
            category=category,
            since=since,
            until=until,
            offset=offset,
        )

    def count_by(
        self,
        group_field: str,
        since: str | None = None,
        until: str | None = None,
        offset: int = 0,
    ) -> dict[str, int]:
        """Count events grouped by a specific field."""
        return self._repository.count_by(
            group_field,
            since=since,
            until=until,
            offset=offset,
        )
