"""Unit tests for cosh-extension/hooks/skill_ledger_hook.py.

The hook is self-contained (no agent_sec_cli imports), so we test it
by piping JSON via subprocess — identical to the code_scanner_hook tests.

Tests are grouped into three categories:

1. **Fail-open paths** — invalid input, wrong tool, missing skill dir.
   These never invoke the CLI and verify the hook always returns allow.
2. **Skill directory resolution** — project-level lookup, missing SKILL.md.
3. **Output mapping** — exposure summary message → prompt formatting.
   Uses a mock CLI script to return canned show results, verifying that only
   ``message`` controls user prompts.
"""

import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

# Path to the standalone cosh hook script
_COSH_HOOK = str(
    Path(__file__).resolve().parents[2]
    / ".."
    / "cosh-extension"
    / "hooks"
    / "skill_ledger_hook.py"
)

sys.path.insert(0, str(Path(_COSH_HOOK).parent))
import skill_ledger_hook  # noqa: E402

_COSH_MANIFEST = Path(_COSH_HOOK).parents[1] / "cosh-extension.json"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _run_hook(input_data, *, env_override=None, return_stderr=False):
    """Run the hook as a subprocess with *input_data* as stdin JSON.

    Returns the parsed JSON output dict.
    """
    env = os.environ.copy()
    if env_override:
        env.update(env_override)
    proc = subprocess.run(
        [sys.executable, _COSH_HOOK],
        input=json.dumps(input_data) if isinstance(input_data, dict) else input_data,
        capture_output=True,
        check=False,
        text=True,
        timeout=15,
        env=env,
    )
    assert proc.returncode == 0, f"Hook stderr: {proc.stderr}"
    output = json.loads(proc.stdout)
    if return_stderr:
        return output, proc.stderr
    return output


def _make_skill_event(skill_name, cwd=".", skill_file_path=None):
    """Build a minimal PreToolUse event for the skill tool."""
    event = {
        "hook_event_name": "PreToolUse",
        "tool_name": "skill",
        "tool_input": {"skill": skill_name},
        "cwd": cwd,
    }
    if skill_file_path is not None:
        event["skill_context"] = {
            "skill_name": skill_name,
            "file_path": str(skill_file_path),
        }
    return event


def _create_skill_dir(parent, name="test-skill", manifest_name=None):
    """Create a minimal skill directory with a SKILL.md file.

    Returns the absolute path to ``<parent>/.copilot-shell/skills/<name>/``.
    """
    manifest_name = manifest_name or name
    skill_dir = Path(parent) / ".copilot-shell" / "skills" / name
    skill_dir.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_text(
        f"---\nname: {manifest_name}\ndescription: A test skill\n---\nHello\n"
    )
    return str(skill_dir)


def test_cosh_manifest_registers_skill_ledger_by_default():
    """Default Cosh installs keep the Skill Ledger hook mounted."""
    manifest = json.loads(_COSH_MANIFEST.read_text(encoding="utf-8"))
    registered_hook_names = [
        hook.get("name")
        for event_hooks in manifest.get("hooks", {}).values()
        for matcher_hooks in event_hooks
        for hook in matcher_hooks.get("hooks", [])
    ]

    assert "skill-ledger" in registered_hook_names


