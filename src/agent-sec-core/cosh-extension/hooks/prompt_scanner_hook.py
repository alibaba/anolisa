#!/usr/bin/env python3
"""Cosh hook script for prompt-scanner.

Reads a cosh UserPromptSubmit JSON from stdin, extracts the user prompt,
invokes ``agent-sec-cli scan-prompt`` via subprocess, and writes a cosh
HookOutput JSON to stdout.

Usage::

    python3 prompt_scanner_hook.py          # reads stdin, writes stdout

Hook point: **UserPromptSubmit** — fires when the user submits a prompt.
Input schema::

    {
        "session_id": "...",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "<user prompt text>"
    }

This script is intentionally self-contained — it does NOT import any
``agent_sec_cli`` package.  All it needs is the standard library and the
``agent-sec-cli`` on $PATH.
"""

import json
import subprocess
import sys

from trace_context import with_trace_context

# -- config ----------------------------------------------------------------

_DEFAULT_MODE = "standard"
_DEFAULT_SOURCE = "user_input"


# -- helpers ---------------------------------------------------------------


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _ask(reason: str) -> str:
    """Return an 'ask' cosh HookOutput so the user sees the issue and decides."""
    return json.dumps({"decision": "ask", "reason": reason}, ensure_ascii=False)


def _build_detail_reason(scan_result: dict) -> str:
    """Build a detailed reason string from scan result for security operations."""
    threat_type = scan_result.get("threat_type", "")
    risk_level = scan_result.get("risk_level", "unknown")
    confidence = scan_result.get("confidence")

    lines = [
        f"[prompt-scanner] 检测到安全风险",
        f"  攻击类型 : {threat_type or 'unknown'}",
        f"  风险等级 : {risk_level}",
        f"  拦截环节 : 用户输入扫描 (UserPromptSubmit)",
    ]
    if confidence is not None:
        try:
            lines.append(f"  模型置信度: {float(confidence) * 100:.1f}%")
        except (TypeError, ValueError):
            pass

    return "\n".join(lines)


def _format_cosh(scan_result: dict) -> str:
    """Convert a ScanResult dict into a cosh HookOutput JSON string.

    Mapping:
        verdict == "pass"  -> decision "allow"
        verdict == "warn"  -> decision "ask"  (let user decide)
        verdict == "deny"  -> decision "ask"  (let user decide)
        otherwise           -> decision "ask"  (fail-ask: surface scan failure to user)
    """
    verdict = scan_result.get("verdict")

    if verdict == "pass":
        return json.dumps({"decision": "allow"})

    reason = _build_detail_reason(scan_result)

    if verdict == "warn":
        return json.dumps(
            {"decision": "ask", "reason": reason},
            ensure_ascii=False,
        )
    # Use "ask" to avoid blocking users outright.
    # TODO: switch to "block" once the policy is mature enough.
    if verdict == "deny":
        return json.dumps(
            {"decision": "ask", "reason": reason},
            ensure_ascii=False,
        )
    # missing, error, or unknown verdict -> fail-ask: let user decide
    error_detail = scan_result.get("summary", verdict or "unknown")
    return _ask(f"[prompt-scanner] 扫描异常 (verdict={verdict}): {error_detail}")


# -- main ------------------------------------------------------------------


def main() -> None:
    # 1. Read stdin JSON (UserPromptSubmit event)
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        print(_allow())
        return

    # 2. Extract user prompt text
    prompt_text = input_data.get("prompt", "")
    if not prompt_text or not isinstance(prompt_text, str) or not prompt_text.strip():
        print(_allow())
        return

    # 3. Call CLI. Model download/loading is owned by the daemon.
    try:
        cmd = with_trace_context(
            [
                "agent-sec-cli",
                "scan-prompt",
                "--text",
                prompt_text,
                "--mode",
                _DEFAULT_MODE,
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
            text=True,
            timeout=10,
        )
    except subprocess.TimeoutExpired as exc:
        print(
            f"[prompt-scanner] CLI timed out after {exc.timeout}s",
            file=sys.stderr,
        )
        print(_ask(f"[prompt-scanner] 安全扫描超时 ({exc.timeout}s)，未完成扫描"))
        return
    except Exception as exc:
        print(f"[prompt-scanner] CLI invocation failed: {exc}", file=sys.stderr)
        print(_ask(f"[prompt-scanner] 安全扫描调用失败: {exc}"))
        return

    if proc.returncode != 0:
        stderr_tail = (proc.stderr or "").strip().splitlines()[-5:]
        print(
            f"[prompt-scanner] CLI exited with code {proc.returncode}:"
            f" {'; '.join(stderr_tail)}",
            file=sys.stderr,
        )
        print(_ask(f"[prompt-scanner] 安全扫描异常退出 (code={proc.returncode})"))
        return

    # 4. Parse ScanResult JSON from stdout
    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError) as exc:
        print(
            f"[prompt-scanner] failed to parse CLI output: {exc}",
            file=sys.stderr,
        )
        print(_ask("[prompt-scanner] 安全扫描结果解析失败，未完成扫描"))
        return

    # 5. Format and print cosh output
    print(_format_cosh(scan_result))


if __name__ == "__main__":
    main()
