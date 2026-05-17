"""Unit tests for observability dual persistence."""

import json
import sqlite3
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import agent_sec_cli.observability as observability
import agent_sec_cli.observability.sqlite_writer as observability_sqlite_writer_module
import agent_sec_cli.security_events.orm_store as orm_store
import pytest
from agent_sec_cli.observability import record_observability
from agent_sec_cli.observability.models import (
    OBSERVABILITY_SQLITE_SCHEMA_VERSION,
)
from agent_sec_cli.observability.schema import validate_observability_record
from agent_sec_cli.observability.sqlite_writer import ObservabilitySqliteWriter
from agent_sec_cli.observability.writer import ObservabilityWriter


def _fresh_observed_at() -> str:
    return datetime.now(timezone.utc).isoformat()


def _payload(**overrides: Any) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "hook": "before_agent_run",
        "observedAt": _fresh_observed_at(),
        "metadata": {
            "sessionId": "session-123",
            "runId": "run-123",
        },
        "metrics": {
            "prompt": "Summarize ./README.md",
            "model_id": "qwen3",
            "model_provider": "dashscope",
        },
    }
    payload.update(overrides)
    return payload


def _jsonl_records(path: Path) -> list[dict[str, Any]]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _sqlite_columns(path: Path) -> set[str]:
    conn = sqlite3.connect(path)
    try:
        return {
            row[1] for row in conn.execute("PRAGMA table_info(observability_events)")
        }
    finally:
        conn.close()


def _sqlite_user_version(path: Path) -> int:
    conn = sqlite3.connect(path)
    try:
        return int(conn.execute("PRAGMA user_version").fetchone()[0])
    finally:
        conn.close()


def _sqlite_row_count(path: Path) -> int:
    conn = sqlite3.connect(path)
    try:
        return int(
            conn.execute("SELECT count(*) FROM observability_events").fetchone()[0]
        )
    finally:
        conn.close()


def test_observability_jsonl_writer_only_writes_jsonl(
    tmp_path: Path,
) -> None:
    record = validate_observability_record(_payload())
    writer = ObservabilityWriter(path=tmp_path / "observability.jsonl")

    writer.write(record)

    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert records[0]["hook"] == "before_agent_run"
    assert records[0]["metadata"]["sessionId"] == "session-123"
    assert not (tmp_path / "observability.db").exists()
    assert not (tmp_path / "security-events.jsonl").exists()
    assert not (tmp_path / "security-events.db").exists()


def test_observability_sqlite_writer_only_writes_independent_sqlite_index(
    tmp_path: Path,
) -> None:
    record = validate_observability_record(_payload())
    writer = ObservabilitySqliteWriter(path=tmp_path / "observability.db")

    writer.write(record)
    writer.close()

    assert not (tmp_path / "observability.jsonl").exists()
    assert not (tmp_path / "security-events.jsonl").exists()
    assert not (tmp_path / "security-events.db").exists()

    conn = sqlite3.connect(tmp_path / "observability.db")
    try:
        row = conn.execute("""
            SELECT id, hook, observed_at, session_id, run_id, metrics_json,
                   metadata_json, call_id, tool_call_id
            FROM observability_events
            """).fetchone()
        indexes = {
            item[1]
            for item in conn.execute(
                "PRAGMA index_list(observability_events)"
            ).fetchall()
        }
        session_run_index_columns = [
            item[2]
            for item in conn.execute(
                "PRAGMA index_info(idx_observability_session_run_observed_at_epoch)"
            ).fetchall()
        ]
    finally:
        conn.close()

    assert row is not None
    assert row[0] == 1
    assert row[1] == "before_agent_run"
    assert row[2] == record.to_record()["observedAt"]
    assert row[3] == "session-123"
    assert row[4] == "run-123"
    assert json.loads(row[5])["prompt"] == "Summarize ./README.md"
    assert json.loads(row[6]) == {"sessionId": "session-123", "runId": "run-123"}
    assert row[7] is None
    assert row[8] is None
    assert {
        "idx_observability_observed_at_epoch",
        "idx_observability_hook_observed_at_epoch",
        "idx_observability_session_observed_at_epoch",
        "idx_observability_session_run_observed_at_epoch",
    }.issubset(indexes)
    assert "idx_observability_run_observed_at_epoch" not in indexes
    assert session_run_index_columns == [
        "session_id",
        "run_id",
        "observed_at_epoch",
    ]
    assert _sqlite_user_version(tmp_path / "observability.db") == (
        OBSERVABILITY_SQLITE_SCHEMA_VERSION
    )


def test_observability_sqlite_columns_are_core_index_and_correlation_only(
    tmp_path: Path,
) -> None:
    record = validate_observability_record(_payload())
    writer = ObservabilitySqliteWriter(path=tmp_path / "observability.db")

    writer.write(record)
    writer.close()

    columns = _sqlite_columns(tmp_path / "observability.db")
    assert columns == {
        "id",
        "hook",
        "observed_at",
        "observed_at_epoch",
        "session_id",
        "run_id",
        "metrics_json",
        "metadata_json",
        "call_id",
        "tool_call_id",
    }


