#!/usr/bin/env python3
"""Qoder PreToolUse hook for command code scanning."""

import json
import os
import subprocess
from typing import Any

from qoder_hook_common import (
    dumps_hook_output,
    jsonish_value,
    load_hook_input,
    pre_tool_decision_output,
    with_trace_context,
)

_MODE = os.environ.get("CODE_SCANNER_MODE", "observe").strip().lower()
_VALID_MODES = {"observe", "ask", "deny"}
try:
    _TIMEOUT = int(os.environ.get("CODE_SCANNER_TIMEOUT", "10"))
except (TypeError, ValueError):
    _TIMEOUT = 10

_DEFAULT_LANGUAGE = "bash"
_MAX_FINDINGS_DISPLAY = 5
_MAX_TEXT_CHARS = 120


def _safe_string(value: Any) -> str:
    """Return value when it is a string, otherwise an empty string."""
    return value if isinstance(value, str) else ""


def _as_list(value: Any) -> list[Any]:
    """Return value when it is a list, otherwise an empty list."""
    return value if isinstance(value, list) else []


def _shorten(value: str, limit: int = _MAX_TEXT_CHARS) -> str:
    """Return compact user-visible text."""
    normalized = " ".join(value.split())
    if len(normalized) <= limit:
        return normalized
    return normalized[: limit - 1] + "..."


def _hook_event(input_data: dict[str, Any]) -> str:
    """Return the Qoder hook event name."""
    return _safe_string(input_data.get("hook_event_name"))


def _command_from_input(input_data: dict[str, Any]) -> str | None:
    """Return the Bash command from a Qoder PreToolUse payload."""
    if _hook_event(input_data) != "PreToolUse":
        return None
    tool_input = jsonish_value(input_data.get("tool_input"))
    if not isinstance(tool_input, dict):
        return None
    command = tool_input.get("command")
    if not isinstance(command, str) or not command.strip():
        return None
    return command


def _scan_code(input_data: dict[str, Any], command: str) -> dict[str, Any] | None:
    """Run agent-sec-cli scan-code and parse its JSON response."""
    args = [
        "agent-sec-cli",
        "scan-code",
        "--code",
        command,
        "--language",
        _DEFAULT_LANGUAGE,
    ]
    try:
        proc = subprocess.run(
            with_trace_context(args, input_data),
            capture_output=True,
            check=False,
            text=True,
            timeout=_TIMEOUT,
        )
    except (OSError, subprocess.SubprocessError):
        return None

    if proc.returncode != 0:
        return None

    try:
        scan_result = json.loads(proc.stdout)
    except (ValueError, TypeError):
        return None
    return scan_result if isinstance(scan_result, dict) else None


def _finding_line(finding: dict[str, Any]) -> str:
    """Format a finding without echoing raw command evidence."""
    rule_id = _safe_string(finding.get("rule_id")) or "unknown"
    description = _safe_string(finding.get("desc_zh")) or _safe_string(
        finding.get("desc_en")
    )
    if description:
        return f"- {rule_id}: {_shorten(description)}"
    return f"- {rule_id}"


def _format_notice(findings: list[Any], final_message: str) -> str:
    """Build a Qoder-visible code scanner notice."""
    typed_findings = [item for item in findings if isinstance(item, dict)]
    lines = [
        f"[code-scanner] Detected {len(typed_findings)} risk finding(s) in this Bash command."
    ]
    for finding in typed_findings[:_MAX_FINDINGS_DISPLAY]:
        lines.append(_finding_line(finding))
    remaining = len(typed_findings) - _MAX_FINDINGS_DISPLAY
    if remaining > 0:
        lines.append(f"- ... and {remaining} more finding(s)")
    lines.append(final_message)
    return "\n".join(lines)


def _warn_output(notice: str) -> str:
    """Return a non-blocking output with a user-visible system message."""
    return dumps_hook_output({"systemMessage": notice})


def _invalid_mode_output() -> str:
    """Return a visible fail-open warning for an invalid scanner mode."""
    configured_mode = _shorten(_MODE, 32) or "<empty>"
    notice = (
        f"[code-scanner] Invalid CODE_SCANNER_MODE {configured_mode!r}; expected "
        "'observe', 'ask', or 'deny'. Falling back to observe mode; execution will continue."
    )
    return _warn_output(notice)


def _format_decision(verdict: str, findings: list[Any]) -> str | None:
    """Map a scan-code verdict to Qoder PreToolUse output."""
    if verdict in {"pass", "error"} or not findings:
        return None
    if verdict not in {"warn", "deny"}:
        return None
    if _MODE == "ask":
        return pre_tool_decision_output(
            "ask",
            _format_notice(findings, "Review this command before execution."),
        )
    if _MODE == "deny":
        return pre_tool_decision_output(
            "deny",
            _format_notice(findings, "This command was denied before execution."),
        )
    return None


def main() -> None:
    """Run the Qoder code scanner hook."""
    input_data = load_hook_input()
    if input_data is None:
        return

    command = _command_from_input(input_data)
    if command is None:
        return

    if _MODE not in _VALID_MODES:
        print(_invalid_mode_output())
        return

    scan_result = _scan_code(input_data, command)
    if scan_result is None:
        return

    verdict = _safe_string(scan_result.get("verdict")) or "pass"
    findings = _as_list(scan_result.get("findings"))
    output = _format_decision(verdict, findings)
    if output:
        print(output)


if __name__ == "__main__":
    main()
