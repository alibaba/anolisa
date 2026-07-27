"""E2E checks for cosh hook command execution."""

import json
import os
import shlex
import subprocess
from pathlib import Path

_SYSTEM_EXTENSION_DIR = Path("/usr/share/anolisa/extensions/agent-sec-core")
_USER_EXTENSION_DIR = Path.home() / ".copilot-shell" / "extensions" / "agent-sec-core"
_SOURCE_EXTENSION_DIR = Path(__file__).resolve().parents[3] / "cosh-extension"


def _extension_dir() -> Path:
    if (_SYSTEM_EXTENSION_DIR / "cosh-extension.json").exists():
        return _SYSTEM_EXTENSION_DIR
    if (_USER_EXTENSION_DIR / "cosh-extension.json").exists():
        return _USER_EXTENSION_DIR
    return _SOURCE_EXTENSION_DIR


def _manifest_hook_commands(extension_dir: Path) -> list[str]:
    manifest = json.loads((extension_dir / "cosh-extension.json").read_text())
    commands: set[str] = set()
    for hook_groups in manifest["hooks"].values():
        for group in hook_groups:
            for hook in group.get("hooks", []):
                command = hook.get("command")
                if isinstance(command, str) and command.startswith("python3 "):
                    commands.add(command)
    return sorted(commands)


def test_cosh_manifest_hooks_are_directly_executable() -> None:
    extension_dir = _extension_dir()
    commands = _manifest_hook_commands(extension_dir)
    assert commands

    env = os.environ.copy()
    env.pop("PYTHONPATH", None)

    failed: list[str] = []
    for command in commands:
        argv = [
            part.replace("${extensionPath}", str(extension_dir))
            for part in shlex.split(command)
        ]
        proc = subprocess.run(
            argv,
            input="{}\n",
            capture_output=True,
            check=False,
            env=env,
            text=True,
            timeout=5,
        )
        if proc.returncode != 0:
            failed.append(
                f"{command}: exit={proc.returncode}, stderr={proc.stderr.strip()}"
            )
            continue
        try:
            json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            failed.append(f"{command}: invalid stdout JSON: {exc}: {proc.stdout!r}")

    assert failed == []
