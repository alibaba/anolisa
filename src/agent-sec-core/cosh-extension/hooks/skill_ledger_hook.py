#!/usr/bin/env python3
"""Cosh hook script for skill-ledger.

Reads a cosh PreToolUse JSON from stdin, resolves the skill directory
from the skill context or skill name, invokes ``agent-sec-cli skill-ledger check`` via
subprocess, and writes a cosh HookOutput JSON to stdout.

Hook point: **PreToolUse** — matcher: ``skill``

Input schema::

    {
        "session_id": "...",
        "hook_event_name": "PreToolUse",
        "tool_name": "skill",
        "tool_input": { "skill": "<skill-name>" },
        "cwd": "/path/to/project"
    }

Output mapping:

    status "pass"     → { "decision": "allow" }
    status "none"     → { "decision": "ask", "reason": "⚠️ ..." }
    status "drifted"  → { "decision": "ask", "reason": "⚠️ ..." }
    status "deny"     → { "decision": "ask", "reason": "🚨 ..." }
    status "tampered" → { "decision": "ask", "reason": "🚨 ..." }
    status otherwise  → { "decision": "allow", "reason": "⚠️ ..." }

Copilot-shell settings.json configuration::

    {
      "hooks": {
        "PreToolUse": [{
          "matcher": "skill",
          "hooks": [{
            "type": "command",
            "name": "skill-ledger",
            "command": "python3 cosh-extension/hooks/skill_ledger_hook.py",
            "timeout": 10000
          }]
        }]
      }
    }

This script is intentionally self-contained — it does NOT import any
``agent_sec_cli`` package.  All it needs is the standard library and the
``agent-sec-cli`` on $PATH.
"""

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from trace_context import with_trace_context

# -- constants ---------------------------------------------------------------

_TOOL_NAME = "skill"
_CHECK_TIMEOUT = 5  # seconds for the CLI check call
_INIT_TIMEOUT = 3  # seconds for key initialization

_ASK_STATUSES = frozenset({"none", "drifted", "deny", "tampered"})

_STATUS_MESSAGES = {
    "warn": "\u26a0\ufe0f Skill '{name}' has low-risk findings \u2014 review recommended",
    "drifted": (
        "\u26a0\ufe0f Skill '{name}' content has changed since last scan"
        " \u2014 confirm before using and run a fresh scan when possible"
    ),
    "none": (
        "\u26a0\ufe0f Skill '{name}' has not been security-scanned yet"
        " \u2014 confirm before using"
    ),
    "error": "\u26a0\ufe0f Skill '{name}' check failed \u2014 invalid path or missing SKILL.md",
    "deny": (
        "\U0001f6a8 Skill '{name}' has high-risk findings"
        " \u2014 confirm only if you trust the skill and intend to review it"
    ),
    "tampered": (
        "\U0001f6a8 Skill '{name}' metadata signature verification failed"
        " \u2014 confirm only if you trust the skill source"
    ),
}


# -- helpers -----------------------------------------------------------------


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _allow_with_reason(reason: str) -> str:
    """Return an allow decision with a warning reason for display."""
    return json.dumps({"decision": "allow", "reason": reason}, ensure_ascii=False)


def _ask_with_reason(reason: str) -> str:
    """Return an ask decision with a confirmation reason for display."""
    return json.dumps({"decision": "ask", "reason": reason}, ensure_ascii=False)


def _debug(message: str) -> None:
    """Write debug-only hook details to stderr."""
    print(f"[skill-ledger debug] {message}", file=sys.stderr)


def _supported_skill_bases(cwd: str) -> list[Path]:
    """Return the skill roots currently covered by this hook.

    Current scope is intentionally limited to:
    project (.copilot-shell/skills/) → user (~/.copilot-shell/skills/)
    → system (/usr/share/anolisa/skills/).
    """
    return [
        Path(cwd) / ".copilot-shell" / "skills",
        Path.home() / ".copilot-shell" / "skills",
        Path("/usr/share/anolisa/skills"),
    ]


