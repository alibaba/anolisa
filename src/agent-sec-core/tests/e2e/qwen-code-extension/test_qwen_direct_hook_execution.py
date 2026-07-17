"""E2E checks for Qwen Code command-hook execution."""

import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

_RPM_EXTENSION_DIR = Path("/opt/agent-sec/qwen-code-extension")
_SYSTEM_EXTENSION_DIR = Path("/usr/local/lib/anolisa/sec-core/qwen-code-extension")
_USER_EXTENSION_DIR = (
    Path.home() / ".local" / "lib" / "anolisa" / "sec-core" / "qwen-code-extension"
)
_SOURCE_EXTENSION_DIR = Path(__file__).resolve().parents[3] / "qwen-code-extension"
_ZERO_RUN_ID = "00000000-0000-0000-0000-000000000000"


def _extension_dir() -> Path:
    override = os.environ.get("QWEN_CODE_EXTENSION_E2E_DIR")
    if override:
        return Path(override).expanduser().resolve()
    for extension_dir in (
        _RPM_EXTENSION_DIR,
        _SYSTEM_EXTENSION_DIR,
        _USER_EXTENSION_DIR,
    ):
        if (extension_dir / "qwen-extension.json").exists():
            return extension_dir
    return _SOURCE_EXTENSION_DIR


def _manifest_hook_commands(extension_dir: Path) -> dict[str, list[str]]:
    manifest = json.loads(
        (extension_dir / "qwen-extension.json").read_text(encoding="utf-8")
    )
    commands: dict[str, list[str]] = {}
    for event_name, hook_groups in manifest["hooks"].items():
        event_commands: list[str] = []
        for group in hook_groups:
            for hook in group.get("hooks", []):
                command = hook.get("command")
                if isinstance(command, str) and command.startswith("python3 "):
                    event_commands.append(command)
        commands[event_name] = event_commands
    return commands


def _command_argv(command: str, extension_dir: Path) -> list[str]:
    expanded = command.replace("${extensionPath}", str(extension_dir)).replace(
        "${/}", os.sep
    )
    return shlex.split(expanded)


def _event_argvs(extension_dir: Path, event_name: str) -> list[list[str]]:
    commands = _manifest_hook_commands(extension_dir)[event_name]
    assert commands
    return [_command_argv(command, extension_dir) for command in commands]


