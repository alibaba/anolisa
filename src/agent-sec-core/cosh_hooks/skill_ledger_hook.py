#!/usr/bin/env python3
"""Cosh hook script for skill-ledger.

Reads a cosh PreToolUse JSON from stdin, resolves the skill directory
from the skill name, invokes ``agent-sec-cli skill-ledger check`` via
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

Output mapping (design doc §4 — warning-only, never block):

    status "pass"     → { "decision": "allow" }
    status otherwise  → { "decision": "allow", "reason": "⚠️ ..." }

Copilot-shell settings.json configuration::

    {
      "hooks": {
        "PreToolUse": [{
          "matcher": "skill",
          "hooks": [{
            "type": "command",
            "name": "skill-ledger",
            "command": "python3 cosh_hooks/skill_ledger_hook.py",
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

# -- constants ---------------------------------------------------------------

_TOOL_NAME = "skill"
_CHECK_TIMEOUT = 5  # seconds for the CLI check call
_INIT_TIMEOUT = 3  # seconds for key initialization

# Warning messages per status (design doc §4)
_WARNING_MESSAGES = {
    "warn": "\u26a0\ufe0f Skill '{name}' has low-risk findings \u2014 review recommended",
    "drifted": "\u26a0\ufe0f Skill '{name}' content has changed since last scan",
    "none": "\u26a0\ufe0f Skill '{name}' has not been security-scanned yet",
    "error": "\u26a0\ufe0f Skill '{name}' check failed \u2014 invalid path or missing SKILL.md",
    "deny": (
        "\U0001f6a8 Skill '{name}' has high-risk findings"
        " \u2014 immediate review recommended"
    ),
    "tampered": ("\U0001f6a8 Skill '{name}' metadata signature verification failed"),
}


# -- helpers -----------------------------------------------------------------


def _allow() -> str:
    """Return a permissive cosh HookOutput JSON string."""
    return json.dumps({"decision": "allow"})


def _allow_with_reason(reason: str) -> str:
    """Return an allow decision with a warning reason for display."""
    return json.dumps({"decision": "allow", "reason": reason}, ensure_ascii=False)


def _resolve_skill_dir(skill_name: str, cwd: str) -> tuple[str | None, bool]:
    """Resolve a skill name to its on-disk directory.

    Search order mirrors copilot-shell's SkillManager priority:
    project (.copilot-shell/skills/) → user (~/.copilot-shell/skills/)
    → system (/usr/share/anolisa/skills/) → extensions
    (~/.copilot-shell/extensions/*/).

    Returns ``(path, traversal_detected)``:
    - ``(str, False)`` — resolved successfully.
    - ``(None, True)`` — path escapes the skills base (traversal attempt).
    - ``(None, False)`` — not found (remote or unknown skill).
    """
    traversal_detected = False
    bases = [
        Path(cwd) / ".copilot-shell" / "skills",
        Path.home() / ".copilot-shell" / "skills",
        Path("/usr/share/anolisa/skills"),
    ]
    for base in bases:
        candidate = base / skill_name
        try:
            resolved = candidate.resolve()
        except (OSError, ValueError):
            continue
        if not resolved.is_relative_to(base.resolve()):
            traversal_detected = True
            continue  # path-traversal attempt — skip this base
        if resolved.is_dir() and (resolved / "SKILL.md").is_file():
            return str(resolved), False

    # Search extension skill directories
    ext_result = _resolve_extension_skill_dir(skill_name)
    if ext_result is not None:
        return ext_result, False

    return None, traversal_detected


def _get_extensions_dir() -> Path:
    """Return the global copilot-shell extensions directory."""
    return Path.home() / ".copilot-shell" / "extensions"


def _read_json_file(filepath: Path) -> dict | None:
    """Read and parse a JSON file, returning ``None`` on any error."""
    try:
        return json.loads(filepath.read_text(encoding="utf-8"))
    except Exception:
        return None


def _effective_extension_path(ext_dir: Path) -> Path:
    """Resolve the effective extension path, following ``link`` installs.

    If ``.qwen-extension-install.json`` declares ``"type": "link"``, the
    ``source`` field points to the real extension root.
    """
    install_meta = _read_json_file(ext_dir / ".qwen-extension-install.json")
    if install_meta and install_meta.get("type") == "link":
        source = install_meta.get("source", "")
        if source:
            linked = Path(source)
            if linked.is_dir():
                return linked
    return ext_dir


def _extension_skill_bases(ext_path: Path) -> list[Path]:
    """Read extension config and return the list of skill base directories.

    Mirrors ``getSkillDirs()`` in copilot-shell's extensionManager.ts.
    """
    config: dict | None = None
    for config_name in ("cosh-extension.json", "qwen-extension.json"):
        config = _read_json_file(ext_path / config_name)
        if config is not None:
            break
    if config is None:
        return []

    skills_field = config.get("skills")
    if skills_field is None:
        dirs = ["skills"]
    elif isinstance(skills_field, list):
        dirs = [str(d) for d in skills_field]
    else:
        dirs = [str(skills_field)]

    return [ext_path / d for d in dirs]


def _find_skill_recursive(
    base: Path,
    skill_name: str,
    trust_root: Path,
) -> str | None:
    """Recursively search *base* for a skill directory named *skill_name*.

    Mirrors copilot-shell's ``loadSkillsFromDir`` recursion:
    directories containing ``SKILL.md`` are skills; others are containers
    that are recursed into.

    *trust_root* is used for path-traversal protection — the resolved
    path must stay within *trust_root*.
    """
    if not base.is_dir():
        return None
    try:
        entries = sorted(base.iterdir())
    except OSError:
        return None

    for entry in entries:
        if not entry.is_dir():
            continue
        has_manifest = (entry / "SKILL.md").is_file()
        if has_manifest:
            if entry.name == skill_name:
                try:
                    resolved = entry.resolve()
                    if resolved.is_relative_to(trust_root.resolve()):
                        return str(resolved)
                except (OSError, ValueError):
                    pass
        else:
            # Container directory (no SKILL.md) — recurse into it
            result = _find_skill_recursive(entry, skill_name, trust_root)
            if result is not None:
                return result
    return None


def _resolve_extension_skill_dir(skill_name: str) -> str | None:
    """Search installed extensions for a skill directory named *skill_name*.

    Iterates every extension under ``~/.copilot-shell/extensions/``, reads
    its config to determine skill directories, then recursively searches
    each for ``<skill_name>/SKILL.md``.
    """
    extensions_dir = _get_extensions_dir()
    if not extensions_dir.is_dir():
        return None

    for ext_entry in sorted(extensions_dir.iterdir()):
        if not ext_entry.is_dir():
            continue
        effective_path = _effective_extension_path(ext_entry)
        skill_bases = _extension_skill_bases(effective_path)
        for skill_base in skill_bases:
            result = _find_skill_recursive(skill_base, skill_name, effective_path)
            if result is not None:
                return result
    return None


def _keys_exist() -> bool:
    """Return True if both key.pub and key.enc exist."""
    xdg_data = os.environ.get("XDG_DATA_HOME", "")
    if not xdg_data:
        xdg_data = str(Path.home() / ".local" / "share")
    data_dir = Path(xdg_data) / "skill-ledger"
    return (data_dir / "key.pub").is_file() and (data_dir / "key.enc").is_file()


def _ensure_keys() -> None:
    """Auto-initialize signing keys if missing (fire-and-forget)."""
    if _keys_exist():
        return
    try:
        subprocess.run(
            ["agent-sec-cli", "skill-ledger", "init-keys"],
            capture_output=True,
            text=True,
            timeout=_INIT_TIMEOUT,
        )
    except Exception:
        pass


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

    # 3. Resolve skill directory
    cwd = input_data.get("cwd", os.environ.get("COPILOT_SHELL_PROJECT_DIR", "."))
    skill_dir, traversal = _resolve_skill_dir(skill_name, cwd)
    if traversal:
        reason = "\U0001f6a8 Skill '{}' rejected: path traversal detected".format(
            skill_name
        )
        print(_allow_with_reason(reason))
        return
    if skill_dir is None:
        # Not found in any location (project/user/system/extension) — remote or unknown → fail-open
        reason = (
            "\u26a0\ufe0f Skill '{}' not found on disk \u2014 check skipped".format(
                skill_name
            )
        )
        print(_allow_with_reason(reason))
        return

    # 4. Ensure signing keys exist (auto-init if missing)
    _ensure_keys()

    # 5. Call agent-sec-cli skill-ledger check <skill_dir>
    try:
        proc = subprocess.run(
            ["agent-sec-cli", "skill-ledger", "check", skill_dir],
            capture_output=True,
            text=True,
            timeout=_CHECK_TIMEOUT,
        )
    except Exception:
        # Timeout or CLI not found → fail-open
        print(_allow())
        return

    # 6. Parse check result JSON — the CLI may use non-zero exit codes
    #    for non-pass statuses (e.g. deny → rc=1), so we parse stdout
    #    regardless of returncode.
    try:
        check_result = json.loads(proc.stdout)
    except (json.JSONDecodeError, ValueError):
        print(_allow())
        return

    status = check_result.get("status", "unknown")

    # 7. Format output — always allow, warn on non-pass (design doc §4)
    if status == "pass":
        print(_allow())
        return

    template = _WARNING_MESSAGES.get(status)
    if template:
        reason = template.format(name=skill_name)
    else:
        reason = "\u26a0\ufe0f Skill '{}' has unknown status '{}'".format(
            skill_name, status
        )

    print(_allow_with_reason(reason))


if __name__ == "__main__":
    main()
