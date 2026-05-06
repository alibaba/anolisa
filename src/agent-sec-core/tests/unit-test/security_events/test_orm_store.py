"""Unit tests for security_events.orm_store helpers."""

import sqlite3
from pathlib import Path

import pytest
from agent_sec_cli.security_events.orm_store import (
    create_sqlite_engine,
    ensure_schema,
    is_sqlite_corruption_error,
    is_sqlite_retryable_error,
    normalize_sqlite_path,
)
from sqlalchemy import text
from sqlalchemy.exc import SQLAlchemyError


def test_sqlite_corruption_classification_uses_result_code(tmp_path: Path) -> None:
    db_path = tmp_path / "corrupt.db"
    db_path.write_bytes(b"CORRUPT_GARBAGE" * 100)

    with pytest.raises(sqlite3.DatabaseError) as exc_info:
        conn = sqlite3.connect(db_path)
        try:
            conn.execute("SELECT * FROM sqlite_master").fetchall()
        finally:
            conn.close()

    assert is_sqlite_corruption_error(exc_info.value)
    assert not is_sqlite_retryable_error(exc_info.value)


def test_sqlite_lock_classification_uses_result_code(tmp_path: Path) -> None:
    db_path = tmp_path / "locked.db"
    holder = sqlite3.connect(db_path, isolation_level=None, timeout=0)
    contender = sqlite3.connect(db_path, isolation_level=None, timeout=0)
    try:
        holder.execute("CREATE TABLE t (id INTEGER)")
        holder.execute("BEGIN IMMEDIATE")

        with pytest.raises(sqlite3.OperationalError) as exc_info:
            contender.execute("INSERT INTO t VALUES (1)")

        assert is_sqlite_retryable_error(exc_info.value)
        assert not is_sqlite_corruption_error(exc_info.value)
    finally:
        holder.close()
        contender.close()


def test_sqlite_schema_classification_is_retryable() -> None:
    class SchemaError(Exception):
        sqlite_errorcode = sqlite3.SQLITE_SCHEMA

    assert is_sqlite_retryable_error(SchemaError())
    assert not is_sqlite_corruption_error(SchemaError())


def test_write_engine_preserves_sqlite_pragmas(tmp_path: Path) -> None:
    engine = create_sqlite_engine(tmp_path / "events.db")
    try:
        ensure_schema(engine)
        with engine.connect() as conn:
            assert conn.execute(text("PRAGMA busy_timeout")).scalar_one() == 200
            assert conn.execute(text("PRAGMA journal_mode")).scalar_one() == "wal"
            assert conn.execute(text("PRAGMA synchronous")).scalar_one() == 1
            assert conn.execute(text("PRAGMA wal_autocheckpoint")).scalar_one() == 100
    finally:
        engine.dispose()


def test_readonly_engine_uses_sqlite_readonly_uri(tmp_path: Path) -> None:
    missing_db = tmp_path / "missing.db"
    engine = create_sqlite_engine(missing_db, read_only=True)
    try:
        with pytest.raises(SQLAlchemyError):
            with engine.connect():
                pass
        assert not missing_db.exists()
    finally:
        engine.dispose()


def test_normalize_sqlite_path_expands_user(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))

    assert normalize_sqlite_path("~/events.db") == tmp_path / "events.db"