def _resolve_supported_skill_bases(cwd: str, skill_name: str) -> list[Path]:
    """Resolve supported skill roots, skipping only roots that fail."""
    supported_bases: list[Path] = []
    for base in _supported_skill_bases(cwd):
        try:
            supported_bases.append(base.resolve())
        except (OSError, ValueError) as exc:
            _debug(
                "Skill '{}' check skipped for base '{}': failed to resolve: {}".format(
                    skill_name, base, exc
                )
            )
    return supported_bases


def _resolve_skill_dir_from_context(
    input_data: dict, cwd: str, skill_name: str
) -> tuple[str | None, bool]:
    """Resolve the skill dir from ``skill_context.file_path`` when available.

    Returns ``(skill_dir, handled)``.  ``handled`` is True whenever a
    well-formed ``skill_context.file_path`` was present, even if the path is
    outside the supported project/user/system scope.  In that case the caller
    should fail open without falling back to name-based lookup, because the
    context identifies the actual skill that copilot-shell resolved.
    """
    skill_context = input_data.get("skill_context")
    if not isinstance(skill_context, dict):
        return None, False

    file_path = skill_context.get("file_path")
    if not isinstance(file_path, str) or not file_path.strip():
        return None, False

    try:
        skill_file = Path(file_path).expanduser().resolve()
    except (OSError, ValueError) as exc:
        _debug(
            "Skill '{}' check skipped: invalid skill_context.file_path '{}': {}".format(
                skill_name, file_path, exc
            )
        )
        return None, True

    supported_bases = _resolve_supported_skill_bases(cwd, skill_name)
    if not supported_bases:
        _debug(
            "Skill '{}' check skipped: no supported skill bases could be resolved".format(
                skill_name
            )
        )
        return None, True

    if not any(skill_file.is_relative_to(base) for base in supported_bases):
        _debug(
            "Skill '{}' at '{}' is outside current skill-ledger hook scope "
            "(project/user/system); check skipped".format(skill_name, skill_file)
        )
        return None, True

    if skill_file.name != "SKILL.md" or not skill_file.is_file():
        _debug(
            "Skill '{}' check skipped: skill_context.file_path '{}' does not "
            "point to an existing SKILL.md".format(skill_name, skill_file)
        )
        return None, True

    return str(skill_file.parent), True


def _resolve_skill_dir(skill_name: str, cwd: str) -> tuple[str | None, bool]:
    """Resolve a skill name to its on-disk directory.

    Current hook scope is intentionally limited to:
    project (.copilot-shell/skills/) → user (~/.copilot-shell/skills/)
    → system (/usr/share/anolisa/skills/).

    Returns ``(path, traversal_detected)``:
    - ``(str, False)`` — resolved successfully.
    - ``(None, True)`` — path escapes the skills base (traversal attempt).
    - ``(None, False)`` — not found (remote or unknown skill).
    """
    traversal_detected = False
    bases = _supported_skill_bases(cwd)
    for base in bases:
        candidate = base / skill_name
        try:
            resolved_base = base.resolve()
            resolved = candidate.resolve()
        except (OSError, ValueError):
            continue
        if not resolved.is_relative_to(resolved_base):
            traversal_detected = True
            continue  # path-traversal attempt — skip this base
        if resolved.is_dir() and (resolved / "SKILL.md").is_file():
            return str(resolved), False

    return None, traversal_detected


def _keys_exist() -> bool:
    """Return True if both key.pub and key.enc exist."""
    xdg_data = os.environ.get("XDG_DATA_HOME", "")
    if not xdg_data:
        xdg_data = str(Path.home() / ".local" / "share")
    data_dir = Path(xdg_data) / "agent-sec" / "skill-ledger"
    return (data_dir / "key.pub").is_file() and (data_dir / "key.enc").is_file()


