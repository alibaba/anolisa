"""SQLAlchemy-backed writer for security events.

Runs alongside the existing JSONL writer (dual-write pattern).
All exceptions are swallowed — never raises to callers.
"""

from pathlib import Path

from agent_sec_cli.security_events.config import get_db_path
from agent_sec_cli.security_events.orm_store import (
    SqliteStore,
    is_sqlite_corruption_error,
    is_sqlite_schema_error,
)
from agent_sec_cli.security_events.repositories import SecurityEventRepository
from agent_sec_cli.security_events.schema import SecurityEvent
from sqlalchemy.engine import Engine
from sqlalchemy.exc import DatabaseError, SQLAlchemyError
from sqlalchemy.orm import Session, sessionmaker


class SqliteEventWriter:
    """Fire-and-forget SQLAlchemy writer for security events."""

    def __init__(
        self,
        path: str | Path | None = None,
        max_age_days: int = 30,
    ) -> None:
        self._store = SqliteStore(path or get_db_path())
        self._repository = SecurityEventRepository(self._store)
        self._max_age_days = max_age_days

    @property
    def _engine(self) -> Engine | None:
        return self._store.engine

    @property
    def _session_factory(self) -> sessionmaker[Session] | None:
        return self._store.cached_session_factory

    @property
    def _disabled(self) -> bool:
        return self._store.disabled

    def write(self, event: SecurityEvent) -> None:
        """Insert *event* into SQLite. Fire-and-forget — never raises."""
        if self._store.disabled:
            return

        try:
            self._repository.insert(event)
        except DatabaseError as exc:
            if not is_sqlite_corruption_error(exc):
                if is_sqlite_schema_error(exc):
                    self._store.request_schema_repair()
                return
            self._store.handle_corruption(exc)
            if self._store.disabled:
                return
            try:
                self._repository.insert(event)
            except Exception:  # noqa: BLE001
                pass
        except (SQLAlchemyError, OSError):
            self._store.dispose()

    def close(self) -> None:
        """Best-effort prune, WAL checkpoint, and dispose pooled connections."""
        if self._store.engine is None:
            return
        self._repository.prune(self._max_age_days)
        self._repository.checkpoint()
        self._store.close()

    def _ensure_session_factory(self) -> sessionmaker[Session] | None:
        """Return the lazily initialized session factory."""
        return self._store.session_factory()

    def _dispose_engine(self) -> None:
        """Dispose SQLAlchemy engine state and clear session factory."""
        self._store.dispose()

    def _handle_corruption(self, exc: Exception) -> None:
        """Delete a corrupt database and prepare for a fresh start."""
        self._store.handle_corruption(exc)
