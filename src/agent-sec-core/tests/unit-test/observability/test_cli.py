"""Unit tests for the observability record CLI."""

import json
from pathlib import Path
from typing import Any

import agent_sec_cli.observability as observability
import agent_sec_cli.observability.cli as observability_cli
import agent_sec_cli.observability.review as observability_review
import agent_sec_cli.observability.sqlite_reader as observability_sqlite_reader
import pytest
import typer
from agent_sec_cli.cli import app
from agent_sec_cli.observability.metrics import HOOK_METRIC_ALLOWLIST
from typer.testing import CliRunner


@pytest.fixture(autouse=True)
def reset_observability_writer(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(observability, "_writer", None, raising=False)
    monkeypatch.setattr(observability, "_sqlite_writer", None, raising=False)


def _payload(**overrides: Any) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "hook": "before_agent_run",
        "observedAt": "2026-05-11T12:00:00Z",
        "metadata": {
            "sessionId": "session-123",
            "runId": "run-123",
        },
        "metrics": {
            "prompt": "Summarize ./README.md",
            "prompt_length_chars": 21,
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


def test_record_json_stdin_writes_observability_stores_only(tmp_path: Path) -> None:
    runner = CliRunner()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(_payload()),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 0, result.output
    assert result.output == ""
    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert len(records) == 1
    assert "schemaVersion" not in records[0]
    assert records[0]["hook"] == "before_agent_run"
    assert records[0]["metadata"]["sessionId"] == "session-123"
    assert (tmp_path / "observability.db").exists()
    assert not (tmp_path / "security-events.jsonl").exists()
    assert not (tmp_path / "security-events.db").exists()


def test_record_accepts_before_llm_call_without_call_id(tmp_path: Path) -> None:
    runner = CliRunner()
    payload = _payload(
        hook="before_llm_call",
        metadata={
            "sessionId": "session-123",
            "runId": "run-123",
        },
        metrics={
            "prompt": "assembled prompt",
        },
    )

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(payload),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 0, result.output
    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert records[0]["hook"] == "before_llm_call"
    assert records[0]["metadata"] == {
        "sessionId": "session-123",
        "runId": "run-123",
    }
    assert records[0]["metrics"] == {"prompt": "assembled prompt"}


def test_record_accepts_after_agent_run_llm_output_response(tmp_path: Path) -> None:
    runner = CliRunner()
    payload = _payload(
        hook="after_agent_run",
        metadata={
            "sessionId": "session-123",
            "runId": "run-123",
        },
        metrics={
            "response": "Done.",
        },
    )

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(payload),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 0, result.output
    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert records[0]["hook"] == "after_agent_run"
    assert records[0]["metadata"] == {
        "sessionId": "session-123",
        "runId": "run-123",
    }
    assert records[0]["metrics"] == {"response": "Done."}


def test_record_accepts_after_agent_run_llm_output_tool_use_summary(
    tmp_path: Path,
) -> None:
    runner = CliRunner()
    metrics = {
        "output_kind": "tool_use",
        "stop_reason": "toolUse",
        "assistant_texts_count": 0,
        "tool_calls_count": 1,
        "tool_calls": [
            {
                "toolName": "exec",
                "parameters": {
                    "command": 'find /home/xingdong -name "testfolder2" -maxdepth 3 2>/dev/null'
                },
            }
        ],
    }
    payload = _payload(
        hook="after_agent_run",
        metadata={
            "sessionId": "session-123",
            "runId": "run-123",
        },
        metrics=metrics,
    )

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(payload),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 0, result.output
    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert records[0]["hook"] == "after_agent_run"
    assert records[0]["metadata"] == {
        "sessionId": "session-123",
        "runId": "run-123",
    }
    assert records[0]["metrics"] == metrics


def test_record_drops_unknown_fields_and_metrics(tmp_path: Path) -> None:
    runner = CliRunner()
    payload = _payload(
        producerVersion="2.0.0",
        metadata={
            "sessionId": "session-123",
            "runId": "run-123",
            "futureCorrelationId": "future-123",
        },
        metrics={
            "prompt": "Summarize ./README.md",
            "future_metric": 42,
        },
    )

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(payload),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 0, result.output
    records = _jsonl_records(tmp_path / "observability.jsonl")
    assert "producerVersion" not in records[0]
    assert "futureCorrelationId" not in records[0]["metadata"]
    assert records[0]["metrics"] == {"prompt": "Summarize ./README.md"}


def test_record_rejects_empty_metrics_after_filtering(tmp_path: Path) -> None:
    runner = CliRunner()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(_payload(metrics={"future_metric": 42})),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 1
    assert "at least one allowed metric" in result.output
    assert not (tmp_path / "observability.jsonl").exists()


def test_record_returns_nonzero_when_jsonl_append_fails(tmp_path: Path) -> None:
    runner = CliRunner()
    data_dir = tmp_path / "agent-sec-data"
    data_dir.mkdir()
    (data_dir / "observability.jsonl").mkdir()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps(
            _payload(
                hook="after_agent_run",
                metrics={"success": True},
            )
        ),
        env={"AGENT_SEC_DATA_DIR": str(data_dir)},
    )

    assert result.exit_code == 1
    assert "Error: failed to write observability record:" in result.output


def test_observability_schema_outputs_wire_schema() -> None:
    runner = CliRunner()
    call_id_hooks = {
        "before_llm_call",
        "after_llm_call",
        "before_tool_call",
        "after_tool_call",
    }
    tool_call_hooks = {"before_tool_call", "after_tool_call"}

    result = runner.invoke(app, ["observability", "schema"])

    assert result.exit_code == 0, result.output
    schema = json.loads(result.output)
    assert schema["$schema"] == "https://json-schema.org/draft/2020-12/schema"
    assert "allOf" not in schema
    assert schema["discriminator"]["propertyName"] == "hook"
    assert set(schema["discriminator"]["mapping"]) == set(HOOK_METRIC_ALLOWLIST)

    record_def_by_hook = {
        hook: schema["$defs"][ref.removeprefix("#/$defs/")]
        for hook, ref in schema["discriminator"]["mapping"].items()
    }
    assert len(schema["oneOf"]) == len(HOOK_METRIC_ALLOWLIST)

    for hook, metrics in HOOK_METRIC_ALLOWLIST.items():
        record_schema = record_def_by_hook[hook]
        assert "schemaVersion" not in record_schema["properties"]
        assert record_schema["properties"]["hook"]["const"] == hook
        assert "observedAt" in record_schema["properties"]
        metadata_ref = record_schema["properties"]["metadata"]["$ref"]
        metadata_schema = schema["$defs"][metadata_ref.removeprefix("#/$defs/")]
        assert {"sessionId", "runId"}.issubset(metadata_schema["required"])
        if hook in call_id_hooks:
            assert "callId" in metadata_schema["properties"]
            assert "callId" not in metadata_schema["required"]
        if hook in tool_call_hooks:
            assert "toolCallId" in metadata_schema["required"]
        metric_ref = record_schema["properties"]["metrics"]["$ref"]
        metric_schema = schema["$defs"][metric_ref.removeprefix("#/$defs/")]
        assert metric_schema["type"] == "object"
        assert metric_schema["minProperties"] == 1
        assert set(metric_schema["properties"]) == set(metrics)
        assert metric_schema.get("additionalProperties", True) is True


def test_record_rejects_jsonl_format(tmp_path: Path) -> None:
    runner = CliRunner()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "jsonl", "--stdin"],
        input=f"{json.dumps(_payload())}\n",
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 1
    assert "Error: --format must be json." in result.output
    assert not (tmp_path / "observability.jsonl").exists()


