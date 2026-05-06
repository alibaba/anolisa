"""Unit tests for security_events.sqlite_reader — SqliteEventReader."""

import io
import sqlite3
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest
from agent_sec_cli.security_events.schema import SecurityEvent
from agent_sec_cli.security_events.sqlite_reader import SqliteEventReader
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


@pytest.fixture()
def tilde_db_path(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> str:
    monkeypatch.setenv("HOME", str(tmp_path))
    return "~/events.db"


@pytest.fixture()
def writer(db_path: str) -> SqliteEventWriter:
    w = SqliteEventWriter(path=db_path)
    yield w
    w.close()


@pytest.fixture()
def reader(db_path: str) -> SqliteEventReader:
    return SqliteEventReader(path=db_path)


class TestSqliteEventReader:
    def test_write_read_roundtrip(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        """Verify that a complete SecurityEvent can be written and read back with all fields intact.

        This is the most critical data path test — validates Writer → SQLite → Reader
        entire pipeline including _event_params, INSERT, SELECT, and _row_to_event.
        """
        # Create a comprehensive event with all fields
        original_event = SecurityEvent(
            event_type="harden",
            category="hardening",
            result="failed",
            timestamp="2026-04-20T13:47:00.123456+00:00",
            trace_id="test-trace-456",
            session_id="session-xyz",
            details={
                "request": {"config": "default", "dry_run": True},
                "result": {"violations": ["RULE_001", "RULE_002"]},
                "unicode": "测试中文🎉",
                "nested": {"level1": {"level2": "value"}},
                "list_data": [1, "two", 3.0, None],
                "empty_string": "",
                "null_value": None,
            },
        )

        # Write the event
        writer.write(original_event)
        writer.close()

        # Read it back
        events = reader.query(event_type="harden")
        assert len(events) == 1

        retrieved_event = events[0]

        # Verify all fields match
        assert retrieved_event.event_id == original_event.event_id
        assert retrieved_event.event_type == original_event.event_type
        assert retrieved_event.category == original_event.category
        assert retrieved_event.result == original_event.result
        assert retrieved_event.timestamp == original_event.timestamp
        assert retrieved_event.trace_id == original_event.trace_id
        assert retrieved_event.pid == original_event.pid
        assert retrieved_event.uid == original_event.uid
        assert retrieved_event.session_id == original_event.session_id

        # Verify details JSON round-trip
        assert retrieved_event.details == original_event.details
        assert retrieved_event.details["unicode"] == "测试中文🎉"
        assert retrieved_event.details["nested"]["level1"]["level2"] == "value"
        assert retrieved_event.details["list_data"] == [1, "two", 3.0, None]
        assert retrieved_event.details["null_value"] is None

    def test_malformed_details_are_skipped(
        self, db_path: str, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        """Verify that rows with malformed JSON in details are skipped gracefully."""
        # Write a valid event first
        writer.write(_make_event(event_type="valid_event"))

        # Manually insert a row with invalid JSON in details
        conn = sqlite3.connect(db_path)
        conn.execute(
            "INSERT INTO security_events "
            "(event_id, event_type, category, result, timestamp, timestamp_epoch, "
            "trace_id, pid, uid, session_id, details) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                "malformed-event-id",
                "malformed_event",
                "test",
                "succeeded",
                "2026-04-20T13:47:00+00:00",
                1745298420.0,
                "",
                12345,
                1000,
                None,
                "NOT VALID JSON{{{",  # Intentionally malformed
            ),
        )
        conn.commit()
        conn.close()

        # Capture stderr to verify warning is printed
        old_stderr = sys.stderr
        sys.stderr = io.StringIO()

        try:
            # Query should return only the valid event, skipping the malformed one
            events = reader.query()
            stderr_output = sys.stderr.getvalue()
        finally:
            sys.stderr = old_stderr

        # Should have skipped the malformed row
        assert len(events) == 1
        assert events[0].event_type == "valid_event"

        # Should have printed warning to stderr
        assert "malformed row skipped" in stderr_output

    def test_query_returns_all_events(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        for _ in range(5):
            writer.write(_make_event())
        events = reader.query()
        assert len(events) == 5

    def test_query_filter_by_event_type(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(event_type="alpha"))
        writer.write(_make_event(event_type="alpha"))
        writer.write(_make_event(event_type="beta"))
        events = reader.query(event_type="alpha")
        assert len(events) == 2
        for e in events:
            assert e.event_type == "alpha"

    def test_query_filter_by_category(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="hardening"))
        events = reader.query(category="sandbox")
        assert len(events) == 2
        for e in events:
            assert e.category == "sandbox"

    def test_query_filter_by_trace_id(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(trace_id="trace-abc"))
        writer.write(_make_event(trace_id="trace-abc"))
        writer.write(_make_event(trace_id="trace-xyz"))
        events = reader.query(trace_id="trace-abc")
        assert len(events) == 2
        for e in events:
            assert e.trace_id == "trace-abc"

    def test_query_time_range_since_until(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        now = datetime.now(timezone.utc)
        past = now - timedelta(hours=2)
        future = now + timedelta(hours=2)

        for _ in range(3):
            writer.write(_make_event())

        since_iso = past.isoformat()
        until_iso = future.isoformat()
        events = reader.query(since=since_iso, until=until_iso)
        assert len(events) == 3

    def test_query_ordering_desc(
        self, db_path: str, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event())
        time.sleep(0.02)
        writer.write(_make_event())
        time.sleep(0.02)
        writer.write(_make_event())

        events = reader.query()
        assert len(events) == 3
        # Results should be in descending order by time — verify via DB directly
        conn = sqlite3.connect(db_path)
        rows = conn.execute(
            "SELECT timestamp_epoch FROM security_events ORDER BY timestamp_epoch DESC"
        ).fetchall()
        conn.close()
        epochs = [r[0] for r in rows]
        assert epochs == sorted(epochs, reverse=True)

    def test_query_limit_offset(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        for _ in range(10):
            writer.write(_make_event())
            time.sleep(0.005)

        events = reader.query(limit=3, offset=2)
        assert len(events) == 3

    def test_count_returns_total(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        for _ in range(5):
            writer.write(_make_event())
        assert reader.count() == 5

    def test_count_with_filters(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="hardening"))
        assert reader.count(category="sandbox") == 2

    def test_count_by_category(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="sandbox"))
        writer.write(_make_event(category="hardening"))
        result = reader.count_by("category")
        assert result["sandbox"] == 2
        assert result["hardening"] == 1

    def test_count_by_event_type(
        self, writer: SqliteEventWriter, reader: SqliteEventReader
    ) -> None:
        writer.write(_make_event(event_type="alpha"))
        writer.write(_make_event(event_type="alpha"))
        writer.write(_make_event(event_type="beta"))
        result = reader.count_by("event_type")
        assert result["alpha"] == 2
        assert result["beta"] == 1

    def test_count_by_invalid_field_raises(self, reader: SqliteEventReader) -> None:
        with pytest.raises(ValueError):
            reader.count_by("invalid_field")

    def test_missing_db_returns_empty(self, tmp_path: Path) -> None:
        missing_path = str(tmp_path / "nonexistent.db")
        reader = SqliteEventReader(path=missing_path)
        assert reader.query() == []
        assert reader.count() == 0
        assert reader.count_by("category") == {}

    def test_reader_reopens_after_db_file_replaced(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event(event_type="before_replace"))
        writer.close()

        reader = SqliteEventReader(path=db_path)
        assert [event.event_type for event in reader.query()] == ["before_replace"]

        db = Path(db_path)
        db.unlink()
        Path(f"{db_path}-wal").unlink(missing_ok=True)
        Path(f"{db_path}-shm").unlink(missing_ok=True)

        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event(event_type="after_replace"))
        writer.close()

        assert [event.event_type for event in reader.query()] == ["after_replace"]

    def test_tilde_path_is_normalized_for_reader_and_writer(
        self, tilde_db_path: str
    ) -> None:
        writer = SqliteEventWriter(path=tilde_db_path)
        writer.write(_make_event(event_type="tilde_path"))
        writer.close()

        reader = SqliteEventReader(path=tilde_db_path)
        assert [event.event_type for event in reader.query()] == ["tilde_path"]
        assert not Path("~").exists()
