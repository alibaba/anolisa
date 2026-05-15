"""SQLAlchemy-backed writer for observability records."""

from pathlib import Path

from agent_sec_cli.observability.config import (
    DEFAULT_OBSERVABILITY_RETENTION_DAYS,
    OBSERVABILITY_LOG_PREFIX,
    get_observability_db_path,
)
from agent_sec_cli.observability.models import (
    OBSERVABILITY_SQLITE_SCHEMA_VERSION,
    ORM_MODELS,
)
from agent_sec_cli.observability.repositories import (
    ObservabilityEventRepository,
)
from agent_sec_cli.observability.schema import ObservabilityRecord
from agent_sec_cli.security_events.orm_store import (
    SqliteStore,
    is_sqlite_corruption_error,
    is_sqlite_schema_error,
)
from sqlalchemy.engine import Engine
from sqlalchemy.exc import DatabaseError, SQLAlchemyError
from sqlalchemy.orm import Session, sessionmaker


class ObservabilitySqliteWriter:
    """Best-effort SQLite index writer for observability records."""

    def __init__(
        self,
        path: str | Path | None = None,
        max_age_days: int = DEFAULT_OBSERVABILITY_RETENTION_DAYS,
    ) -> None:
        self._store = SqliteStore(
            path or get_observability_db_path(),
            models=ORM_MODELS,
            schema_version=OBSERVABILITY_SQLITE_SCHEMA_VERSION,
            log_prefix=OBSERVABILITY_LOG_PREFIX,
        )
        self._repository = ObservabilityEventRepository(self._store)
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

    def write(self, record: ObservabilityRecord) -> None:
        """Insert *record* into SQLite. Fire-and-forget index writes never raise."""
        if self._store.disabled:
            return

        try:
            self._repository.insert(record)
        except DatabaseError as exc:
            if not is_sqlite_corruption_error(exc):
                if is_sqlite_schema_error(exc):
                    self._store.request_schema_repair()
                return
            self._store.handle_corruption(exc)
            if self._store.disabled:
                return
            try:
                self._repository.insert(record)
            except Exception:  # noqa: BLE001
                pass
        except (SQLAlchemyError, OSError):
            self._store.dispose()

    def close(self) -> None:
        """Best-effort WAL checkpoint and dispose pooled connections."""
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


__all__ = ["ObservabilitySqliteWriter"]
