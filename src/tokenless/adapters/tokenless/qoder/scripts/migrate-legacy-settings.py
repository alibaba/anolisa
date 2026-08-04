#!/usr/bin/env python3
"""Remove only legacy tokenless entries from Qoder settings.

The native Qoder plugin owns its registration through ``qodercli plugins``.
This helper exists solely to migrate the exact settings shape written by old
tokenless adapters. It fails closed on unsafe paths or unexpected JSON shapes.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any


PLUGIN_ID = "tokenless@local"

LEGACY_HOOKS = {
    "PreToolUse": [
        {
            "matcher": "",
            "sequential": True,
            "name": "tokenless-tool-ready",
            "description": (
                "Pre-checks tool environment readiness, auto-fixes, and provides "
                "skip-retry guidance"
            ),
            "interpreter": "bash",
            "script": "tool_ready_hook.sh",
            "timeout": 10000,
        },
        {
            "matcher": "^(Bash|Shell|run_shell_command|terminal|execute_command)$",
            "name": "tokenless-rewrite",
            "description": "Rewrites shell commands via rtk for token savings",
            "interpreter": "python3",
            "script": "rewrite_hook.py",
            "timeout": 5000,
            "accept_agent_id_arg": True,
        },
    ],
    "PostToolUse": [
        {
            "matcher": "",
            "name": "tokenless-compress-response",
            "description": "Compresses tool responses and encodes to TOON format",
            "interpreter": "python3",
            "script": "compress_response_hook.py",
            "timeout": 10000,
            "accept_agent_id_arg": True,
        }
    ],
}


class MigrationError(RuntimeError):
    """Raised when settings cannot be migrated without risking user data."""


def _matches_command(
    command: Any,
    interpreter: str,
    script: str,
    legacy_hooks_roots: tuple[Path, ...],
    accept_agent_id_arg: bool,
) -> bool:
    if not isinstance(command, str):
        return False
    try:
        parts = shlex.split(command)
    except ValueError:
        return False
    if len(parts) not in (2, 4) or parts[0] != interpreter:
        return False
    if len(parts) == 4 and (
        not accept_agent_id_arg or parts[2:] != ["--agent-id", "qoder-cli"]
    ):
        return False
    path = Path(os.path.normpath(parts[1]))
    return path.is_absolute() and any(
        path == legacy_hooks_root / script
        for legacy_hooks_root in legacy_hooks_roots
    )


def _matches_legacy_entry(
    event: str, entry: Any, legacy_hooks_roots: tuple[Path, ...]
) -> bool:
    if not isinstance(entry, dict):
        return False
    hooks = entry.get("hooks")
    if not isinstance(hooks, list) or len(hooks) != 1 or not isinstance(hooks[0], dict):
        return False
    hook = hooks[0]

    for spec in LEGACY_HOOKS[event]:
        expected_entry_keys = {"matcher", "hooks"}
        if spec.get("sequential") is True:
            expected_entry_keys.add("sequential")
        if set(entry) != expected_entry_keys:
            continue
        if entry.get("matcher") != spec["matcher"]:
            continue
        if entry.get("sequential") != spec.get("sequential"):
            continue
        if set(hook) != {"type", "name", "description", "command", "timeout", "env"}:
            continue
        if hook.get("type") != "command":
            continue
        if hook.get("name") != spec["name"] or hook.get("description") != spec["description"]:
            continue
        if hook.get("env") != {"TOKENLESS_AGENT_ID": "qoder-cli"}:
            continue
        if hook.get("timeout") != spec["timeout"]:
            continue
        if _matches_command(
            hook.get("command"),
            spec["interpreter"],
            spec["script"],
            legacy_hooks_roots,
            spec.get("accept_agent_id_arg", False),
        ):
            return True
    return False


def _prune(
    config: dict[str, Any], legacy_hooks_roots: tuple[Path, ...]
) -> tuple[dict[str, Any], int]:
    removed = 0
    hooks = config.get("hooks")
    if hooks is not None:
        if not isinstance(hooks, dict):
            raise MigrationError("settings field 'hooks' is not an object")
        for event in list(hooks):
            entries = hooks[event]
            if not isinstance(entries, list):
                raise MigrationError(f"settings hook event '{event}' is not an array")
            if event not in LEGACY_HOOKS:
                continue
            kept = []
            for entry in entries:
                if _matches_legacy_entry(event, entry, legacy_hooks_roots):
                    removed += 1
                else:
                    kept.append(entry)
            if kept:
                hooks[event] = kept
            else:
                hooks.pop(event)
        if not hooks:
            config.pop("hooks", None)

    plugins = config.get("plugins")
    if plugins is not None:
        if not isinstance(plugins, dict):
            raise MigrationError("settings field 'plugins' is not an object")
        enabled = plugins.get("enabled")
        if enabled is not None:
            if not isinstance(enabled, list):
                raise MigrationError("settings field 'plugins.enabled' is not an array")
            new_enabled = [item for item in enabled if item != PLUGIN_ID]
            removed += len(enabled) - len(new_enabled)
            if new_enabled:
                plugins["enabled"] = new_enabled
            else:
                plugins.pop("enabled", None)
        if not plugins:
            config.pop("plugins", None)

    return config, removed


def _load_regular_file(path: Path) -> tuple[dict[str, Any], os.stat_result] | None:
    try:
        before = path.lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(before.st_mode):
        raise MigrationError(f"refusing non-regular settings path: {path}")

    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise MigrationError(f"cannot safely open {path}: {exc}") from exc
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
            raise MigrationError(f"settings path changed while opening: {path}")
        with os.fdopen(fd, encoding="utf-8") as stream:
            fd = -1
            try:
                data = json.load(stream)
            except json.JSONDecodeError as exc:
                raise MigrationError(f"invalid JSON in {path}: {exc}") from exc
    finally:
        if fd >= 0:
            os.close(fd)
    if not isinstance(data, dict):
        raise MigrationError("Qoder settings root is not an object")
    return data, before


def _atomic_write(path: Path, config: dict[str, Any], before: os.stat_result) -> None:
    current = path.lstat()
    if not stat.S_ISREG(current.st_mode) or (
        current.st_dev,
        current.st_ino,
    ) != (before.st_dev, before.st_ino):
        raise MigrationError(f"settings path changed before write: {path}")

    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp_path: Path | None = Path(temp_name)
    try:
        os.fchmod(fd, stat.S_IMODE(before.st_mode))
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            fd = -1
            json.dump(config, stream, indent=2, ensure_ascii=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_path, path)
        temp_path = None
        dir_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        if fd >= 0:
            os.close(fd)
        if temp_path is not None:
            try:
                temp_path.unlink()
            except FileNotFoundError:
                pass


def migrate(path: Path, legacy_hooks_roots: list[Path]) -> int:
    """Migrate ``path`` and return the number of removed legacy entries."""
    if any(not root.is_absolute() for root in legacy_hooks_roots):
        raise MigrationError("legacy hooks roots must be absolute")
    normalized_roots = tuple(
        dict.fromkeys(Path(os.path.normpath(root)) for root in legacy_hooks_roots)
    )
    loaded = _load_regular_file(path)
    if loaded is None:
        return 0
    config, before = loaded
    config, removed = _prune(config, normalized_roots)
    if removed:
        _atomic_write(path, config, before)
    return removed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--legacy-hooks-root", required=True, action="append", type=Path
    )
    parser.add_argument("settings", type=Path)
    args = parser.parse_args()
    try:
        removed = migrate(args.settings, args.legacy_hooks_root)
    except (MigrationError, OSError) as exc:
        print(f"legacy Qoder settings migration failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({"ok": True, "removed": removed}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
