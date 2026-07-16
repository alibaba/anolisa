"""Unit tests for the Qoder plugin framework shell."""

import importlib.util
import json
import os
import shlex
import stat
import subprocess
import sys
import textwrap
import tomllib
from pathlib import Path

_CORE_DIR = Path(__file__).resolve().parents[3]
_PLUGIN_DIR = _CORE_DIR / "qoder-plugin"
_HOOKS_DIR = _PLUGIN_DIR / "hooks"
_INSTALL_SCRIPT = _PLUGIN_DIR / "install.sh"

_spec = importlib.util.spec_from_file_location(
    "qoder_hook_common", _HOOKS_DIR / "qoder_hook_common.py"
)
qoder_hook_common = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = qoder_hook_common
_spec.loader.exec_module(qoder_hook_common)


def _write_executable(path: Path, content: str) -> None:
    path.write_text(content)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


def _package_version() -> str:
    pyproject = tomllib.loads(
        (_CORE_DIR / "agent-sec-cli" / "pyproject.toml").read_text()
    )
    return pyproject["project"]["version"]


def _python_version_script(version: tuple[int, int]) -> str:
    major, minor = version
    return textwrap.dedent(f"""\
        #!/usr/bin/env bash
        set -euo pipefail
        {{
            echo 'import sys'
            echo 'sys.version_info = ({major}, {minor}, 0)'
            cat
        }} | {shlex.quote(sys.executable)} -
        """)


def _run_install_script(
    tmp_path: Path,
    *,
    qodercli_script: str,
    python_script: str | None = None,
    agent_sec_cli_script: str | None = None,
    args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    calls_path = tmp_path / "qoder-calls.txt"

    _write_executable(bin_dir / "qodercli", qodercli_script)
    if python_script is not None:
        _write_executable(bin_dir / "python3", python_script)
    if agent_sec_cli_script is not None:
        _write_executable(bin_dir / "agent-sec-cli", agent_sec_cli_script)

    env = {
        **os.environ,
        "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""),
        "QODER_CALLS": str(calls_path),
    }
    return subprocess.run(
        [str(_INSTALL_SCRIPT), *(args or [])],
        capture_output=True,
        check=False,
        env=env,
        text=True,
        timeout=15,
    )


_QODER_WITH_PLUGINS = textwrap.dedent("""\
    #!/usr/bin/env bash
    set -euo pipefail
    echo "$*" >> "$QODER_CALLS"
    case "$*" in
        "plugins --help"|"plugins install --help"|"plugins validate --help")
            exit 0
            ;;
        plugins\\ validate\\ *|plugins\\ install\\ *|plugins\\ uninstall\\ *)
            exit 0
            ;;
        *)
            exit 2
            ;;
    esac
    """)

_QODER_WITHOUT_PLUGINS = textwrap.dedent("""\
    #!/usr/bin/env bash
    set -euo pipefail
    echo "$*" >> "$QODER_CALLS"
    exit 2
    """)

_AGENT_SEC_CLI = textwrap.dedent("""\
    #!/usr/bin/env bash
    set -euo pipefail
    [[ "$*" == "scan-pii --help" ]]
    """)


def test_plugin_manifest_declares_stable_name() -> None:
    manifest = json.loads((_PLUGIN_DIR / ".qoder-plugin" / "plugin.json").read_text())

    assert manifest["name"] == "agent-sec-core"
    assert manifest["version"] == _package_version()


def test_component_manifest_does_not_declare_qoder_adapter() -> None:
    component = tomllib.loads((_CORE_DIR / "adapters" / "component.toml").read_text())
    frameworks = {adapter["framework"] for adapter in component.get("adapters", [])}

    assert "qoder" not in frameworks


def test_hooks_json_uses_qoder_plugin_wrapper() -> None:
    hooks = json.loads((_HOOKS_DIR / "hooks.json").read_text())

    assert set(hooks) == {"hooks"}
    assert set(hooks["hooks"]) == {"UserPromptSubmit", "PreToolUse", "PostToolUse"}


def test_common_outputs_qoder_hook_shapes() -> None:
    deny = json.loads(qoder_hook_common.deny_output("blocked"))
    pre_tool = json.loads(qoder_hook_common.pre_tool_decision_output("deny", "nope"))
    post_tool = json.loads(qoder_hook_common.post_tool_output_replacement("redacted"))

    assert deny == {
        "decision": "deny",
        "reason": "blocked",
        "systemMessage": "blocked",
    }
    assert pre_tool == {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": "nope",
        }
    }
    assert post_tool == {
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": "redacted",
        }
    }


def test_trace_context_marks_qoder_agent() -> None:
    args = qoder_hook_common.with_trace_context(
        ["agent-sec-cli", "scan-pii"],
        {"session_id": "sess-1", "tool_use_id": "tool-1"},
    )

    assert args[:2] == ["agent-sec-cli", "--trace-context"]
    context = json.loads(args[2])
    assert context == {
        "agent_name": "qoder",
        "session_id": "sess-1",
        "tool_call_id": "tool-1",
    }


def test_install_rejects_qodercli_without_plugin_support(tmp_path: Path) -> None:
    proc = _run_install_script(
        tmp_path,
        qodercli_script=_QODER_WITHOUT_PLUGINS,
    )

    assert proc.returncode != 0
    assert "does not support plugins" in proc.stderr


def test_install_rejects_python_below_311(tmp_path: Path) -> None:
    proc = _run_install_script(
        tmp_path,
        qodercli_script=_QODER_WITH_PLUGINS,
        python_script=_python_version_script((3, 10)),
        agent_sec_cli_script=_AGENT_SEC_CLI,
    )

    assert proc.returncode != 0
    assert "python3 >= 3.11 and < 3.12 is required" in proc.stderr


def test_install_rejects_python_312_or_newer(tmp_path: Path) -> None:
    proc = _run_install_script(
        tmp_path,
        qodercli_script=_QODER_WITH_PLUGINS,
        python_script=_python_version_script((3, 12)),
        agent_sec_cli_script=_AGENT_SEC_CLI,
    )

    assert proc.returncode != 0
    assert "python3 >= 3.11 and < 3.12 is required" in proc.stderr


def test_remove_does_not_require_agent_sec_cli_or_python(tmp_path: Path) -> None:
    proc = _run_install_script(
        tmp_path,
        qodercli_script=_QODER_WITH_PLUGINS,
        args=["--remove"],
    )

    assert proc.returncode == 0, proc.stderr
    calls = (tmp_path / "qoder-calls.txt").read_text().splitlines()
    assert "plugins --help" in calls
    assert "plugins uninstall agent-sec-core --scope user" in calls


def test_install_validates_and_installs(tmp_path: Path) -> None:
    proc = _run_install_script(
        tmp_path,
        qodercli_script=_QODER_WITH_PLUGINS,
        python_script=_python_version_script((3, 11)),
        agent_sec_cli_script=_AGENT_SEC_CLI,
    )

    assert proc.returncode == 0, proc.stderr
    calls = (tmp_path / "qoder-calls.txt").read_text().splitlines()
    assert "plugins --help" in calls
    assert "plugins install --help" in calls
    assert "plugins validate --help" in calls
    assert f"plugins validate {_PLUGIN_DIR}" in calls
    assert f"plugins install {_PLUGIN_DIR} --scope user" in calls
