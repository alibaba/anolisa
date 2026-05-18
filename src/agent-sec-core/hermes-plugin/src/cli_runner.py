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
) -> CliResult:
    """Call agent-sec-cli as a subprocess.

    - Never raises exceptions (fail-open principle)
    - On timeout → CliResult("", "timed out", 124)
    - On other errors → CliResult("", str(e), 1)
    """
    try:
        proc = subprocess.run(
            ["agent-sec-cli", *args],
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
