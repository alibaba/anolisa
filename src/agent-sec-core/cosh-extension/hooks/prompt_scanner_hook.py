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

# -- config ----------------------------------------------------------------

_DEFAULT_MODE = "standard"
_DEFAULT_SOURCE = "user_input"


# -- helpers ---------------------------------------------------------------


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _format_cosh(scan_result: dict) -> str:
    """Convert a ScanResult dict into a cosh HookOutput JSON string.

    Mapping:
        verdict == "pass"  -> decision "allow"
        verdict == "warn"  -> decision "ask"  (let user decide)
        verdict == "deny"  -> decision "ask"  (let user decide)
        otherwise          -> fail-open "allow"
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
    # error or unknown -> fail-open
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

    # 3. Call agent-sec-cli scan-prompt via subprocess
    try:
        proc = subprocess.run(
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
            capture_output=True,
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

    # 4. Parse ScanResult JSON from stdout
    try:
        scan_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        print(_allow())
        return

    # 5. Format and print cosh output
    print(_format_cosh(scan_result))


if __name__ == "__main__":
    main()
