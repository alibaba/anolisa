#!/usr/bin/env python3
"""Scan Qwen Code hook content for PII and credentials.

Qwen Code compatibility contract
--------------------------------
This hook targets the Qwen Code 0.19.9 protocol:

* ``PostToolUse`` stops downstream handling through ``continue=false``;
  ``decision=block`` alone is not consumed by that version.
* ``PostToolUseFailure`` and ``StopFailure`` do not consume blocking output, so
  they remain scan-and-audit only.
* ``stop_hook_active`` identifies a repeated ``Stop`` pass, which must not be
  blocked again or it can create a rewrite loop.
* Qwen Code has no pre-render output-transform hook, so final-output blocking is
  best effort.

The 0.19.9 HookInput does not expose a runtime version field. Compatibility is
therefore pinned by protocol tests instead of inferred inside the hook.
"""

import json
import math
import os
import subprocess
import sys
from typing import Any

from pii_text import value_to_text
from qwen_trace_context import with_trace_context

_DEFAULT_TIMEOUT_SECONDS = 5.0
_MAX_TIMEOUT_SECONDS = 8.0
_MAX_PAYLOAD_SIZE = 1024 * 1024
_MAX_EVIDENCE_ITEMS = 3
_MAX_EVIDENCE_CHARS = 80
_TRUE_VALUES = {"1", "true", "yes", "on"}
_FALSE_VALUES = {"0", "false", "no", "off"}
_VALID_VERDICTS = {"pass", "warn", "deny", "error"}

_USER_INPUT_SOURCE = "user_input"
_TOOL_INPUT_SOURCE = "tool_input"
_TOOL_OUTPUT_SOURCE = "tool_output"
_MODEL_OUTPUT_SOURCE = "model_output"


def _noop() -> dict[str, Any]:
    """Return a Qwen Code HookOutput that does not alter execution."""
    return {}


def _environment_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    normalized = value.strip().lower()
    if normalized in _TRUE_VALUES:
        return True
    if normalized in _FALSE_VALUES:
        return False
    return default


def _mode() -> str:
    value = os.environ.get("PII_CHECKER_MODE", "observe").strip().lower()
    if value == "deny":
        return "block"
    return value if value in {"observe", "block"} else "observe"


def _timeout_seconds() -> float:
    try:
        value = float(os.environ.get("PII_CHECKER_TIMEOUT", _DEFAULT_TIMEOUT_SECONDS))
    except (TypeError, ValueError):
        return _DEFAULT_TIMEOUT_SECONDS
    if not math.isfinite(value) or value <= 0:
        return _DEFAULT_TIMEOUT_SECONDS
    return min(value, _MAX_TIMEOUT_SECONDS)


def _read_hook_input() -> dict[str, Any] | None:
    """Read one bounded Qwen Code HookInput object from stdin."""
    try:
        stream = getattr(sys.stdin, "buffer", sys.stdin)
        payload = stream.read(_MAX_PAYLOAD_SIZE + 1)
        if len(payload) > _MAX_PAYLOAD_SIZE:
            return None
        input_data = json.loads(payload)
    except (json.JSONDecodeError, EOFError, OSError, TypeError, ValueError):
        return None
    return input_data if isinstance(input_data, dict) else None


def _string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _scan_target(input_data: dict[str, Any]) -> tuple[str, str, str] | None:
    """Return event name, scanner source, and exact text for supported hooks."""
    event_name = _string(input_data.get("hook_event_name"))
    if event_name == "UserPromptSubmit":
        text = _string(input_data.get("prompt"))
        return (event_name, _USER_INPUT_SOURCE, text) if text.strip() else None
    if event_name == "PreToolUse" and "tool_input" in input_data:
        text = value_to_text(input_data.get("tool_input"))
        return (event_name, _TOOL_INPUT_SOURCE, text) if text.strip() else None
    if event_name == "PostToolUse" and "tool_response" in input_data:
        text = value_to_text(input_data.get("tool_response"))
        return (event_name, _TOOL_OUTPUT_SOURCE, text) if text.strip() else None
    if event_name == "PostToolUseFailure":
        text = _string(input_data.get("error"))
        return (event_name, _TOOL_OUTPUT_SOURCE, text) if text.strip() else None
    if event_name in {"Stop", "StopFailure"}:
        text = _string(input_data.get("last_assistant_message"))
        return (event_name, _MODEL_OUTPUT_SOURCE, text) if text.strip() else None
    return None


