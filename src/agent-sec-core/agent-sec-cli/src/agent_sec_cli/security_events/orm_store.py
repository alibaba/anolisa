"""SQLAlchemy ORM storage primitives for security events."""

import re
import sqlite3
from pathlib import Path

from sqlalchemy import (
    Float,
    Index,
    Integer,
    Text,
    create_engine,
    event,
    inspect,
    text,
)
from sqlalchemy.engine import URL, Engine
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column
from sqlalchemy.schema import CreateIndex, CreateTable

_SCHEMA_VERSION = 1
_SQLITE_PRIMARY_CODE_MASK = 0xFF
_SQLITE_CORRUPTION_CODES = {
    sqlite3.SQLITE_CORRUPT,
    sqlite3.SQLITE_NOTADB,
}
_SQLITE_RETRYABLE_CODES = {
    sqlite3.SQLITE_BUSY,
    sqlite3.SQLITE_LOCKED,
    sqlite3.SQLITE_SCHEMA,
}

# Declarative column registry for convergent migration.
# To add a column: insert an entry and bump _SCHEMA_VERSION.
_COLUMNS: dict[str, str] = {
    # "severity": "TEXT DEFAULT 'info'",  # Future: uncomment and bump version
}

_IDENTIFIER_RE = re.compile(r"^[a-z_]+$")


class Base(DeclarativeBase):
    """Base class for security event ORM models."""


class SecurityEventRecord(Base):
    """ORM mapping for the queryable security event index."""

    __tablename__ = "security_events"
    __table_args__ = (
        Index("idx_event_type", "event_type"),
        Index("idx_category_epoch", "category", "timestamp_epoch"),
        Index("idx_trace_id", "trace_id"),
        Index("idx_timestamp_epoch", "timestamp_epoch"),
    )

    event_id: Mapped[str] = mapped_column(Text, primary_key=True)
    event_type: Mapped[str] = mapped_column(Text, nullable=False)
    category: Mapped[str] = mapped_column(Text, nullable=False)
    result: Mapped[str] = mapped_column(
        Text, nullable=False, server_default="succeeded"
    )
    timestamp: Mapped[str] = mapped_column(Text, nullable=False)
    timestamp_epoch: Mapped[float] = mapped_column(Float, nullable=False)
    trace_id: Mapped[str] = mapped_column(Text, nullable=False, server_default="")
    pid: Mapped[int] = mapped_column(Integer, nullable=False)
    uid: Mapped[int] = mapped_column(Integer, nullable=False)
    session_id: Mapped[str | None] = mapped_column(Text, nullable=True)
    details: Mapped[str] = mapped_column(Text, nullable=False)


def normalize_sqlite_path(path: str | Path) -> Path:
    """Return a normalized filesystem path for SQLite state."""
    return Path(path).expanduser().resolve()


def create_sqlite_engine(path: Path, *, read_only: bool = False) -> Engine:
    """Create a pooled SQLAlchemy engine for the security-events SQLite DB."""
    if read_only:
        url = URL.create(
            "sqlite+pysqlite",
            database=f"file:{path.as_posix()}",
            query={"mode": "ro", "uri": "true"},
        )
    else:
        url = URL.create("sqlite+pysqlite", database=str(path))

    engine = create_engine(
        url,
        connect_args={"check_same_thread": False},
        future=True,
    )

    @event.listens_for(engine, "connect")
    def _configure_connection(dbapi_connection, _connection_record) -> None:  # type: ignore[no-untyped-def]
        cursor = dbapi_connection.cursor()
        try:
            cursor.execute("PRAGMA busy_timeout=200")
            if read_only:
                cursor.execute("PRAGMA query_only=ON")
            else:
                cursor.execute("PRAGMA journal_mode=WAL")
                cursor.execute("PRAGMA synchronous=NORMAL")
                cursor.execute("PRAGMA wal_autocheckpoint=100")
        finally:
            cursor.close()

    return engine


def ensure_schema(engine: Engine) -> None:
    """Create tables/indexes and apply convergent column migrations."""
    with engine.begin() as conn:
        conn.execute(text("PRAGMA auto_vacuum = INCREMENTAL"))
        conn.execute(CreateTable(SecurityEventRecord.__table__, if_not_exists=True))

        existing = {
            column["name"]
            for column in inspect(conn).get_columns(SecurityEventRecord.__tablename__)
        }
        for col, typedef in _COLUMNS.items():
            if col not in existing:
                if not _IDENTIFIER_RE.match(col):
                    raise ValueError(f"Invalid column name in schema: {col!r}")
                conn.execute(
                    text(
                        f"ALTER TABLE {SecurityEventRecord.__tablename__} "
                        f"ADD COLUMN {col} {typedef}"
                    )
                )

        for index in SecurityEventRecord.__table__.indexes:
            conn.execute(CreateIndex(index, if_not_exists=True))

        # Version-gated escape hatch
        conn.execute(text("PRAGMA user_version"))  # retained for future migrations
        conn.execute(text(f"PRAGMA user_version = {_SCHEMA_VERSION}"))


def sqlite_database_files(path: Path) -> tuple[Path, Path, Path]:
    """Return the main DB path and SQLite sidecar paths."""
    return (
        path,
        Path(str(path) + "-wal"),
        Path(str(path) + "-shm"),
    )


def _sqlite_primary_error_code(exc: Exception) -> int | None:
    """Return the SQLite primary result code for DBAPI/SQLAlchemy exceptions."""
    for candidate in (getattr(exc, "orig", None), exc):
        code = getattr(candidate, "sqlite_errorcode", None)
        if isinstance(code, int):
            return code & _SQLITE_PRIMARY_CODE_MASK
    return None


def is_sqlite_corruption_error(exc: Exception) -> bool:
    """Return True only for errors that indicate true DB corruption."""
    code = _sqlite_primary_error_code(exc)
    return code in _SQLITE_CORRUPTION_CODES


def is_sqlite_retryable_error(exc: Exception) -> bool:
    """Return True for transient SQLite lock/busy/schema races."""
    code = _sqlite_primary_error_code(exc)
    return code in _SQLITE_RETRYABLE_CODES