def test_observability_sqlite_writer_prunes_on_close_not_write(
    tmp_path: Path,
) -> None:
    now = datetime.now(timezone.utc)
    stale_record = validate_observability_record(
        _payload(
            observedAt=(now - timedelta(days=8)).isoformat(),
            metadata={"sessionId": "stale-session", "runId": "stale-run"},
        )
    )
    fresh_record = validate_observability_record(
        _payload(
            observedAt=now.isoformat(),
            metadata={"sessionId": "fresh-session", "runId": "fresh-run"},
        )
    )
    writer = ObservabilitySqliteWriter(
        path=tmp_path / "observability.db",
        max_age_days=7,
    )

    writer.write(stale_record)
    writer.write(fresh_record)

    assert _sqlite_row_count(tmp_path / "observability.db") == 2

    writer.close()

    conn = sqlite3.connect(tmp_path / "observability.db")
    try:
        rows = conn.execute("""
            SELECT session_id
            FROM observability_events
            ORDER BY observed_at_epoch
            """).fetchall()
    finally:
        conn.close()

    assert rows == [("fresh-session",)]


def test_observability_sqlite_writer_closes_through_maintenance_gate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db_path = tmp_path / "observability.db"
    writer = ObservabilitySqliteWriter(path=db_path)
    writer.write(validate_observability_record(_payload()))
    gated_paths: list[Path] = []

    def fake_run_sqlite_maintenance_if_due(
        db_path_arg: str | Path,
        maintenance: Callable[[], None],
        *,
        interval_seconds: float = 0,
        now: float | None = None,
    ) -> bool:
        gated_paths.append(Path(db_path_arg))
        maintenance()
        return True

    monkeypatch.setattr(
        observability_sqlite_writer_module,
        "run_sqlite_maintenance_if_due",
        fake_run_sqlite_maintenance_if_due,
        raising=False,
    )

    writer.close()

    assert gated_paths == [db_path.resolve()]
    assert writer._engine is None
    assert writer._session_factory is None


def test_observability_sqlite_writer_uses_schema_version_fast_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db_path = tmp_path / "observability.db"
    writer = ObservabilitySqliteWriter(path=db_path)
    writer.write(validate_observability_record(_payload()))
    writer.close()

    assert _sqlite_user_version(db_path) == OBSERVABILITY_SQLITE_SCHEMA_VERSION

    def fail_full_schema(*args: Any, **kwargs: Any) -> None:
        raise AssertionError("current observability schema should use the fast path")

    monkeypatch.setattr(orm_store, "ensure_schema", fail_full_schema)

    writer = ObservabilitySqliteWriter(path=db_path)
    writer.write(
        validate_observability_record(
            _payload(metadata={"sessionId": "session-456", "runId": "run-456"})
        )
    )
    writer.close()

    assert _sqlite_row_count(db_path) == 2


def test_record_observability_dual_writes_jsonl_and_sqlite(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENT_SEC_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(observability, "_writer", None, raising=False)
    monkeypatch.setattr(observability, "_sqlite_writer", None, raising=False)
    record = validate_observability_record(_payload())

    record_observability(record)
    observability.get_sqlite_writer().close()

    assert _jsonl_records(tmp_path / "observability.jsonl")[0]["hook"] == (
        "before_agent_run"
    )
    assert (tmp_path / "observability.db").exists()
    assert not (tmp_path / "security-events.jsonl").exists()
    assert not (tmp_path / "security-events.db").exists()


def test_observability_writer_indexes_llm_call_correlation_only(
    tmp_path: Path,
) -> None:
    record = validate_observability_record(
        _payload(
            hook="after_llm_call",
            metadata={
                "sessionId": "session-123",
                "runId": "run-123",
                "callId": "call-123",
            },
            metrics={
                "latency_ms": 125.5,
                "outcome": "failure",
                "response": {"error": "timeout"},
            },
        )
    )
    writer = ObservabilitySqliteWriter(path=tmp_path / "observability.db")

    writer.write(record)
    writer.close()

    conn = sqlite3.connect(tmp_path / "observability.db")
    try:
        row = conn.execute("""
            SELECT call_id, tool_call_id, metrics_json
            FROM observability_events
            """).fetchone()
    finally:
        conn.close()

    assert row[0] == "call-123"
    assert row[1] is None
    assert json.loads(row[2]) == {
        "latency_ms": 125.5,
        "outcome": "failure",
        "response": {"error": "timeout"},
    }


def test_observability_writer_indexes_tool_call_correlation_only(
    tmp_path: Path,
) -> None:
    record = validate_observability_record(
        _payload(
            hook="after_tool_call",
            metadata={
                "sessionId": "session-123",
                "runId": "run-123",
                "callId": "call-123",
                "toolCallId": "tool-call-123",
            },
            metrics={
                "result": {"ok": True},
                "duration_ms": 25,
                "result_size_bytes": 128,
            },
        )
    )
    writer = ObservabilitySqliteWriter(path=tmp_path / "observability.db")

    writer.write(record)
    writer.close()

    conn = sqlite3.connect(tmp_path / "observability.db")
    try:
        row = conn.execute("""
            SELECT call_id, tool_call_id, metrics_json
            FROM observability_events
            """).fetchone()
    finally:
        conn.close()

    assert row[0] == "call-123"
    assert row[1] == "tool-call-123"
    assert json.loads(row[2]) == {
        "result": {"ok": True},
        "duration_ms": 25,
        "result_size_bytes": 128,
    }