def _scan_pii(
    input_data: dict[str, Any], text: str, source: str
) -> dict[str, Any] | None:
    """Run scan-pii with raw content on stdin and parse its JSON response."""
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
    if _environment_bool("PII_CHECKER_INCLUDE_LOW_CONFIDENCE", False):
        args.append("--include-low-confidence")

    try:
        result = subprocess.run(
            with_trace_context(args, input_data),
            input=text,
            capture_output=True,
            text=True,
            timeout=_timeout_seconds(),
            check=False,
        )
    except (OSError, subprocess.SubprocessError, TimeoutError):
        return None
    if result.returncode != 0:
        return None

    try:
        scan_result = json.loads(result.stdout)
    except (json.JSONDecodeError, TypeError, ValueError):
        return None
    return scan_result if isinstance(scan_result, dict) else None


def _shorten_evidence(value: str) -> str:
    normalized = " ".join(value.split())
    if len(normalized) <= _MAX_EVIDENCE_CHARS:
        return normalized
    return normalized[: _MAX_EVIDENCE_CHARS - 3] + "..."


def _validated_result(
    scan_result: dict[str, Any],
) -> tuple[str, list[str]] | None:
    """Return a supported verdict with audit-safe evidence only."""
    verdict = scan_result.get("verdict")
    if not isinstance(verdict, str) or verdict not in _VALID_VERDICTS:
        return None
    if verdict == "pass":
        return verdict, []
    if verdict == "error":
        return None

    findings = scan_result.get("findings")
    if not isinstance(findings, list):
        return None
    evidence: list[str] = []
    for finding in findings:
        if not isinstance(finding, dict):
            continue
        redacted = finding.get("evidence_redacted")
        if not isinstance(redacted, str) or not redacted.strip():
            continue
        shortened = _shorten_evidence(redacted)
        if shortened not in evidence:
            evidence.append(shortened)
        if len(evidence) >= _MAX_EVIDENCE_ITEMS:
            break
    return (verdict, evidence) if evidence else None


def _notice(evidence: list[str], action: str) -> str:
    return (
        "[pii-checker] Sensitive data detected. Redacted evidence: "
        f"{', '.join(evidence)}. {action}"
    )


def _decision(
    input_data: dict[str, Any], event_name: str, verdict: str, evidence: list[str]
) -> dict[str, Any]:
    """Map one validated scanner verdict to Qwen Code HookOutput."""
    if event_name in {"PostToolUseFailure", "StopFailure"}:
        # Qwen Code 0.19.9 does not consume control fields for these failure
        # events, so scanning remains audit-only instead of claiming a block.
        return _noop()

    if verdict == "warn" or _mode() == "observe":
        return {
            "systemMessage": _notice(evidence, "Execution will continue."),
        }

    if event_name == "UserPromptSubmit":
        reason = _notice(evidence, "Remove the sensitive data and submit again.")
        return {
            "decision": "block",
            "reason": reason,
            "hookSpecificOutput": {"hookEventName": event_name},
        }
    if event_name == "PreToolUse":
        reason = _notice(evidence, "This tool call was blocked.")
        return {
            "hookSpecificOutput": {
                "hookEventName": event_name,
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }
    if event_name == "PostToolUse":
        reason = _notice(
            evidence,
            "Do not use or repeat the sensitive tool output.",
        )
        # Qwen Code 0.19.9 only stops PostToolUse through
        # shouldStopExecution(), which checks continue=false. Keep the
        # documented decision fields for policy intent and future compatibility.
        return {
            "continue": False,
            "stopReason": reason,
            "decision": "block",
            "reason": reason,
        }
    if event_name == "Stop":
        if input_data.get("stop_hook_active") is True:
            return {
                "systemMessage": _notice(
                    evidence,
                    "The response will not be blocked again to avoid a retry loop.",
                )
            }
        reason = _notice(
            evidence,
            "Rewrite the final response using placeholders; do not repeat the original values.",
        )
        return {"decision": "block", "reason": reason}
    return _noop()


def main() -> None:
    """Run the Qwen Code PII checker hook with fail-open behavior."""
    try:
        input_data = _read_hook_input()
        if input_data is None or not _environment_bool("PII_CHECKER_ENABLED", True):
            print(json.dumps(_noop()))
            return

        target = _scan_target(input_data)
        if target is None:
            print(json.dumps(_noop()))
            return
        event_name, source, text = target

        scan_result = _scan_pii(input_data, text, source)
        if scan_result is None:
            print(json.dumps(_noop()))
            return
        validated = _validated_result(scan_result)
        if validated is None:
            print(json.dumps(_noop()))
            return
        verdict, evidence = validated
        output = (
            _noop()
            if verdict == "pass"
            else _decision(input_data, event_name, verdict, evidence)
        )
        print(json.dumps(output, ensure_ascii=False))
    except Exception:  # noqa: BLE001 - hook failures must remain silent and fail-open
        print(json.dumps(_noop()))


if __name__ == "__main__":
    main()
