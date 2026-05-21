"""Subprocess wrapper for calling agent-sec-cli — fail-open, never raises."""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from typing import Any


@dataclass
class CliResult:
    """Result of an agent-sec-cli subprocess invocation."""

    stdout: str
    stderr: str
    exit_code: int


def call_agent_sec_cli(
    args: list[str],
    timeout: float = 10.0,
    stdin: str | None = None,
    trace_context: dict[str, str] | None = None,
) -> CliResult:
    """Call agent-sec-cli as a subprocess.

    - Never raises exceptions (fail-open principle)
    - On timeout → CliResult("", "timed out", 124)
    - On other errors → CliResult("", str(e), 1)
    """
    final_args = _with_trace_context(args, trace_context)
    try:
        proc = subprocess.run(
            ["agent-sec-cli", *final_args],
            input=stdin,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return CliResult(
            stdout=proc.stdout,
            stderr=proc.stderr,
            exit_code=proc.returncode,
        )
    except subprocess.TimeoutExpired:
        return CliResult(stdout="", stderr="timed out", exit_code=124)
    except Exception as e:
        return CliResult(stdout="", stderr=str(e), exit_code=1)


_TRACE_FIELD_SPECS = (
    ("trace_id", ("trace_id", "traceId")),
    ("session_id", ("session_id", "sessionId")),
    ("run_id", ("run_id", "runId")),
    ("call_id", ("call_id", "callId")),
    ("tool_call_id", ("tool_call_id", "toolCallId", "tool_use_id", "toolUseId")),
)


def trace_context(data: dict[str, Any]) -> dict[str, str] | None:
    """Build agent-sec-cli trace context from Hermes hook kwargs."""
    context: dict[str, str] = {}
    for output_key, input_keys in _TRACE_FIELD_SPECS:
        for input_key in input_keys:
            value = data.get(input_key)
            if isinstance(value, str) and value.strip():
                context[output_key] = value.strip()
                break
    return context or None


def _with_trace_context(
    args: list[str],
    context: dict[str, str] | None,
) -> list[str]:
    if not context:
        return args
    return [
        "--trace-context",
        json.dumps(context, ensure_ascii=False, separators=(",", ":")),
        *args,
    ]


def record_hermes_observability(
    record: dict[str, Any],
    timeout: float = 10.0,
) -> CliResult:
    """Emit one Hermes observability record via agent-sec-cli stdin."""
    return call_agent_sec_cli(
        ["observability", "record", "--format", "json", "--stdin"],
        timeout=timeout,
        stdin=json.dumps(record, ensure_ascii=False, separators=(",", ":")),
    )
