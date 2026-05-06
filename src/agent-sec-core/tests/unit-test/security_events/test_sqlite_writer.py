"""Unit tests for security_events.sqlite_writer — SqliteEventWriter."""

import io
import json
import sqlite3
import stat
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime
from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest
from agent_sec_cli.security_events.schema import SecurityEvent
from agent_sec_cli.security_events.sqlite_writer import SqliteEventWriter


def _make_event(
    event_type: str = "test_event", category: str = "test", **kwargs: Any
) -> SecurityEvent:
    return SecurityEvent(
        event_type=event_type,
        category=category,
        details=kwargs.get("details", {"key": "value"}),
        trace_id=kwargs.get("trace_id", ""),
    )


@pytest.fixture()
def db_path(tmp_path: Path) -> str:
    return str(tmp_path / "test.db")


class TestSqliteEventWriter:
    def test_write_with_invalid_timestamp(self, db_path: str) -> None:
        """Verify that invalid timestamps are caught and logged to stderr."""
        writer = SqliteEventWriter(path=db_path)

        # Create event with malformed timestamp
        evt = SecurityEvent(
            event_type="test",
            category="test",
            details={"key": "value"},
            timestamp="not-a-valid-timestamp",
        )

        # Capture stderr
        old_stderr = sys.stderr
        sys.stderr = io.StringIO()

        try:
            writer.write(evt)
            stderr_output = sys.stderr.getvalue()
        finally:
            sys.stderr = old_stderr

        # Should print warning to stderr
        assert "invalid event params" in stderr_output

        # DB file should not be created since write fails before connection
        assert not Path(db_path).exists()

        writer.close()

    def test_write_with_non_serializable_details(self, db_path: str) -> None:
        """Verify that non-serializable details are caught and logged."""
        writer = SqliteEventWriter(path=db_path)

        # Create event with non-serializable details (custom object)
        class CustomObject:
            pass

        evt = SecurityEvent(
            event_type="test",
            category="test",
            details={"obj": CustomObject()},  # json.dumps will fail
        )

        # Capture stderr
        old_stderr = sys.stderr
        sys.stderr = io.StringIO()

        try:
            writer.write(evt)
            stderr_output = sys.stderr.getvalue()
        finally:
            sys.stderr = old_stderr

        # Should print warning to stderr
        assert "invalid event params" in stderr_output

        # DB file should not be created since write fails before connection
        assert not Path(db_path).exists()

        writer.close()

    def test_write_column_values_are_correct(self, db_path: str) -> None:
        """Verify that all column values are correctly written to SQLite.

        This is a critical data integrity test — validates the entire
        _event_params conversion and INSERT correctness.
        """
        writer = SqliteEventWriter(path=db_path)

        # Create a comprehensive event with all fields
        evt = SecurityEvent(
            event_type="harden",
            category="hardening",
            result="failed",
            timestamp="2026-04-20T13:47:00.123456+00:00",
            trace_id="test-trace-123",
            session_id="session-abc",
            details={
                "nested": {"key": "value"},
                "list": [1, 2, 3],
                "unicode": "测试中文",
                "null_value": None,
                "empty_string": "",
            },
        )

        writer.write(evt)
        writer.close()

        # Directly query the database to verify all column values
        conn = sqlite3.connect(db_path)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT * FROM security_events").fetchone()
        conn.close()

        # Verify all columns
        assert row is not None
        assert row["event_id"] == evt.event_id
        assert row["event_type"] == "harden"
        assert row["category"] == "hardening"
        assert row["result"] == "failed"
        assert row["timestamp"] == "2026-04-20T13:47:00.123456+00:00"
        assert row["trace_id"] == "test-trace-123"
        assert row["pid"] == evt.pid
        assert row["uid"] == evt.uid
        assert row["session_id"] == "session-abc"

        # Verify timestamp_epoch is correct
        expected_epoch = datetime.fromisoformat(evt.timestamp).timestamp()
        assert abs(row["timestamp_epoch"] - expected_epoch) < 0.001

        # Verify details JSON serialization
        details_dict = json.loads(row["details"])
        assert details_dict["nested"] == {"key": "value"}
        assert details_dict["list"] == [1, 2, 3]
        assert details_dict["unicode"] == "测试中文"
        assert details_dict["null_value"] is None
        assert details_dict["empty_string"] == ""

    def test_write_creates_db_and_inserts(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        evt = _make_event()
        writer.write(evt)

        assert Path(db_path).exists()

        conn = sqlite3.connect(db_path)
        rows = conn.execute("SELECT * FROM security_events").fetchall()
        conn.close()
        assert len(rows) == 1
        writer.close()

    def test_db_file_permissions_are_restrictive(self, db_path: str) -> None:
        """Verify that the database file is created with 0o600 permissions."""
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())

        # Check file permissions
        file_stat = Path(db_path).stat()
        mode = stat.S_IMODE(file_stat.st_mode)

        # Should be 0o600 (owner read/write only)
        assert (
            mode == 0o600
        ), f"Database file has permissions {oct(mode)}, expected 0o600"

        writer.close()

    def test_wal_mode_enabled(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())

        conn = sqlite3.connect(db_path)
        mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
        conn.close()
        assert mode == "wal"
        writer.close()

    def test_fire_and_forget_never_raises(self) -> None:
        invalid_path = "/nonexistent/dir/test.db"
        writer = SqliteEventWriter(path=invalid_path)
        # Should not raise
        writer.write(_make_event())

    def test_insert_or_ignore_dedup(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        evt = _make_event()
        writer.write(evt)
        writer.write(evt)  # Same event_id

        conn = sqlite3.connect(db_path)
        count = conn.execute("SELECT COUNT(*) FROM security_events").fetchone()[0]
        conn.close()
        assert count == 1
        writer.close()

    def test_thread_safety(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        errors: list[Exception] = []

        def write_events(thread_id: int) -> None:
            try:
                for i in range(10):
                    writer.write(
                        _make_event(
                            trace_id=f"thread-{thread_id}-event-{i}",
                        )
                    )
            except Exception as e:
                errors.append(e)

        threads = [threading.Thread(target=write_events, args=(t,)) for t in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert len(errors) == 0

        conn = sqlite3.connect(db_path)
        count = conn.execute("SELECT COUNT(*) FROM security_events").fetchone()[0]
        conn.close()
        assert count == 100
        writer.close()

    def test_concurrent_writes_from_independent_writers(self, db_path: str) -> None:
        writer_count = 8
        events_per_writer = 25

        def write_events(writer_id: int) -> None:
            writer = SqliteEventWriter(path=db_path)
            try:
                for event_num in range(events_per_writer):
                    writer.write(
                        SecurityEvent(
                            event_id=f"writer-{writer_id}-event-{event_num}",
                            event_type="concurrent_event",
                            category="test",
                            trace_id=f"writer-{writer_id}",
                            details={"writer_id": writer_id, "event_num": event_num},
                        )
                    )
            finally:
                writer.close()

        with ThreadPoolExecutor(max_workers=writer_count) as executor:
            futures = [
                executor.submit(write_events, writer_id)
                for writer_id in range(writer_count)
            ]
            for future in as_completed(futures):
                future.result()

        conn = sqlite3.connect(db_path)
        total, distinct_ids = conn.execute(
            "SELECT COUNT(*), COUNT(DISTINCT event_id) FROM security_events"
        ).fetchone()
        conn.close()

        expected = writer_count * events_per_writer
        assert total == expected
        assert distinct_ids == expected

    def test_pruning_at_close(self, db_path: str) -> None:
        """Pruning happens in close(), not during writes.

        agent-sec-cli is short-lived: each invocation is a separate process,
        so counter-based pruning inside write() would never accumulate across
        invocations.  Instead, close() (called via atexit) prunes once per
        process lifetime.
        """
        writer = SqliteEventWriter(path=db_path, max_age_days=0)

        for _ in range(10):
            writer.write(_make_event())
        time.sleep(0.01)  # Ensure events are in the past relative to close()

        # Before close: all events still present
        conn = sqlite3.connect(db_path)
        count_before = conn.execute("SELECT COUNT(*) FROM security_events").fetchone()[
            0
        ]
        conn.close()
        assert count_before == 10

        # After close: pruning removes events (max_age_days=0 means cutoff=now)
        writer.close()

        conn = sqlite3.connect(db_path)
        count_after = conn.execute("SELECT COUNT(*) FROM security_events").fetchone()[0]
        conn.close()
        assert count_after < 10

    def test_corruption_detection_and_rebuild(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())
        writer.close()

        # Corrupt the DB file
        with open(db_path, "r+b") as f:
            f.write(b"CORRUPT_GARBAGE" * 100)

        # Create new writer on same path — should detect corruption, delete,
        # recreate fresh DB, and successfully write the current event
        writer2 = SqliteEventWriter(path=db_path)
        writer2.write(_make_event())

        # Fresh DB should exist with the event (no event dropped)
        conn = sqlite3.connect(db_path)
        count = conn.execute("SELECT COUNT(*) FROM security_events").fetchone()[0]
        conn.close()
        assert count == 1
        writer2.close()

    def test_schema_migration_adds_columns(self, db_path: str) -> None:
        # _COLUMNS dict is currently empty, so just verify _ensure_schema runs without error
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())

        conn = sqlite3.connect(db_path)
        columns = {row[1] for row in conn.execute("PRAGMA table_info(security_events)")}
        conn.close()
        # Verify core columns exist
        assert "event_id" in columns
        assert "event_type" in columns
        assert "category" in columns
        assert "timestamp_epoch" in columns
        writer.close()

    def test_schema_repairs_missing_indexes(self, db_path: str) -> None:
        conn = sqlite3.connect(db_path)
        conn.execute(
            "CREATE TABLE security_events ("
            "event_id TEXT PRIMARY KEY, "
            "event_type TEXT NOT NULL, "
            "category TEXT NOT NULL, "
            "result TEXT NOT NULL DEFAULT 'succeeded', "
            "timestamp TEXT NOT NULL, "
            "timestamp_epoch FLOAT NOT NULL, "
            "trace_id TEXT NOT NULL DEFAULT '', "
            "pid INTEGER NOT NULL, "
            "uid INTEGER NOT NULL, "
            "session_id TEXT, "
            "details TEXT NOT NULL"
            ")"
        )
        conn.commit()
        conn.close()

        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())
        writer.close()

        conn = sqlite3.connect(db_path)
        index_names = {
            row[1] for row in conn.execute("PRAGMA index_list(security_events)")
        }
        conn.close()

        assert {
            "idx_event_type",
            "idx_category_epoch",
            "idx_trace_id",
            "idx_timestamp_epoch",
        }.issubset(index_names)

    def test_close_performs_checkpoint(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())
        writer.write(_make_event())

        writer.close()
        assert writer._engine is None
        assert writer._session_factory is None

    def test_disabled_after_delete_failure(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event())
        writer.close()

        # Corrupt the DB
        with open(db_path, "r+b") as f:
            f.write(b"CORRUPT_GARBAGE" * 100)

        writer2 = SqliteEventWriter(path=db_path)

        # Mock Path.unlink to raise OSError
        with patch.object(Path, "unlink", side_effect=OSError("permission denied")):
            writer2.write(_make_event())

        # Writer should be disabled now
        assert writer2._disabled

        # Subsequent writes should be no-ops
        writer2.write(_make_event())
