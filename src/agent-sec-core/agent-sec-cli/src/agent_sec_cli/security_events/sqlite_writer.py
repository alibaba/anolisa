"""SQLAlchemy-backed writer for security events.

Runs alongside the existing JSONL writer (dual-write pattern).
Uses SQLAlchemy engine pooling and per-write transactions for concurrency.
All exceptions are swallowed — never raises to callers.
"""

import json
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

from agent_sec_cli.security_events.config import get_db_path
from agent_sec_cli.security_events.orm_store import (
    SecurityEventRecord,
    create_sqlite_engine,
    ensure_schema,
    is_sqlite_corruption_error,
    normalize_sqlite_path,
    sqlite_database_files,
)
from agent_sec_cli.security_events.schema import SecurityEvent
from sqlalchemy import delete, text
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.engine import Engine
from sqlalchemy.exc import DatabaseError, SQLAlchemyError
from sqlalchemy.orm import Session, sessionmaker

# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------


class SqliteEventWriter:
    """Fire-and-forget SQLAlchemy writer for security events.

    SQLAlchemy owns connection pooling and transaction boundaries.  The writer
    keeps only a short lock for lazy engine initialization and corruption
    recovery; concurrent writes use independent sessions.
    """

    def __init__(
        self,
        path: str | Path | None = None,
        max_age_days: int = 30,
    ) -> None:
        self._path = normalize_sqlite_path(path or get_db_path())
        self._max_age_days = max_age_days
        self._engine_lock = threading.Lock()
        self._engine: Engine | None = None
        self._session_factory: sessionmaker[Session] | None = None
        # Per-process flag: prevents futile repeated unlink attempts within
        # a single CLI invocation (e.g. batch scan writing 200 events).
        # Resets naturally on next process — allows retry if the filesystem
        # issue was transient.
        self._disabled = False

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def write(self, event: SecurityEvent) -> None:
        """Insert *event* into SQLite. Fire-and-forget — never raises."""
        if self._disabled:
            return

        # Validate event params BEFORE opening a session to avoid occupying a
        # pooled connection during potentially failing serialization.
        try:
            values = self._event_values(event)
        except (ValueError, TypeError) as exc:
            print(
                f"[security_events] invalid event params: {exc}",
                file=sys.stderr,
            )
            return

        try:
            self._write_values(values)
        except DatabaseError as exc:
            # Only true on-disk corruption triggers destructive rebuild;
            # transient errors (lock/busy/full/readonly/schema races) are
            # skipped to keep CLI invocations fast and avoid false rebuilds.
            if not is_sqlite_corruption_error(exc):
                return

            self._handle_corruption(exc)
            if self._disabled:
                return
            try:
                self._write_values(values)
            except Exception:  # noqa: BLE001
                pass
        except (SQLAlchemyError, OSError) as exc:
            print(
                f"[security_events] sqlite write error: {exc}",
                file=sys.stderr,
            )
            self._dispose_engine()

    def _write_values(self, values: dict[str, object]) -> None:
        """Write one already-serialized event row."""
        session_factory = self._ensure_session_factory()
        if session_factory is None:
            return

        stmt = (
            sqlite_insert(SecurityEventRecord)
            .values(**values)
            .on_conflict_do_nothing(index_elements=[SecurityEventRecord.event_id])
        )
        with session_factory.begin() as session:
            session.execute(stmt)

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
            "details": json.dumps(event.details, ensure_ascii=False),
        }

    def close(self) -> None:
        """Best-effort prune, WAL checkpoint, and dispose pooled connections.

        Pruning is done here (not inside write()) because agent-sec-cli is a
        short-lived CLI: each invocation is a separate process, so an in-process
        write counter would never accumulate across invocations. Pruning at
        close() guarantees exactly one prune attempt per process lifetime,
        regardless of how many events were written.
        """
        engine = self._engine
        if engine is None:
            return

        try:
            cutoff = time.time() - (self._max_age_days * 86400)
            with engine.begin() as conn:
                conn.execute(
                    delete(SecurityEventRecord).where(
                        SecurityEventRecord.timestamp_epoch < cutoff
                    )
                )
        except Exception:  # noqa: BLE001
            pass

        # WAL checkpoint — TRUNCATE preserves the previous raw-sqlite behavior.
        try:
            with engine.connect() as conn:
                conn.execute(text("PRAGMA wal_checkpoint(TRUNCATE)"))
        except Exception:  # noqa: BLE001
            pass

        self._dispose_engine()

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _ensure_session_factory(self) -> sessionmaker[Session] | None:
        """Lazily open the database and apply schema migrations.

        If corruption is detected, deletes the corrupt file so the caller can
        retry once with a fresh DB.
        """
        if self._session_factory is not None:
            return self._session_factory

        with self._engine_lock:
            if self._session_factory is not None:
                return self._session_factory

            try:
                self._open_session_factory()
                return self._session_factory
            except DatabaseError as exc:
                if not is_sqlite_corruption_error(exc):
                    return None
                self._handle_corruption(exc)
                if self._disabled:
                    return None

                try:
                    self._open_session_factory()
                    return self._session_factory
                except (SQLAlchemyError, OSError):
                    return None
            except (SQLAlchemyError, OSError):
                return None

        return None

    def _open_session_factory(self) -> None:
        """Open the engine, ensure schema, and cache a session factory."""
        self._path.parent.mkdir(parents=True, exist_ok=True)
        engine = create_sqlite_engine(self._path)
        try:
            ensure_schema(engine)
            self._engine = engine
            self._session_factory = sessionmaker(
                bind=engine,
                expire_on_commit=False,
                future=True,
            )
            try:
                self._path.chmod(0o600)
            except OSError:
                pass
        except Exception:
            engine.dispose()
            raise

    def _dispose_engine(self) -> None:
        """Dispose SQLAlchemy engine state and clear session factory."""
        if self._engine is not None:
            try:
                self._engine.dispose()
            except Exception:  # noqa: BLE001
                pass
        self._engine = None
        self._session_factory = None

    def _handle_corruption(self, exc: Exception) -> None:
        """Delete the corrupt database and prepare for a fresh start.

        The SQLite DB is an expendable queryable index — JSONL is the source
        of truth. A corrupt DB has no forensic value (it's unreadable), so we
        simply delete it rather than renaming/accumulating corrupt copies.
        """
        print(
            f"[security_events] corrupt DB detected, recreating: {exc}",
            file=sys.stderr,
        )
        self._dispose_engine()

        # Delete corrupt file — next _ensure_session_factory() will create fresh.
        try:
            for db_file in sqlite_database_files(self._path):
                db_file.unlink(missing_ok=True)
        except OSError as delete_exc:
            self._disabled = True
            print(
                f"[security_events] cannot delete corrupt db, "
                f"writer disabled: {delete_exc}",
                file=sys.stderr,
            )
