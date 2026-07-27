#!/usr/bin/env python3
"""Cosh hook script for PIIChecker.

Reads a cosh UserPromptSubmit JSON from stdin, extracts the user prompt,
invokes ``agent-sec-cli scan-pii`` via subprocess, and writes a cosh
HookOutput JSON to stdout.

This script is intentionally self-contained — it does NOT import any
``agent_sec_cli`` package. All it needs is the standard library and the
``agent-sec-cli`` binary on $PATH.
"""

import json
import subprocess
import sys
from typing import Any

from pii_text import value_to_text
from trace_context import with_trace_context

_USER_INPUT_SOURCE = "user_input"
_TOOL_INPUT_SOURCE = "tool_input"
_TOOL_OUTPUT_SOURCE = "tool_output"
_MODEL_OUTPUT_SOURCE = "model_output"
_MAX_EVIDENCE_ITEMS = 3
_MAX_EVIDENCE_CHARS = 80


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _as_list(value: Any) -> list[Any]:
    return value if isinstance(value, list) else []


def _safe_text(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _shorten(value: str, limit: int = _MAX_EVIDENCE_CHARS) -> str:
    value = " ".join(value.split())
    if len(value) <= limit:
        return value
    return value[: limit - 1] + "…"


def _format_pii_warning(verdict: str, findings: list[Any]) -> str:
    """Build a minimal-disclosure warning from structured PII findings."""
    typed_findings = [item for item in findings if isinstance(item, dict)]
    count = len(typed_findings)
    pii_types = sorted(
        {
            finding_type
            for finding in typed_findings
            if (finding_type := _safe_text(finding.get("type")))
        }
    )
    severities = sorted(
        {
            severity
            for finding in typed_findings
            if (severity := _safe_text(finding.get("severity")))
        }
    )
    redacted_evidence: list[str] = []
    for finding in typed_findings:
        evidence = _safe_text(finding.get("evidence_redacted"))
        if evidence and evidence not in redacted_evidence:
            redacted_evidence.append(_shorten(evidence))
        if len(redacted_evidence) >= _MAX_EVIDENCE_ITEMS:
            break

    risk = "高风险敏感信息" if verdict == "deny" else "敏感信息"
    parts = [
        f"[pii-checker] 检测到 {count} 项{risk}",
        f"类型：{', '.join(pii_types) if pii_types else 'unknown'}",
    ]
    if severities:
        parts.append(f"严重级别：{', '.join(severities)}")
    if redacted_evidence:
        parts.append(f"脱敏示例：{', '.join(redacted_evidence)}")
    parts.append("本轮请求将继续处理。")
    return "；".join(parts)


def _scan_text(
    input_data: dict[str, Any], text: str, source: str
) -> dict[str, Any] | None:
    """Run scan-pii with a source label and parse JSON output."""
    try:
        cmd = with_trace_context(
            [
                "agent-sec-cli",
                "scan-pii",
                "--stdin",
                "--format",
                "json",
                "--redact-output",
                "--source",
                source,
            ],
            input_data,
        )
        proc = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
            input=text,
            text=True,
            timeout=10,
        )
    except Exception:
        return None

    if proc.returncode != 0:
        return None

    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        return None
    return scan_result if isinstance(scan_result, dict) else None


def _extract_response_text(llm_response: Any) -> str:
    """Extract text from common Cosh AfterModel response shapes."""
    if isinstance(llm_response, str):
        return llm_response
    if not isinstance(llm_response, dict):
        return ""

    text = llm_response.get("text")
    if isinstance(text, str):
        return text

    candidates = llm_response.get("candidates")
    if not isinstance(candidates, list):
        return ""

    parts: list[str] = []
    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        content = candidate.get("content")
        if not isinstance(content, dict):
            continue
        candidate_parts = content.get("parts")
        if isinstance(candidate_parts, str):
            parts.append(candidate_parts)
        elif isinstance(candidate_parts, list):
            for part in candidate_parts:
                if isinstance(part, str):
                    parts.append(part)
                elif isinstance(part, dict) and isinstance(part.get("text"), str):
                    parts.append(part["text"])
    return "".join(parts)


def _extract_scan_target(input_data: dict[str, Any]) -> tuple[str, str]:
    """Return text and source for supported Cosh hook events."""
    event_name = _safe_text(input_data.get("hook_event_name"))
    if not event_name:
        event_name = _safe_text(input_data.get("hookEventName"))

    if event_name in {"", "UserPromptSubmit"}:
        return _safe_text(input_data.get("prompt")), _USER_INPUT_SOURCE

    if event_name == "PreToolUse":
        if "tool_input" not in input_data:
            return "", _TOOL_INPUT_SOURCE
        return value_to_text(input_data.get("tool_input")), _TOOL_INPUT_SOURCE

    if event_name == "PostToolUse":
        if "tool_response" not in input_data:
            return "", _TOOL_OUTPUT_SOURCE
        return value_to_text(input_data.get("tool_response")), _TOOL_OUTPUT_SOURCE

    if event_name == "PostToolUseFailure":
        return _safe_text(input_data.get("error")), _TOOL_OUTPUT_SOURCE

    if event_name == "AfterModel":
        return (
            _extract_response_text(input_data.get("llm_response")),
            _MODEL_OUTPUT_SOURCE,
        )

    return "", "unknown"


def _format_cosh(scan_result: dict[str, Any]) -> str:
    """Convert a scan-pii result dict into a cosh HookOutput JSON string.

    Mapping:
        verdict == "pass" -> decision "allow"
        verdict == "warn" -> decision "allow" with reason
        verdict == "deny" -> decision "allow" with high-risk reason
        verdict == "error" or unknown -> fail-open "allow"
    """
    verdict = _safe_text(scan_result.get("verdict")) or "pass"
    findings = _as_list(scan_result.get("findings"))

    if verdict == "pass" or not findings:
        return _allow()

    if verdict in {"warn", "deny"}:
        return json.dumps(
            {"decision": "allow", "reason": _format_pii_warning(verdict, findings)},
            ensure_ascii=False,
        )

    return _allow()


def main() -> None:
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        print(_allow())
        return

    if not isinstance(input_data, dict):
        print(_allow())
        return

    scan_text, source = _extract_scan_target(input_data)
    if not isinstance(scan_text, str) or not scan_text.strip():
        print(_allow())
        return

    scan_result = _scan_text(input_data, scan_text, source)
    if scan_result is None:
        print(_allow())
        return
    print(_format_cosh(scan_result))


if __name__ == "__main__":
    main()
