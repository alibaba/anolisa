"""Unit tests for Hermes observability CLI helper."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from unittest.mock import patch

_HERMES_PLUGIN_DIR = Path(__file__).resolve().parents[3] / "hermes-plugin"
sys.path.insert(0, str(_HERMES_PLUGIN_DIR))

from src.cli_runner import CliResult, record_hermes_observability  # noqa: E402


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
