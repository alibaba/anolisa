"""Unit tests for telemetry writer."""

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import MagicMock

import agent_sec_cli.telemetry as telemetry
from agent_sec_cli.security_events.schema import SecurityEvent
from agent_sec_cli.telemetry import writer as telemetry_writer
from agent_sec_cli.telemetry.writer import (
    TelemetryWriter,
    get_writer,
    record_security_event_telemetry,
)


def _event() -> SecurityEvent:
    return SecurityEvent(
        event_id="event-1",
        event_type="pii_scan",
        category="pii_scan",
        result="succeeded",
        timestamp="2026-06-15T12:00:00+00:00",
        trace_id="trace-1",
        details={
            "request": {"source": "manual"},
            "result": {"verdict": "deny", "summary": {"total": 1}, "elapsed_ms": 3},
        },
    )


def test_telemetry_package_exports_public_api() -> None:
    assert telemetry.record_security_event_telemetry is record_security_event_telemetry
    assert telemetry.__all__ == ["record_security_event_telemetry"]


def test_telemetry_package_imports_in_clean_interpreter() -> None:
    probe = """
import agent_sec_cli.telemetry as telemetry

print(",".join(telemetry.__all__))
"""

    result = subprocess.run(
        [sys.executable, "-c", probe],
        text=True,
        capture_output=True,
        check=True,
    )

    assert result.stdout.strip() == "record_security_event_telemetry"


def test_writer_skips_missing_target_without_creating_file(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "missing" / "agent-sec-core.jsonl"
    writer = TelemetryWriter(path=path)
    log_failure = MagicMock()
    monkeypatch.setattr(telemetry_writer, "_log_telemetry_write_failure", log_failure)

    writer.write({"component.name": "agent-sec-core"})

    assert not path.exists()
    assert not path.parent.exists()
    log_failure.assert_not_called()


def test_writer_appends_existing_file(tmp_path: Path) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    path.write_text("", encoding="utf-8")
    writer = TelemetryWriter(path=path)

    writer.write({"component.name": "agent-sec-core", "seccore.event_id": "event-1"})

    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 1
    assert json.loads(lines[0]) == {
        "component.name": "agent-sec-core",
        "seccore.event_id": "event-1",
    }
    assert not Path(f"{path}.lock").exists()
    assert list(tmp_path.glob("agent-sec-core.jsonl.*")) == []


def test_writer_uses_short_lived_flock_on_target_fd(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    path.write_text("", encoding="utf-8")
    writer = TelemetryWriter(path=path)
    flock_calls: list[tuple[int, int]] = []

    def record_flock(fd: int, operation: int) -> None:
        flock_calls.append((fd, operation))

    monkeypatch.setattr(telemetry_writer.fcntl, "flock", record_flock)

    writer.write({"seq": 1})

    assert [operation for _, operation in flock_calls] == [
        telemetry_writer.fcntl.LOCK_EX,
        telemetry_writer.fcntl.LOCK_UN,
    ]
    assert flock_calls[0][0] == flock_calls[1][0]
    assert not Path(f"{path}.lock").exists()


def test_writer_reopens_target_path_after_rename_rotation(tmp_path: Path) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    rotated_path = tmp_path / "agent-sec-core.jsonl.rotated"
    path.write_text("", encoding="utf-8")
    writer = TelemetryWriter(path=path)

    writer.write({"seq": 1})
    path.rename(rotated_path)
    path.write_text("", encoding="utf-8")
    writer.write({"seq": 2})

    assert json.loads(rotated_path.read_text(encoding="utf-8").splitlines()[0]) == {
        "seq": 1
    }
    assert json.loads(path.read_text(encoding="utf-8").splitlines()[0]) == {"seq": 2}


def test_writer_swallows_missing_file_race_after_exists_check(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    path.write_text("", encoding="utf-8")
    writer = TelemetryWriter(path=path)
    open_calls = 0

    def race_open(file_path: Path, flags: int) -> int:
        nonlocal open_calls
        open_calls += 1
        raise FileNotFoundError(file_path)

    monkeypatch.setattr(telemetry_writer.os, "open", race_open)

    writer.write({"seq": 1})

    assert open_calls == 1
    assert path.read_text(encoding="utf-8") == ""


def test_record_security_event_telemetry_uses_mapping_and_writer(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    path.write_text("", encoding="utf-8")
    monkeypatch.setenv("AGENT_SEC_TELEMETRY_LOG_PATH", str(path))
    monkeypatch.setattr(telemetry_writer, "_writer", None)

    record_security_event_telemetry(_event())

    record = json.loads(path.read_text(encoding="utf-8").splitlines()[0])
    assert record["component.name"] == "agent-sec-core"
    assert record["seccore.event_id"] == "event-1"
    assert record["seccore.verdict"] == "deny"


def test_record_security_event_telemetry_skips_mapping_when_target_missing(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "missing.jsonl"
    monkeypatch.setenv("AGENT_SEC_TELEMETRY_LOG_PATH", str(path))
    monkeypatch.setattr(telemetry_writer, "_writer", None)
    mapper = MagicMock(return_value={"component.name": "agent-sec-core"})
    monkeypatch.setattr(telemetry_writer, "build_telemetry_security_event", mapper)

    record_security_event_telemetry(_event())

    mapper.assert_not_called()
    assert not path.exists()


def test_record_security_event_telemetry_swallows_mapping_errors(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    path.write_text("", encoding="utf-8")
    monkeypatch.setenv("AGENT_SEC_TELEMETRY_LOG_PATH", str(path))
    monkeypatch.setattr(telemetry_writer, "_writer", None)

    def fail_mapping(event: SecurityEvent) -> dict[str, object]:
        raise RuntimeError("mapping failed")

    monkeypatch.setattr(
        telemetry_writer, "build_telemetry_security_event", fail_mapping
    )

    record_security_event_telemetry(_event())

    assert path.read_text(encoding="utf-8") == ""


def test_get_writer_returns_singleton(monkeypatch, tmp_path: Path) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    monkeypatch.setenv("AGENT_SEC_TELEMETRY_LOG_PATH", str(path))
    monkeypatch.setattr(telemetry_writer, "_writer", None)

    first = get_writer()
    second = get_writer()

    assert first is second
    assert first.path == path
