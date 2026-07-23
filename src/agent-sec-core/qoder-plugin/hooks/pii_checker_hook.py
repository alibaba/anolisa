#!/usr/bin/env python3
"""Qoder hook for PII and credential detection."""

import json
import os
import subprocess
from typing import Any

from qoder_hook_common import (
    deny_output,
    dumps_hook_output,
    jsonish_value,
    load_hook_input,
    post_tool_output_replacement,
    pre_tool_decision_output,
    value_to_text,
    with_trace_context,
)

_MODE = os.environ.get("PII_CHECKER_MODE", "observe").strip().lower()
_VALID_MODES = {"observe", "deny"}
try:
    _TIMEOUT = int(os.environ.get("PII_CHECKER_TIMEOUT", "5"))
except (TypeError, ValueError):
    _TIMEOUT = 5

_INCLUDE_LOW_CONFIDENCE = os.environ.get(
    "PII_CHECKER_INCLUDE_LOW_CONFIDENCE", ""
).strip().lower() in {"1", "true", "yes", "on"}
_MAX_EVIDENCE_ITEMS = 3
_MAX_EVIDENCE_CHARS = 80

_USER_INPUT_SOURCE = "user_input"
_TOOL_INPUT_SOURCE = "tool_input"
_TOOL_OUTPUT_SOURCE = "tool_output"


def _safe_string(value: Any) -> str:
    """Return value when it is a string, otherwise an empty string."""
    return value if isinstance(value, str) else ""


def _as_list(value: Any) -> list[Any]:
    """Return value when it is a list, otherwise an empty list."""
    return value if isinstance(value, list) else []


def _shorten(value: str, limit: int = _MAX_EVIDENCE_CHARS) -> str:
    """Return compact evidence text for user-visible notices."""
    normalized = " ".join(value.split())
    if len(normalized) <= limit:
        return normalized
    return normalized[: limit - 1] + "..."


def _hook_event(input_data: dict[str, Any]) -> str:
    """Return the Qoder hook event name."""
    return _safe_string(input_data.get("hook_event_name"))


def _scan_target(input_data: dict[str, Any]) -> tuple[str, str, str] | None:
    """Return text, source label, and display source for supported Qoder hooks."""
    event_name = _hook_event(input_data)
    if event_name == "UserPromptSubmit":
        text = _safe_string(input_data.get("prompt"))
        return (text, _USER_INPUT_SOURCE, "user input") if text.strip() else None

    if event_name == "PreToolUse":
        if "tool_input" not in input_data:
            return None
        text = value_to_text(jsonish_value(input_data.get("tool_input")))
        return (text, _TOOL_INPUT_SOURCE, "tool input") if text.strip() else None

    if event_name == "PostToolUse":
        if "tool_response" not in input_data:
            return None
        text = value_to_text(jsonish_value(input_data.get("tool_response")))
        return (text, _TOOL_OUTPUT_SOURCE, "tool output") if text.strip() else None

    return None


def _scan_pii(
    input_data: dict[str, Any],
    text: str,
    source: str,
) -> dict[str, Any] | None:
    """Run agent-sec-cli scan-pii and parse its JSON response."""
    args = [
        "agent-sec-cli",
        "scan-pii",
        "--stdin",
        "--format",
        "json",
        "--redact-output",
        "--source",
        source,
    ]
    if _INCLUDE_LOW_CONFIDENCE:
        args.append("--include-low-confidence")

    try:
        proc = subprocess.run(
            with_trace_context(args, input_data),
            capture_output=True,
            check=False,
            input=text,
            text=True,
            timeout=_TIMEOUT,
        )
    except (OSError, subprocess.SubprocessError, TimeoutError):
        return None

    if proc.returncode != 0:
        return None

    try:
        scan_result = json.loads(proc.stdout)
    except (ValueError, TypeError):
        return None
    return scan_result if isinstance(scan_result, dict) else None


def _format_notice(
    verdict: str,
    findings: list[Any],
    source_desc: str,
    final_message: str,
) -> str:
    """Build a minimal-disclosure PII notice from sanitized findings."""
    typed_findings = [item for item in findings if isinstance(item, dict)]
    pii_types = sorted(
        {
            finding_type
            for finding in typed_findings
            if (finding_type := _safe_string(finding.get("type")))
        }
    )
    severities = sorted(
        {
            severity
            for finding in typed_findings
            if (severity := _safe_string(finding.get("severity")))
        }
    )
    evidence: list[str] = []
    for finding in typed_findings:
        redacted = _safe_string(finding.get("evidence_redacted"))
        if redacted and redacted not in evidence:
            evidence.append(_shorten(redacted))
        if len(evidence) >= _MAX_EVIDENCE_ITEMS:
            break

    risk = "high-risk sensitive data" if verdict == "deny" else "sensitive data"
    parts = [
        f"[pii-checker] Detected {len(typed_findings)} {risk} finding(s) in {source_desc}",
        f"types: {', '.join(pii_types) if pii_types else 'unknown'}",
    ]
    if severities:
        parts.append(f"severity: {', '.join(severities)}")
    if evidence:
        parts.append(f"redacted evidence: {', '.join(evidence)}")
    parts.append(final_message)
    return "; ".join(parts)


def _warn_output(notice: str) -> str:
    """Return an allow decision with a user-visible system message."""
    return dumps_hook_output({"decision": "allow", "systemMessage": notice})


def _invalid_mode_output() -> str:
    """Return a visible fail-open warning for an invalid checker mode."""
    configured_mode = _shorten(_MODE, 32) or "<empty>"
    notice = (
        f"[pii-checker] Invalid PII_CHECKER_MODE {configured_mode!r}; expected "
        "'observe' or 'deny'. Falling back to observe mode; execution will continue."
    )
    return _warn_output(notice)


def _format_decision(
    input_data: dict[str, Any],
    verdict: str,
    findings: list[Any],
    source_desc: str,
) -> str | None:
    """Map a scan-pii verdict to Qoder hook output."""
    if verdict == "pass" or not findings:
        return None
    if verdict not in {"warn", "deny"}:
        return None
    if _MODE != "deny":
        return None

    event_name = _hook_event(input_data)
    if verdict == "warn":
        notice = _format_notice(
            verdict,
            findings,
            source_desc,
            "Execution will continue.",
        )
        return _warn_output(notice)

    if event_name == "UserPromptSubmit":
        notice = _format_notice(
            verdict,
            findings,
            source_desc,
            "Remove the sensitive data and submit again.",
        )
        return deny_output(notice)

    if event_name == "PreToolUse":
        notice = _format_notice(
            verdict,
            findings,
            source_desc,
            "This tool call was blocked.",
        )
        return pre_tool_decision_output("deny", notice)

    if event_name == "PostToolUse":
        notice = _format_notice(
            verdict,
            findings,
            source_desc,
            "The raw tool output was replaced before entering model context.",
        )
        return post_tool_output_replacement(notice)

    return None


def main() -> None:
    """Run the Qoder PII hook."""
    input_data = load_hook_input()
    if input_data is None:
        return

    target = _scan_target(input_data)
    if target is None:
        return
    text, source, source_desc = target

    scan_result = _scan_pii(input_data, text, source)
    if _MODE not in _VALID_MODES:
        print(_invalid_mode_output())
        return
    if scan_result is None:
        return

    verdict = _safe_string(scan_result.get("verdict")) or "pass"
    findings = _as_list(scan_result.get("findings"))
    output = _format_decision(input_data, verdict, findings, source_desc)
    if output:
        print(output)


if __name__ == "__main__":
    main()
