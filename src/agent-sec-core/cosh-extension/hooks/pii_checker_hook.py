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

from trace_context import with_trace_context

_DEFAULT_SOURCE = "user_input"
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

    prompt_text = input_data.get("prompt", "")
    if not isinstance(prompt_text, str) or not prompt_text.strip():
        print(_allow())
        return

    try:
        cmd = with_trace_context(
            [
                "agent-sec-cli",
                "scan-pii",
                "--stdin",
                "--format",
                "json",
                "--source",
                _DEFAULT_SOURCE,
            ],
            input_data,
        )
        proc = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
            input=prompt_text,
            text=True,
            timeout=10,
        )
    except Exception:
        print(_allow())
        return

    if proc.returncode != 0:
        print(_allow())
        return

    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        print(_allow())
        return

    print(_format_cosh(scan_result))


if __name__ == "__main__":
    main()