def _ensure_keys(input_data: dict[str, Any]) -> None:
    """Auto-initialize signing keys if missing (fire-and-forget)."""
    if _keys_exist():
        return
    try:
        cmd = with_trace_context(
            ["agent-sec-cli", "skill-ledger", "init", "--no-baseline"],
            input_data,
        )
        subprocess.run(
            cmd,
            capture_output=True,
            check=False,
            text=True,
            timeout=_INIT_TIMEOUT,
        )
    except Exception:
        pass


def _format_cosh(check_result: dict, skill_name: str) -> str:
    """Convert a check-result dict into a cosh HookOutput JSON string.

    Mapping:
        status == "pass"                         → decision "allow" (silent)
        none / drifted / deny / tampered         → decision "ask" + reason
        warn / error / unknown / other statuses  → decision "allow" + reason
    """
    status = check_result.get("status", "unknown")

    if status == "pass":
        return _allow()

    template = _STATUS_MESSAGES.get(status)
    if template:
        reason = template.format(name=skill_name)
    else:
        reason = "\u26a0\ufe0f Skill '{}' has unknown status '{}'".format(
            skill_name, status
        )

    if status in _ASK_STATUSES:
        return _ask_with_reason(reason)

    return _allow_with_reason(reason)


# -- main --------------------------------------------------------------------


def main() -> None:
    """Entry point — read stdin, check skill, write stdout."""
    # 1. Read stdin JSON (PreToolUse event)
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        print(_allow())
        return

    # 2. Verify this is a skill tool call
    tool_name = input_data.get("tool_name", "")
    if tool_name != _TOOL_NAME:
        print(_allow())
        return

    tool_input = input_data.get("tool_input", {})
    skill_name = tool_input.get("skill", "")
    if not isinstance(skill_name, str):
        print(
            _allow_with_reason(
                "\u26a0\ufe0f Skill check skipped: skill name must be a string"
            )
        )
        return
    skill_name = skill_name.strip()
    if not skill_name:
        print(
            _allow_with_reason(
                "\u26a0\ufe0f Skill check skipped: skill name is empty or missing"
            )
        )
        return

    # 3. Resolve skill directory.  Prefer copilot-shell's resolved file path
    # when present so SKILL.md names may differ from directory names, but only
    # within the current project/user/system scope.
    cwd = input_data.get("cwd", os.environ.get("COPILOT_SHELL_PROJECT_DIR", "."))
    skill_dir, context_handled = _resolve_skill_dir_from_context(
        input_data, cwd, skill_name
    )
    if context_handled:
        if skill_dir is None:
            print(_allow())
            return
        traversal = False
    else:
        skill_dir, traversal = _resolve_skill_dir(skill_name, cwd)

    if traversal:
        reason = "\U0001f6a8 Skill '{}' rejected: path traversal detected".format(
            skill_name
        )
        print(_allow_with_reason(reason))
        return
    if skill_dir is None:
        # Not found in any supported location (project/user/system) → fail-open
        reason = (
            "\u26a0\ufe0f Skill '{}' not found on disk \u2014 check skipped".format(
                skill_name
            )
        )
        print(_allow_with_reason(reason))
        return

    # 4. Ensure signing keys exist (auto-init if missing)
    _ensure_keys(input_data)

    # 5. Call agent-sec-cli skill-ledger check <skill_dir>
    try:
        cmd = with_trace_context(
            ["agent-sec-cli", "skill-ledger", "check", skill_dir],
            input_data,
        )
        proc = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
            text=True,
            timeout=_CHECK_TIMEOUT,
        )
    except Exception:
        # Timeout or CLI not found → fail-open
        print(_allow())
        return

    # 6. Parse check result and format output
    try:
        check_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        print(_allow())
        return

    print(_format_cosh(check_result, skill_name))


if __name__ == "__main__":
    main()
