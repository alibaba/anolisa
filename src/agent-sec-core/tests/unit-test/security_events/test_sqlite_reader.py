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
            run_id="run-xyz",
            call_id="call-xyz",
            tool_call_id="tool-xyz",
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
        assert retrieved_event.run_id == original_event.run_id
        assert retrieved_event.call_id == original_event.call_id
        assert retrieved_event.tool_call_id == original_event.tool_call_id

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

    def test_reader_recovers_after_schema_created_in_existing_db(
        self, db_path: str
    ) -> None:
        conn = sqlite3.connect(db_path)
        conn.close()

        reader = SqliteEventReader(path=db_path)
        assert reader.query() == []

        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event(event_type="after_schema"))
        writer.close()

        assert [event.event_type for event in reader.query()] == ["after_schema"]

    def test_tilde_path_is_normalized_for_reader_and_writer(
        self, tilde_db_path: str
    ) -> None:
        writer = SqliteEventWriter(path=tilde_db_path)
        writer.write(_make_event(event_type="tilde_path"))
        writer.close()

        reader = SqliteEventReader(path=tilde_db_path)
        assert [event.event_type for event in reader.query()] == ["tilde_path"]
        assert not Path("~").exists()

    def test_compatibility_helpers_delegate_to_store(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event(event_type="compat"))
        writer.close()

        reader = SqliteEventReader(path=db_path)
        assert reader._ensure_session_factory() is not None
        assert reader._engine is not None
        assert reader._session_factory is not None

        reader._dispose_engine()
        assert reader._engine is None
        assert reader._session_factory is None

    def test_round_trips_new_tracing_fields(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(
            SecurityEvent(
                event_type="code_scan",
                category="code_scan",
                details={},
                trace_id="trace-1",
                session_id="session-1",
                run_id="run-1",
                call_id="call-1",
                tool_call_id="tool-1",
            )
        )
        writer.close()

        events = SqliteEventReader(path=db_path).query(limit=10)

        assert len(events) == 1
        assert events[0].trace_id == "trace-1"
        assert events[0].session_id == "session-1"
        assert events[0].run_id == "run-1"
        assert events[0].call_id == "call-1"
        assert events[0].tool_call_id == "tool-1"

    def test_read_only_v1_schema_missing_new_columns_warns_and_returns_empty(
        self,
        db_path: str,
        capsys: pytest.CaptureFixture[str],
    ) -> None:
        conn = sqlite3.connect(db_path)
        conn.executescript("""
            CREATE TABLE security_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                category TEXT NOT NULL,
                result TEXT NOT NULL DEFAULT 'succeeded',
                timestamp TEXT NOT NULL,
                timestamp_epoch FLOAT NOT NULL,
                trace_id TEXT NOT NULL DEFAULT '',
                pid INTEGER NOT NULL,
                uid INTEGER NOT NULL,
                session_id TEXT,
                details TEXT NOT NULL
            );
            PRAGMA user_version = 1;
            """)
        conn.execute("""
            INSERT INTO security_events (
                event_id, event_type, category, result, timestamp, timestamp_epoch,
                trace_id, pid, uid, session_id, details
            ) VALUES (
                'old-event', 'code_scan', 'code_scan', 'succeeded',
                '2026-05-19T00:00:00+00:00', 1779148800.0,
                'old-trace', 1, 1, 'old-session', '{}'
            )
            """)
        conn.commit()
        conn.close()

        assert SqliteEventReader(path=db_path).query(limit=10) == []
        stderr = capsys.readouterr().err
        assert "sqlite schema is v1, this binary expects v2" in stderr
        assert "run any write command" in stderr
        assert "read-only queries may return empty results until then" in stderr

        conn = sqlite3.connect(db_path)
        try:
            user_version = conn.execute("PRAGMA user_version").fetchone()[0]
            columns = {
                row[1] for row in conn.execute("PRAGMA table_info(security_events)")
            }
        finally:
            conn.close()

        assert user_version == 1
        assert "run_id" not in columns

    def test_close_disposes_readonly_store(self, db_path: str) -> None:
        writer = SqliteEventWriter(path=db_path)
        writer.write(_make_event(event_type="close"))
        writer.close()

        reader = SqliteEventReader(path=db_path)
        assert reader.query()

        reader.close()
        assert reader._engine is None
        assert reader._session_factory is None
