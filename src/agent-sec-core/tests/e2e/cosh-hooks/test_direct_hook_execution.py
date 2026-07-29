"""E2E checks for cosh hook command execution."""

import json
import os
import shlex
import subprocess
import sys
import textwrap
from pathlib import Path

_SYSTEM_EXTENSION_DIR = Path("/usr/share/anolisa/extensions/agent-sec-core")
_USER_EXTENSION_DIR = Path.home() / ".copilot-shell" / "extensions" / "agent-sec-core"
_SOURCE_EXTENSION_DIR = Path(__file__).resolve().parents[3] / "cosh-extension"
_CODE_SCANNER_HOOK = _SOURCE_EXTENSION_DIR / "hooks" / "code_scanner_hook.py"


_MOCK_CLI_SCRIPT = f"#!{sys.executable}\n" + textwrap.dedent("""\
    import json
    import os
    import sys

    capture_path = os.environ.get("_MOCK_CLI_CAPTURE")
    if capture_path:
        with open(capture_path, "w", encoding="utf-8") as handle:
            json.dump({"argv": sys.argv[1:]}, handle)

    output = os.environ.get("_MOCK_CLI_OUTPUT", "")
    if output:
        print(output)
    sys.exit(0)
    """)


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


def _run_code_scanner_hook(tmp_path: Path, env_extra: dict[str, str]):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    cli = bin_dir / "agent-sec-cli"
    cli.write_text(_MOCK_CLI_SCRIPT)
    cli.chmod(0o755)
    capture = tmp_path / "capture.json"
    env = os.environ.copy()
    env.update(
        {
            "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""),
            "_MOCK_CLI_CAPTURE": str(capture),
            "_MOCK_CLI_OUTPUT": json.dumps(
                {"verdict": "deny", "findings": [{"desc_zh": "危险命令"}]}
            ),
        }
    )
    env.update(env_extra)
    proc = subprocess.run(
        [sys.executable, str(_CODE_SCANNER_HOOK)],
        input=json.dumps(
            {"tool_name": "shell", "tool_input": {"command": "rm -rf /secret/path"}}
        ),
        capture_output=True,
        check=False,
        env=env,
        text=True,
        timeout=15,
    )
    return proc, capture


def test_cosh_code_scanner_hook_enabled_false_allows_without_scan(
    tmp_path: Path,
) -> None:
    proc, capture = _run_code_scanner_hook(
        tmp_path,
        {"CODE_SCANNER_HOOK_ENABLED": "false"},
    )

    assert proc.returncode == 0
    assert json.loads(proc.stdout) == {"decision": "allow"}
    assert proc.stderr == ""
    assert not capture.exists()


def test_cosh_code_scanner_invalid_enabled_value_defaults_to_enabled(
    tmp_path: Path,
) -> None:
    proc, capture = _run_code_scanner_hook(
        tmp_path,
        {"CODE_SCANNER_HOOK_ENABLED": "maybe"},
    )

    assert proc.returncode == 0
    assert json.loads(proc.stdout)["decision"] == "ask"
    assert proc.stderr == ""
    assert capture.exists()


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
