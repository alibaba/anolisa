#!/usr/bin/env python3
"""Codex hook for PII (Personal Identifiable Information) detection.

Supports THREE hook points via a single script (routed by hook_event_name):
  - UserPromptSubmit: scans user prompt before it reaches the model.
  - PreToolUse: scans tool input before the tool executes.
  - PostToolUse: scans tool output before it enters model context.

Protection direction:
  - UserPromptSubmit / PostToolUse: detect PII flowing INTO the LLM provider
    (user prompt / tool output → model) before applying the configured policy.
  - PreToolUse: detect PII flowing OUT via a tool call (exfiltration), e.g.
    curl-ing a phone number to an external endpoint or writing PII to a file.
    This is the only point to enforce PII policy before the tool executes.

Modes (controlled by PII_CHECKER_MODE env var, default: observe):
  - observe: silent pass-through, only audit trail via agent-sec-cli events.
             Even if PII is detected, content will NOT be blocked.
  - deny: surface scanner "warn" verdicts through systemMessage and continue;
          block scanner "deny" verdicts at all three hook points.

Protocol note: Codex supports non-blocking systemMessage warnings but does not
support "redact and pass" for these hook points. A warning therefore forwards
the original payload unchanged, while a deny verdict blocks the payload.

Usage::

    python3 pii_checker_hook.py          # reads stdin, writes stdout

This script is intentionally self-contained — it does NOT import any
``agent_sec_cli`` package. All it needs is the standard library and the
``agent-sec-cli`` binary on $PATH.
"""

import json
import os
import subprocess
import sys
from typing import Any

from trace_context import with_trace_context

# -- config ----------------------------------------------------------------

MODE = os.environ.get("PII_CHECKER_MODE", "observe").lower()
try:
    TIMEOUT = int(os.environ.get("PII_CHECKER_TIMEOUT", "5"))
except (ValueError, TypeError):
    TIMEOUT = 5

_MAX_EVIDENCE_ITEMS = 3
_MAX_EVIDENCE_CHARS = 80


# -- helpers ---------------------------------------------------------------


def _as_list(value: Any) -> list[Any]:
    return value if isinstance(value, list) else []


