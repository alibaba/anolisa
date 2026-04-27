"""Unit tests for cosh-extension/hooks/skill_ledger_hook.py.

The hook is self-contained (no agent_sec_cli imports), so we test it
by piping JSON via subprocess — identical to the code_scanner_hook tests.

Tests are grouped into three categories:

1. **Fail-open paths** — invalid input, wrong tool, missing skill dir.
   These never invoke the CLI and verify the hook always returns allow.
2. **Skill directory resolution** — project-level lookup, missing SKILL.md.
3. **Output mapping** — status → warning message formatting.
   Uses a mock CLI script to return canned check results, verifying the
   hook's decision/reason output for every known status.
"""

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


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _run_hook(input_data, *, env_override=None):
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
        text=True,
        timeout=15,
        env=env,
    )
    assert proc.returncode == 0, f"Hook stderr: {proc.stderr}"
    return json.loads(proc.stdout)


def _make_skill_event(skill_name, cwd="."):
    """Build a minimal PreToolUse event for the skill tool."""
    return {
        "hook_event_name": "PreToolUse",
        "tool_name": "skill",
        "tool_input": {"skill": skill_name},
        "cwd": cwd,
    }


def _create_skill_dir(parent, name="test-skill"):
    """Create a minimal skill directory with a SKILL.md file.

    Returns the absolute path to ``<parent>/.copilot-shell/skills/<name>/``.
    """
    skill_dir = Path(parent) / ".copilot-shell" / "skills" / name
    skill_dir.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: test-skill\ndescription: A test skill\n---\nHello\n"
    )
    return str(skill_dir)


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
        output = _run_hook({"tool_name": "skill", "tool_input": {}})
        assert output["decision"] == "allow"
        assert "empty or missing" in output["reason"]

    def test_empty_skill_name_allows(self):
        output = _run_hook({"tool_name": "skill", "tool_input": {"skill": ""}})
        assert output["decision"] == "allow"
        assert "empty or missing" in output["reason"]

    def test_whitespace_skill_name_allows(self):
        output = _run_hook({"tool_name": "skill", "tool_input": {"skill": "   "}})
        assert output["decision"] == "allow"
        assert "empty or missing" in output["reason"]

    def test_nonstring_skill_name_allows(self):
        output = _run_hook({"tool_name": "skill", "tool_input": {"skill": 42}})
        assert output["decision"] == "allow"
        assert "must be a string" in output["reason"]

    def test_skill_dir_not_found_allows(self):
        """Skill name that resolves to no on-disk directory → fail-open."""
        output = _run_hook(_make_skill_event("nonexistent-skill-xyz", "/tmp"))
        assert output["decision"] == "allow"
        assert "not found on disk" in output["reason"]
        assert "nonexistent-skill-xyz" in output["reason"]

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

        output = _run_hook(_make_skill_event("../evil", str(project)))
        assert output["decision"] == "allow"
        assert "path traversal" in output["reason"]
        assert "../evil" in output["reason"]


# ---------------------------------------------------------------------------
# Skill directory resolution tests
# ---------------------------------------------------------------------------


class TestSkillDirResolution:
    """Verify the hook correctly locates skill directories."""

    def test_project_level_skill_found(self, mock_cli_env):
        """Skill in <cwd>/.copilot-shell/skills/<name>/ should be found.

        We verify by feeding a mock CLI that returns ``{"status": "warn"}``.
        If the skill dir is found the hook calls the CLI and produces a
        ``reason`` field; if the skill dir were *not* found, the hook would
        return plain allow with no ``reason`` at all.
        """
        env = mock_cli_env["make_env"](json.dumps({"status": "warn"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "reason" in output, "Skill dir not found — CLI was never called"

    def test_missing_skill_md_not_found(self):
        """Directory exists but no SKILL.md → not recognized as a skill."""
        with tempfile.TemporaryDirectory() as tmpdir:
            skill_dir = Path(tmpdir) / ".copilot-shell" / "skills" / "bad"
            skill_dir.mkdir(parents=True)
            # No SKILL.md file
            output = _run_hook(_make_skill_event("bad", tmpdir))
            assert output["decision"] == "allow"
            assert "not found on disk" in output["reason"]
            assert "bad" in output["reason"]


# A tiny script that pretends to be agent-sec-cli.
# It reads _MOCK_CHECK_OUTPUT env var and prints it to stdout.
# For "init-keys", it's a no-op.
_MOCK_CLI_SCRIPT = f"#!{sys.executable}\n" + textwrap.dedent("""\
    import os, sys
    # init-keys → silent success
    if len(sys.argv) >= 3 and sys.argv[2] == "init-keys":
        sys.exit(0)
    # check → return canned output from env
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
    ``env(status, rc=0)`` that builds the env dict for a given canned
    check response.
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
    """Verify status → decision/reason mapping for every known status."""

    def test_pass_returns_silent_allow(self, mock_cli_env):
        """status=pass → allow with NO reason field."""
        env = mock_cli_env["make_env"](json.dumps({"status": "pass"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output == {"decision": "allow"}

    def test_none_returns_warning(self, mock_cli_env):
        """status=none → allow + 'not been security-scanned'."""
        env = mock_cli_env["make_env"](json.dumps({"status": "none"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "not been security-scanned" in output["reason"]
        assert "test-skill" in output["reason"]

    def test_warn_returns_warning(self, mock_cli_env):
        """status=warn → allow + 'low-risk findings'."""
        env = mock_cli_env["make_env"](json.dumps({"status": "warn"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "low-risk" in output["reason"]

    def test_deny_returns_warning(self, mock_cli_env):
        """status=deny → allow + 'high-risk findings'."""
        env = mock_cli_env["make_env"](json.dumps({"status": "deny"}), rc=1)
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "high-risk" in output["reason"]

    def test_drifted_returns_warning(self, mock_cli_env):
        """status=drifted → allow + 'content has changed'."""
        env = mock_cli_env["make_env"](json.dumps({"status": "drifted"}), rc=1)
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "changed" in output["reason"]

    def test_tampered_returns_warning(self, mock_cli_env):
        """status=tampered → allow + 'signature verification failed'."""
        env = mock_cli_env["make_env"](json.dumps({"status": "tampered"}), rc=1)
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "signature verification failed" in output["reason"]

    def test_unknown_status_returns_warning(self, mock_cli_env):
        """Unrecognized status → allow + generic warning with status name."""
        env = mock_cli_env["make_env"](json.dumps({"status": "banana"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "banana" in output["reason"]
        assert "unknown status" in output["reason"]

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

    def test_cli_missing_status_field_returns_unknown(self, mock_cli_env):
        """CLI returns JSON without 'status' → treated as unknown status."""
        env = mock_cli_env["make_env"](json.dumps({"result": "ok"}))
        output = _run_hook(
            _make_skill_event("test-skill", mock_cli_env["cwd"]),
            env_override=env,
        )
        assert output["decision"] == "allow"
        assert "unknown" in output["reason"]
