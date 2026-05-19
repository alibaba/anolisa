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
from pathlib import Path

from trace_context import with_trace_context

# -- config ----------------------------------------------------------------

_DEFAULT_MODE = "standard"
_DEFAULT_SOURCE = "user_input"

# Model cache directory — mirrors ModelManager._DEFAULT_CACHE_DIR.
_MODEL_CACHE_DIR = Path.home() / ".cache" / "prompt_scanner" / "models"

# Permanent marker: once the user has been reminded about warmup, skip
# further ask dialogs until the model is downloaded.
_REMINDER_MARKER_DIR = Path.home() / ".cache" / "agent-sec" / "prompt-scanner"
_REMINDER_MARKER_FILE = _REMINDER_MARKER_DIR / "warmup-reminded"


# -- helpers ---------------------------------------------------------------


def _is_model_downloaded() -> bool:
    """Check whether any local model has been downloaded.

    Looks for a config.json file two levels under the cache dir
    (i.e. <cache>/<org>/<model>/config.json), which mirrors the
    same check used by ``ModelManager._resolve_local_model_path``.
    """
    if not _MODEL_CACHE_DIR.exists():
        return False
    return any(_MODEL_CACHE_DIR.glob("*/*/config.json"))


def _is_warmup_reminded() -> bool:
    """Check whether the warmup reminder has already been shown.

    Once reminded, the marker file persists until the model is downloaded.
    No TTL — this is permanent suppression.
    """
    return _REMINDER_MARKER_FILE.exists()


def _mark_warmup_reminded() -> None:
    """Write a marker file to suppress future warmup ask dialogs.

    Best-effort; failures are silently ignored so that permission issues
    never break the hook.
    """
    try:
        _REMINDER_MARKER_DIR.mkdir(parents=True, exist_ok=True)
        _REMINDER_MARKER_FILE.write_text("reminded")
    except OSError:
        pass


def _cleanup_warmup_marker() -> None:
    """Remove the warmup-reminded marker file if it exists.

    Called once the model is downloaded so that the marker does not
    accumulate indefinitely.  Best-effort; failures are silently ignored.
    """
    try:
        if _REMINDER_MARKER_FILE.exists():
            _REMINDER_MARKER_FILE.unlink()
    except OSError:
        pass


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _format_cosh(scan_result: dict) -> str:
    """Convert a ScanResult dict into a cosh HookOutput JSON string.

    Mapping:
        verdict == "pass"  -> decision "allow"
        verdict == "warn"  -> decision "ask"  (let user decide)
        verdict == "deny"  -> decision "ask"  (let user decide)
        otherwise           -> fail-open "allow"
    """
    verdict = scan_result.get("verdict", "pass")

    if verdict == "pass":
        return json.dumps({"decision": "allow"})

    # Build reason from summary; it already contains threat type, confidence & evidence.
    summary = scan_result.get("summary", "")
    threat_type = scan_result.get("threat_type", "")
    msg = f"[prompt-scanner] {summary or threat_type or 'Prompt rejected by security policy'}"

    if verdict == "warn":
        return json.dumps(
            {"decision": "ask", "reason": msg},
            ensure_ascii=False,
        )
    # Use "ask" to avoid blocking users outright.
    # TODO: switch to "block" once the policy is mature enough.
    if verdict == "deny":
        return json.dumps(
            {"decision": "ask", "reason": msg},
            ensure_ascii=False,
        )
    # other error or unknown verdict -> fail-open
    return json.dumps({"decision": "allow"})


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

    # 3. Check if the local model is available.
    #    If not, show a one-time ask reminder, then silently allow forever.
    #    NOTE: _mark_warmup_reminded() is called *before* we know the user's
    #    choice (Yes/No).  This is intentional — the cosh hook API does not
    #    provide feedback on the user's decision, so we cannot conditionally
    #    mark.  The trade-off is acceptable: the reminder appears once, and
    #    users who cancel can still run warmup manually.
    if not _is_model_downloaded():
        if _is_warmup_reminded():
            # Already reminded — silently allow without invoking CLI.
            print(_allow())
            return
        # First time — ask the user, then mark as reminded.
        _mark_warmup_reminded()
        warmup_msg = (
            "[prompt-scanner] ⚠️  安全扫描组件尚未完成初始化，本次 prompt 未经安全检测。\n"
            "需要一次性下载本地检测小模型才能启用扫描功能。\n"
            "请在终端执行以下命令完成下载，之后无需再次操作：\n"
            "  agent-sec-cli scan-prompt warmup\n"
            "\n"
            "你仍可以选择继续发送（Yes），或取消（No）后先完成下载。\n"
            "此提醒仅出现一次。"
        )
        print(json.dumps({"decision": "ask", "reason": warmup_msg}, ensure_ascii=False))
        return

    # 4. Model exists — clean up stale warmup marker, then call CLI
    _cleanup_warmup_marker()
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
    except Exception:
        # Timeout or other error -> fail-open
        print(_allow())
        return

    if proc.returncode != 0:
        print(_allow())
        return

    # 5. Parse ScanResult JSON from stdout
    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        print(_allow())
        return

    # 6. Format and print cosh output
    print(_format_cosh(scan_result))


if __name__ == "__main__":
    main()