def _safe_text(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _shorten(value: str, limit: int = _MAX_EVIDENCE_CHARS) -> str:
    value = " ".join(value.split())
    if len(value) <= limit:
        return value
    return value[: limit - 1] + "…"


# -- output helpers --------------------------------------------------------


def _format_finding_details(findings: list[dict]) -> tuple[int, list[str]]:
    """Build shared, audit-safe detail lines from structured PII findings."""
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

    lines = [f"  类型      : {', '.join(pii_types) if pii_types else 'unknown'}"]
    if severities:
        lines.append(f"  严重级别  : {', '.join(severities)}")
    if redacted_evidence:
        lines.append(f"  脱敏示例  : {', '.join(redacted_evidence)}")
    return count, lines


def _format_block_reason(
    findings: list[dict], hook_event: str, source_desc: str
) -> str:
    """Build a human-readable block reason from structured PII findings.

    The reason is shown to the user (UserPromptSubmit) or replaces tool
    output visible to the model (PostToolUse). It contains only PII types
    and redacted evidence — never the raw PII content itself.
    """
    count, details = _format_finding_details(findings)
    lines = [
        f"[pii-checker] 🔒 安全拦截：{source_desc}中检测到 {count} 项个人敏感信息",
        *details,
    ]
    lines.append(f"  拦截环节  : {hook_event}")

    if hook_event == "UserPromptSubmit":
        lines.append("请移除敏感信息后重新提交。")
    elif hook_event == "PreToolUse":
        lines.append("该工具调用已被阻止，敏感信息不会外发。")
    else:
        lines.append("工具输出已被拦截，原始内容不会进入模型上下文。")

    return "\n".join(lines)


def _format_warning_message(
    findings: list[dict], hook_event: str, source_desc: str
) -> str:
    """Build a non-blocking warning using only redacted finding evidence."""
    count, details = _format_finding_details(findings)
    lines = [
        f"[pii-checker] ⚠️ 隐私告警：{source_desc}中检测到 {count} 项个人敏感信息",
        *details,
        f"  告警环节  : {hook_event}",
        "检测结果为 warn，执行将继续。",
    ]
    return "\n".join(lines)


def _block(findings: list[dict], hook_event: str, source_desc: str) -> None:
    """Output block decision JSON to stdout."""
    reason = _format_block_reason(findings, hook_event, source_desc)
    print(json.dumps({"decision": "block", "reason": reason}, ensure_ascii=False))


def _warn(findings: list[dict], hook_event: str, source_desc: str) -> None:
    """Output a user-visible warning without changing execution control."""
    message = _format_warning_message(findings, hook_event, source_desc)
    print(json.dumps({"systemMessage": message}, ensure_ascii=False))


# -- text extraction -------------------------------------------------------


def _extract_scan_text(input_data: dict, hook_event: str) -> str | None:
    """Extract the text to scan based on hook event type.

    Returns None if there's nothing meaningful to scan.
    """
    if hook_event == "UserPromptSubmit":
        text = input_data.get("prompt", "")
        if isinstance(text, str) and text.strip():
            return text
        return None

    if hook_event == "PreToolUse":
        tool_input = input_data.get("tool_input")
        if tool_input is None:
            return None
        # tool_input is a serde_json::Value — could be string, object, array
        if isinstance(tool_input, str):
            return tool_input if tool_input.strip() else None
        # For non-string types, serialize to text for scanning
        try:
            text = json.dumps(tool_input, ensure_ascii=False)
        except (TypeError, ValueError):
            return None
        # Empty containers serialize to non-empty strings ("{}", "[]",
        # "null") but carry no PII — skip to avoid a wasted scan-pii call.
        if not text.strip() or text in ("{}", "[]", "null"):
            return None
        return text

    if hook_event == "PostToolUse":
        tool_response = input_data.get("tool_response")
        if tool_response is None:
            return None
        # tool_response is a serde_json::Value — could be string, object, array
        if isinstance(tool_response, str):
            return tool_response if tool_response.strip() else None
        # For non-string types, serialize to text for scanning
        try:
            text = json.dumps(tool_response, ensure_ascii=False)
        except (TypeError, ValueError):
            return None
        # Empty containers serialize to non-empty strings ("{}", "[]",
        # "null") but carry no PII — skip to avoid a wasted scan-pii call.
        if not text.strip() or text in ("{}", "[]", "null"):
            return None
        return text

    return None


def _source_for_event(hook_event: str) -> str:
    """Return the --source argument value for agent-sec-cli."""
    if hook_event == "PreToolUse":
        return "tool_input"
    if hook_event == "PostToolUse":
        return "tool_output"
    return "user_input"


def _source_desc_for_event(hook_event: str) -> str:
    """Return a human-readable source description for a PII notice."""
    if hook_event == "PreToolUse":
        return "工具输入"
    if hook_event == "PostToolUse":
        return "工具输出"
    return "用户输入"


# -- main ------------------------------------------------------------------


def main() -> None:
    # 1. Read stdin JSON (fail-open: empty stdout = allow in Codex)
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        return

    # 2. Determine which hook event we're handling
    hook_event = input_data.get("hook_event_name", "")
    if hook_event not in ("UserPromptSubmit", "PreToolUse", "PostToolUse"):
        return  # unknown event, fail-open

    # 3. Extract text to scan
    scan_text = _extract_scan_text(input_data, hook_event)
    if not scan_text:
        return  # nothing to scan, allow

    # 4. Call agent-sec-cli scan-pii via subprocess
    source = _source_for_event(hook_event)
    try:
        cmd = with_trace_context(
            [
                "agent-sec-cli",
                "scan-pii",
                "--stdin",
                "--format",
                "json",
                "--source",
                source,
            ],
            input_data,
        )
        proc = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
            input=scan_text,
            text=True,
            timeout=TIMEOUT,
        )
    except Exception:
        return  # fail-open on subprocess error

    if proc.returncode != 0:
        return  # fail-open on CLI error

    # 5. Parse scan result JSON from stdout
    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        return  # fail-open on parse error

    # 6. Mode-based output
    verdict = _safe_text(scan_result.get("verdict")) or "pass"
    findings = _as_list(scan_result.get("findings"))

    if verdict == "pass" or not findings:
        return  # no PII detected, allow

    if MODE == "observe":
        return  # observe mode: don't block, audit only via CLI events
    elif MODE == "deny":
        source_desc = _source_desc_for_event(hook_event)
        if verdict == "warn":
            # systemMessage is supported by all three hook points and remains non-blocking.
            _warn(findings, hook_event, source_desc)
        else:
            # Preserve the existing fail-safe for deny or unexpected non-pass verdicts.
            _block(findings, hook_event, source_desc)
    # else: unknown mode, fail-open


if __name__ == "__main__":
    main()