def _ensure_agent_sec_cli(env: dict[str, str], tmp_path: Path) -> None:
    if shutil.which("agent-sec-cli", path=env.get("PATH")):
        return
    wrapper_dir = tmp_path / "bin"
    wrapper_dir.mkdir()
    wrapper = wrapper_dir / "agent-sec-cli"
    wrapper.write_text(
        f"#!{sys.executable}\n"
        "import runpy\n"
        'runpy.run_module("agent_sec_cli.cli", run_name="__main__")\n',
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    env["PATH"] = f"{wrapper_dir}{os.pathsep}{env.get('PATH', '')}"


def _run_hooks(
    extension_dir: Path,
    input_data: dict,
    *,
    env: dict[str, str],
    cwd: Path,
) -> list[subprocess.CompletedProcess]:
    event_name = input_data["hook_event_name"]
    argvs = _event_argvs(extension_dir, event_name)
    return [
        subprocess.run(
            argv,
            input=json.dumps(input_data),
            capture_output=True,
            check=False,
            cwd=cwd,
            env=env,
            text=True,
            timeout=15,
        )
        for argv in argvs
    ]


def _payload(event_name: str, timestamp: str, **fields) -> dict:
    return {
        "session_id": "qwen-e2e-session",
        "transcript_path": "/unused/qwen-e2e-transcript.jsonl",
        "cwd": "/workspace",
        "hook_event_name": event_name,
        "timestamp": timestamp,
        **fields,
    }


def test_qwen_manifest_hooks_are_directly_executable(tmp_path) -> None:
    extension_dir = _extension_dir()
    commands_by_event = _manifest_hook_commands(extension_dir)
    assert set(commands_by_event) == {
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "Stop",
        "StopFailure",
    }

    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    env["AGENT_SEC_DATA_DIR"] = str(tmp_path / "agent-sec-data")

    failed: list[str] = []
    commands = sorted(
        {
            command
            for event_commands in commands_by_event.values()
            for command in event_commands
        }
    )
    for command in commands:
        proc = subprocess.run(
            _command_argv(command, extension_dir),
            input="{}\n",
            capture_output=True,
            check=False,
            cwd=tmp_path,
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
            output = json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            failed.append(f"{command}: invalid stdout JSON: {exc}: {proc.stdout!r}")
            continue
        if output != {}:
            failed.append(f"{command}: expected fail-open empty output, got {output!r}")

    assert failed == []


def test_qwen_observability_lifecycle_records_through_cli(tmp_path) -> None:
    extension_dir = _extension_dir()
    data_dir = tmp_path / "agent-sec-data"
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    env["AGENT_SEC_DATA_DIR"] = str(data_dir)
    _ensure_agent_sec_cli(env, tmp_path)

    payloads = [
        _payload(
            "UserPromptSubmit",
            "2026-07-14T10:00:00Z",
            prompt="List repository files.",
        ),
        _payload(
            "UserPromptSubmit",
            "2026-07-14T10:00:01Z",
            prompt="",
        ),
        _payload(
            "PreToolUse",
            "2026-07-14T10:00:02Z",
            permission_mode="default",
            tool_name="run_shell_command",
            tool_input={"command": "pwd"},
            tool_call_id="call-success",
            tool_use_id="call-success",
        ),
        _payload(
            "PostToolUse",
            "2026-07-14T10:00:03Z",
            permission_mode="default",
            tool_name="run_shell_command",
            tool_input={"command": "pwd"},
            tool_call_id="call-success",
            tool_use_id="call-success",
            tool_response={"returnDisplay": {"stdout": "/workspace\n", "exitCode": 0}},
        ),
        _payload(
            "Stop",
            "2026-07-14T10:00:04Z",
            stop_hook_active=False,
            last_assistant_message="Done.",
        ),
        _payload(
            "UserPromptSubmit",
            "2026-07-14T10:01:00Z",
            prompt="Run a failing command.",
        ),
        _payload(
            "PreToolUse",
            "2026-07-14T10:01:01Z",
            permission_mode="default",
            tool_name="run_shell_command",
            tool_input={"command": "exit 1"},
            tool_call_id="call-failure",
            tool_use_id="call-failure",
        ),
        _payload(
            "PostToolUseFailure",
            "2026-07-14T10:01:02Z",
            permission_mode="default",
            tool_name="run_shell_command",
            tool_input={"command": "exit 1"},
            tool_call_id="call-failure",
            tool_use_id="call-failure",
            error="command failed",
            is_interrupt=False,
        ),
        _payload(
            "StopFailure",
            "2026-07-14T10:01:03Z",
            error="API_ERROR",
            error_details="model request failed",
            last_assistant_message="",
        ),
    ]

    for payload in payloads:
        procs = _run_hooks(extension_dir, payload, env=env, cwd=tmp_path)
        for proc in procs:
            assert proc.returncode == 0, proc.stderr
            assert json.loads(proc.stdout) == {}

    records = [
        json.loads(line)
        for line in (data_dir / "observability.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
    ]
    assert [record["hook"] for record in records] == [
        "before_agent_run",
        "before_tool_call",
        "after_tool_call",
        "after_agent_run",
        "before_agent_run",
        "before_tool_call",
        "after_tool_call",
        "after_agent_run",
    ]
    assert [
        record["metrics"]["prompt"]
        for record in records
        if record["hook"] == "before_agent_run"
    ] == [
        "List repository files.",
        "Run a failing command.",
    ]
    assert {record["metadata"]["runId"] for record in records} == {_ZERO_RUN_ID}
    assert [
        record["metadata"]["toolCallId"]
        for record in records
        if "toolCallId" in record["metadata"]
    ] == ["call-success", "call-success", "call-failure", "call-failure"]
    assert [
        record["metrics"]["status"]
        for record in records
        if record["hook"] == "after_tool_call"
    ] == ["success", "error"]
    assert [
        record["metrics"]["success"]
        for record in records
        if record["hook"] == "after_agent_run"
    ] == [True, False]
    assert not (data_dir / "qwen-code-extension").exists()
