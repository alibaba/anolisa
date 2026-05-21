"""Unit tests for Hermes observability CLI helper."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

_HERMES_PLUGIN_DIR = Path(__file__).resolve().parents[3] / "hermes-plugin"
sys.path.insert(0, str(_HERMES_PLUGIN_DIR))

from src.cli_runner import (  # noqa: E402
    CliResult,
    call_agent_sec_cli,
    record_hermes_observability,
    trace_context,
)


def _record() -> dict:
    return {
        "hook": "before_agent_run",
        "observedAt": "2026-05-18T00:00:00Z",
        "metadata": {
            "sessionId": "session-1",
            "runId": "00000000-0000-0000-0000-000000000000",
        },
        "metrics": {"user_input": "hello"},
    }


@patch("src.cli_runner.call_agent_sec_cli")
def test_record_hermes_observability_uses_openclaw_cli_shape(mock_cli):
    mock_cli.return_value = CliResult(stdout="", stderr="", exit_code=0)

    result = record_hermes_observability(_record(), timeout=5.0)

    assert result.exit_code == 0
    mock_cli.assert_called_once()
    args, kwargs = mock_cli.call_args
    assert args[0] == ["observability", "record", "--format", "json", "--stdin"]
    assert kwargs["timeout"] == 5.0
    payload = json.loads(kwargs["stdin"])
    assert payload["hook"] == "before_agent_run"
    assert payload["metadata"]["sessionId"] == "session-1"


@patch("src.cli_runner.subprocess.run")
def test_call_agent_sec_cli_prepends_trace_context(mock_run):
    mock_run.return_value = subprocess.CompletedProcess(
        args=[],
        returncode=0,
        stdout="{}",
        stderr="",
    )

    result = call_agent_sec_cli(
        ["scan-code", "--code", "pwd"],
        timeout=5.0,
        trace_context={"session_id": "session-1", "tool_call_id": "tool-1"},
    )

    assert result.exit_code == 0
    argv = mock_run.call_args.args[0]
    assert argv[:4] == [
        "agent-sec-cli",
        "--trace-context",
        '{"session_id":"session-1","tool_call_id":"tool-1"}',
        "scan-code",
    ]


def test_trace_context_normalizes_camelcase_and_strips_whitespace():
    assert trace_context(
        {
            "traceId": " t1 ",
            "sessionId": "s1",
            "runId": "",
            "callId": "  ",
            "toolUseId": "u1",
        }
    ) == {"trace_id": "t1", "session_id": "s1", "tool_call_id": "u1"}


def test_trace_context_uses_first_non_empty_alias():
    assert trace_context(
        {
            "call_id": "",
            "callId": " c1 ",
            "tool_call_id": "",
            "toolCallId": "  ",
            "tool_use_id": " u1 ",
            "toolUseId": "u2",
        }
    ) == {"call_id": "c1", "tool_call_id": "u1"}


def test_trace_context_returns_none_when_all_fields_missing():
    assert trace_context({"foo": "bar"}) is None