def test_record_rejects_json_array_without_partial_write(tmp_path: Path) -> None:
    runner = CliRunner()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json", "--stdin"],
        input=json.dumps([_payload()]),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 1
    assert "Error: payload must be a JSON object" in result.output
    assert not (tmp_path / "observability.jsonl").exists()


def test_record_requires_stdin_flag(tmp_path: Path) -> None:
    runner = CliRunner()

    result = runner.invoke(
        app,
        ["observability", "record", "--format", "json"],
        input=json.dumps(_payload()),
        env={"AGENT_SEC_DATA_DIR": str(tmp_path)},
    )

    assert result.exit_code == 1
    assert "Error:" in result.output


def test_review_rejects_non_interactive_stdio(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(observability_cli.sys.stdin, "isatty", lambda: False)
    monkeypatch.setattr(observability_cli.sys.stdout, "isatty", lambda: True)

    with pytest.raises(typer.Exit) as exc_info:
        observability_cli.review()

    assert exc_info.value.exit_code == 2
    assert "requires an interactive terminal" in capsys.readouterr().err


def test_review_closes_reader_after_tui_exits(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []

    class FakeReader:
        def __init__(self) -> None:
            events.append("reader-init")

        def close(self) -> None:
            events.append("reader-close")

    class FakeReviewApp:
        def __init__(self, reader: FakeReader) -> None:
            events.append(f"app-init:{reader.__class__.__name__}")

        def run(self) -> None:
            events.append("app-run")

    monkeypatch.setattr(observability_cli.sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(observability_cli.sys.stdout, "isatty", lambda: True)
    monkeypatch.setattr(observability_sqlite_reader, "ObservabilityReader", FakeReader)
    monkeypatch.setattr(observability_review, "ObservabilityReviewApp", FakeReviewApp)

    observability_cli.review()

    assert events == [
        "reader-init",
        "app-init:FakeReader",
        "app-run",
        "reader-close",
    ]


def test_review_closes_reader_when_tui_raises(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []

    class FakeReader:
        def close(self) -> None:
            events.append("reader-close")

    class FailingReviewApp:
        def __init__(self, reader: FakeReader) -> None:
            self._reader = reader

        def run(self) -> None:
            events.append("app-run")
            raise RuntimeError("tui failed")

    monkeypatch.setattr(observability_cli.sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(observability_cli.sys.stdout, "isatty", lambda: True)
    monkeypatch.setattr(observability_sqlite_reader, "ObservabilityReader", FakeReader)
    monkeypatch.setattr(
        observability_review, "ObservabilityReviewApp", FailingReviewApp
    )

    with pytest.raises(RuntimeError, match="tui failed"):
        observability_cli.review()

    assert events == ["app-run", "reader-close"]