def test_injects_trace_context_into_skill_ledger_show_command(monkeypatch, capsys):
    captured = {}

    def fake_run(args, **kwargs):
        captured["args"] = args
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(
            args=args,
            returncode=0,
            stdout=json.dumps({"latestStatus": "pass", "message": None}),
            stderr="",
        )

    monkeypatch.setattr(skill_ledger_hook, "_ensure_keys", lambda _input_data: None)
    monkeypatch.setattr(
        skill_ledger_hook,
        "_resolve_skill_dir",
        lambda _skill_name, _cwd: ("/project/.copilot-shell/skills/test-skill", False),
    )
    monkeypatch.setattr(skill_ledger_hook.subprocess, "run", fake_run)
    monkeypatch.setattr(
        skill_ledger_hook.sys,
        "stdin",
        io.StringIO(
            json.dumps(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "skill",
                    "tool_input": {"skill": "test-skill"},
                    "cwd": "/project",
                    "trace_id": "trace-1",
                    "session_id": "session-1",
                    "run_id": "run-1",
                    "tool_use_id": "tool-1",
                }
            )
        ),
    )

    skill_ledger_hook.main()

    output = json.loads(capsys.readouterr().out)
    expected_context = json.dumps(
        {
            "agent_name": "cosh",
            "trace_id": "trace-1",
            "session_id": "session-1",
            "run_id": "run-1",
            "tool_call_id": "tool-1",
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
    assert output == {"decision": "allow"}
    assert captured["args"] == [
        "agent-sec-cli",
        "--trace-context",
        expected_context,
        "skill-ledger",
        "show",
        "/project/.copilot-shell/skills/test-skill",
    ]
    assert captured["kwargs"]["check"] is False


# ---------------------------------------------------------------------------
# Fail-open tests — these never invoke the real CLI
# ---------------------------------------------------------------------------


class TestFailOpen:
    """Every error / unrecognized input must produce ``{"decision": "allow"}``."""

    def test_invalid_json_allows(self):
        """Malformed stdin should fail-open."""
        output = _run_hook("not-json")
        assert output == {"decision": "allow"}

    def test_empty_stdin_allows(self):
        output = _run_hook("")
        assert output == {"decision": "allow"}

    def test_wrong_tool_name_allows(self):
        output = _run_hook(
            {
                "tool_name": "run_shell_command",
                "tool_input": {"command": "echo hello"},
            }
        )
        assert output == {"decision": "allow"}

    def test_missing_tool_name_allows(self):
        output = _run_hook({"tool_input": {"skill": "test"}})
        assert output == {"decision": "allow"}

    def test_missing_skill_name_allows(self):
        output, stderr = _run_hook(
            {"tool_name": "skill", "tool_input": {}},
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "empty or missing" in stderr

    def test_empty_skill_name_allows(self):
        output, stderr = _run_hook(
            {"tool_name": "skill", "tool_input": {"skill": ""}},
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "empty or missing" in stderr

    def test_whitespace_skill_name_allows(self):
        output, stderr = _run_hook(
            {"tool_name": "skill", "tool_input": {"skill": "   "}},
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "empty or missing" in stderr

    def test_nonstring_skill_name_allows(self):
        output, stderr = _run_hook(
            {"tool_name": "skill", "tool_input": {"skill": 42}},
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "must be a string" in stderr

    def test_skill_dir_not_found_allows(self):
        """Skill name that resolves to no on-disk directory → fail-open."""
        output, stderr = _run_hook(
            _make_skill_event("nonexistent-skill-xyz", "/tmp"),
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "not found on disk" in stderr
        assert "nonexistent-skill-xyz" in stderr

    def test_path_traversal_blocked(self, tmp_path):
        """A ``../`` skill name that escapes the skills base emits a warning.

        Layout::
            <tmp>/project/.copilot-shell/skills/   (skills base — empty)
            <tmp>/project/.copilot-shell/evil/      (valid SKILL.md, outside base)

        ``../evil`` resolves outside the skills base → hook must warn about
        path traversal and never reach the CLI.
        """
        project = tmp_path / "project"
        skills_base = project / ".copilot-shell" / "skills"
        skills_base.mkdir(parents=True)
        evil = project / ".copilot-shell" / "evil"
        evil.mkdir()
        (evil / "SKILL.md").write_text("---\nname: evil\n---\n")

        output, stderr = _run_hook(
            _make_skill_event("../evil", str(project)),
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "reason" not in output
        assert "path traversal" in stderr
        assert "../evil" in stderr


# ---------------------------------------------------------------------------
# Skill directory resolution tests
# ---------------------------------------------------------------------------


class TestSkillDirResolution:
    """Verify the hook correctly locates skill directories."""

    def test_project_level_skill_found(self, mock_cli_env):
        """Skill in <cwd>/.copilot-shell/skills/<name>/ should be found.

        We verify by feeding a mock CLI that returns a non-empty ``message``.
        If the skill dir is found the hook calls the CLI and produces a
        ``reason`` field; if the skill dir were *not* found, the hook would
        return plain allow with no ``reason`` at all.
        """
        env = mock_cli_env["make_env"](
            json.dumps({"latestStatus": "drifted", "message": "drifted latest"})
        )
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "ask"
        assert "reason" in output, "Skill dir not found — CLI was never called"

    def test_skill_context_resolves_name_directory_mismatch(self, mock_cli_env):
        """skill_context.file_path should locate project skills by real path.

        This covers the case where the Skill tool receives the frontmatter
        name, but the on-disk directory uses a different name.
        """
        skill_dir = _create_skill_dir(
            mock_cli_env["cwd"],
            name="directory-name",
            manifest_name="frontmatter-name",
        )
        env = mock_cli_env["make_env"](
            json.dumps({"latestStatus": "drifted", "message": "drifted latest"})
        )
        output = _run_hook(
            _make_skill_event(
                "frontmatter-name",
                mock_cli_env["cwd"],
                Path(skill_dir) / "SKILL.md",
            ),
            env_override=env,
        )
        assert output["decision"] == "ask"
        assert "drifted latest" in output["reason"]

    def test_skill_context_skips_only_unresolvable_supported_base(self, mock_cli_env):
        """A bad project base should not discard user/system base checks."""
        home = Path(mock_cli_env["cwd"]).parent / "home"
        skill_dir = home / ".copilot-shell" / "skills" / "user-dir"
        skill_dir.mkdir(parents=True)
        skill_file = skill_dir / "SKILL.md"
        skill_file.write_text(
            "---\nname: user-skill\ndescription: A user skill\n---\nHello\n"
        )

        env = mock_cli_env["make_env"](
            json.dumps({"latestStatus": "drifted", "message": "drifted latest"})
        )
        env["HOME"] = str(home)
        output = _run_hook(
            _make_skill_event("user-skill", "\0bad-project", skill_file),
            env_override=env,
        )

        assert output["decision"] == "ask"
        assert "drifted latest" in output["reason"]

    @pytest.mark.parametrize("scope_name", ["custom", "extension", "remote"])
    def test_skill_context_outside_supported_scope_debug_skips(
        self, tmp_path, scope_name
    ):
        """custom/extension/remote paths are out of scope for this hook."""
        home = tmp_path / "home"
        project = tmp_path / "project"
        home.mkdir()
        project.mkdir()

        if scope_name == "custom":
            skill_dir = tmp_path / "custom-skills" / "custom-skill"
        elif scope_name == "extension":
            skill_dir = (
                home
                / ".copilot-shell"
                / "extensions"
                / "test-ext"
                / "skills"
                / "extension-skill"
            )
        else:
            skill_dir = (
                home / ".copilot-shell" / "remote-skills" / "system" / "remote-skill"
            )

        skill_dir.mkdir(parents=True)
        skill_file = skill_dir / "SKILL.md"
        skill_file.write_text(
            f"---\nname: {scope_name}-skill\ndescription: A test skill\n---\n"
        )

        output, stderr = _run_hook(
            _make_skill_event(f"{scope_name}-skill", str(project), skill_file),
            env_override={"HOME": str(home)},
            return_stderr=True,
        )

        assert output == {"decision": "allow"}
        assert "outside current skill-ledger hook scope" in stderr
        assert "project/user/system" in stderr

    def test_missing_skill_md_not_found(self):
        """Directory exists but no SKILL.md → not recognized as a skill."""
        with tempfile.TemporaryDirectory() as tmpdir:
            skill_dir = Path(tmpdir) / ".copilot-shell" / "skills" / "bad"
            skill_dir.mkdir(parents=True)
            # No SKILL.md file
            output, stderr = _run_hook(
                _make_skill_event("bad", tmpdir),
                return_stderr=True,
            )
            assert output == {"decision": "allow"}
            assert "reason" not in output
            assert "not found on disk" in stderr
            assert "bad" in stderr


# A tiny script that pretends to be agent-sec-cli.
# It reads _MOCK_CHECK_OUTPUT env var and prints it to stdout.
# For "init --no-baseline", it's a no-op.
_MOCK_CLI_SCRIPT = f"#!{sys.executable}\n" + textwrap.dedent("""\
    import os, sys
    # init --no-baseline → silent success
    if len(sys.argv) >= 4 and sys.argv[2] == "init" and sys.argv[3] == "--no-baseline":
        sys.exit(0)
    # show → return canned output from env
    output = os.environ.get("_MOCK_CHECK_OUTPUT", "")
    rc = int(os.environ.get("_MOCK_CHECK_RC", "0"))
    if output:
        print(output)
    sys.exit(rc)
""")


@pytest.fixture()
def mock_cli_env(tmp_path):
    """Create a fake ``agent-sec-cli`` and a skill dir in a temp project.

    Returns a dict with ``cwd``, ``skill_dir``, and a function
    ``env(summary, rc=0)`` that builds the env dict for a given canned
    show response.
    """
    # Write the mock CLI script
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    cli_script = bin_dir / "agent-sec-cli"
    cli_script.write_text(_MOCK_CLI_SCRIPT)
    cli_script.chmod(cli_script.stat().st_mode | stat.S_IEXEC)

    # Create skill directory
    project = tmp_path / "project"
    project.mkdir()
    _create_skill_dir(str(project), "test-skill")

    # Create fake key files so _ensure_keys() is a no-op
    data_dir = tmp_path / "xdg-data" / "agent-sec" / "skill-ledger"
    data_dir.mkdir(parents=True)
    (data_dir / "key.pub").write_text("fake-pub")
    (data_dir / "key.enc").write_text("fake-enc")

    def _make_env(check_output, *, rc=0):
        """Build env override dict for a given canned CLI response."""
        return {
            "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""),
            "XDG_DATA_HOME": str(tmp_path / "xdg-data"),
            "_MOCK_CHECK_OUTPUT": check_output,
            "_MOCK_CHECK_RC": str(rc),
        }

    return {
        "cwd": str(project),
        "make_env": _make_env,
    }


class TestOutputMapping:
    """Verify exposure summary message → decision/reason mapping."""

    def test_default_policy_is_ask(self):
        assert skill_ledger_hook._DEFAULT_POLICY == "ask"

    def test_warn_null_message_returns_silent_allow(self, mock_cli_env):
        """latestStatus=warn with message=null → allow with NO reason field."""
        env = mock_cli_env["make_env"](
            json.dumps(
                {
                    "latestStatus": "warn",
                    "message": None,
                }
            )
        )
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}

    def test_nonempty_message_requires_confirmation(self, mock_cli_env):
        """message text → ask with the shared Skill Ledger message."""
        env = mock_cli_env["make_env"](
            json.dumps(
                {
                    "latestStatus": "deny",
                    "reasonCode": "latest_risk_fallback_to_previous",
                    "message": "Latest skill status is deny; active version is v000001.",
                }
            )
        )
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "ask"
        assert "Latest skill status is deny" in output["reason"]
        assert "test-skill" in output["reason"]

    def test_debug_policy_logs_message_without_prompt(self, mock_cli_env):
        """SKILL_LEDGER_HOOK_POLICY=debug keeps message-based prompts silent."""
        env = mock_cli_env["make_env"](
            json.dumps(
                {
                    "latestStatus": "deny",
                    "message": "Latest skill status is deny; active version is hidden.",
                }
            )
        )
        env["SKILL_LEDGER_HOOK_POLICY"] = "debug"
        output, stderr = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
            return_stderr=True,
        )
        assert output == {"decision": "allow"}
        assert "Latest skill status is deny" in stderr

    def test_warn_policy_returns_allow_with_reason(self, mock_cli_env):
        """SKILL_LEDGER_HOOK_POLICY=warn turns message into allow + reason."""
        env = mock_cli_env["make_env"](
            json.dumps(
                {
                    "latestStatus": "deny",
                    "message": "Latest skill status is deny; active version is hidden.",
                }
            )
        )
        env["SKILL_LEDGER_HOOK_POLICY"] = "warn"
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "Latest skill status is deny" in output["reason"]

    def test_block_policy_returns_block_with_reason(self, mock_cli_env):
        """SKILL_LEDGER_HOOK_POLICY=block rejects execution with the summary message."""
        env = mock_cli_env["make_env"](
            json.dumps(
                {
                    "latestStatus": "deny",
                    "message": "Latest skill status is deny; active version is hidden.",
                }
            )
        )
        env["SKILL_LEDGER_HOOK_POLICY"] = "block"
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "block"
        assert "Latest skill status is deny" in output["reason"]

    def test_missing_message_field_allows(self, mock_cli_env):
        """CLI returns JSON without message → fail-open silent allow."""
        env = mock_cli_env["make_env"](json.dumps({"latestStatus": "deny"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}

    def test_cli_invalid_json_stdout_allows(self, mock_cli_env):
        """CLI returns non-JSON stdout → fail-open."""
        env = mock_cli_env["make_env"]("not-json-at-all")
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}

    def test_cli_empty_stdout_allows(self, mock_cli_env):
        """CLI returns empty stdout → fail-open."""
        env = mock_cli_env["make_env"]("")
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}

    def test_empty_message_field_allows(self, mock_cli_env):
        """CLI returns empty message → silent allow."""
        env = mock_cli_env["make_env"](json.dumps({"message": ""}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}
