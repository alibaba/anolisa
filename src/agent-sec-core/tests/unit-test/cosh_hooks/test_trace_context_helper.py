"""Unit tests for cosh-extension/hooks/trace_context.py."""

import json
import sys
from pathlib import Path

_HOOKS_DIR = Path(__file__).resolve().parents[2] / ".." / "cosh-extension" / "hooks"
sys.path.insert(0, str(_HOOKS_DIR))

from trace_context import trace_context, with_trace_context  # noqa: E402


def test_trace_context_uses_fixed_cosh_hook_input_fields():
    assert trace_context(
        {
            "trace_id": "trace-1",
            "session_id": "session-1",
            "run_id": "run-1",
            "call_id": "call-1",
            "tool_use_id": "tool-1",
        }
    ) == {
        "trace_id": "trace-1",
        "session_id": "session-1",
        "run_id": "run-1",
        "call_id": "call-1",
        "tool_call_id": "tool-1",
    }


def test_trace_context_ignores_camel_case_and_empty_values():
    assert (
        trace_context(
            {
                "trace_id": "",
                "traceId": "trace-1",
                "sessionId": "session-1",
                "runId": "run-1",
                "callId": "call-1",
                "toolUseId": "tool-1",
            }
        )
        is None
    )


def test_with_trace_context_serializes_fixed_fields():
    args = with_trace_context(
        ["agent-sec-cli", "scan-code"],
        {"session_id": "session-1", "run_id": "run-1"},
    )

    assert args == [
        "agent-sec-cli",
        "--trace-context",
        json.dumps(
            {"session_id": "session-1", "run_id": "run-1"},
            ensure_ascii=False,
            separators=(",", ":"),
        ),
        "scan-code",
    ]
