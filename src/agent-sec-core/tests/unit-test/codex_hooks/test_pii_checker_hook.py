"""Unit tests for codex-plugin/hooks/pii_checker_hook.py.

Coverage targets:
  - Triple hook point routing (UserPromptSubmit, PreToolUse & PostToolUse)
  - Text extraction from different event types
  - Fail-open paths (invalid JSON, empty text, subprocess errors)
  - Mode-based decisions (observe vs deny)
  - Output formatting (_format_block_reason)
  - Evidence sanitization (no raw PII in output)
  - Trace context injection
"""

import importlib.util
import io
import json
import os
import stat
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Hook path & module import
# ---------------------------------------------------------------------------

_HOOKS_DIR = str(
    Path(__file__).resolve().parents[2]
    / ".."
    / "codex-plugin"
    / "hooks-plugin"
    / "hooks"
)
if _HOOKS_DIR not in sys.path:
    sys.path.insert(0, _HOOKS_DIR)

# Temporarily register codex's trace_context so the hook's internal
# "from trace_context import ..." resolves to the codex version,
# not cosh-extension's same-named module that may already be cached.
_saved_tc = sys.modules.pop("trace_context", None)
_tc_spec = importlib.util.spec_from_file_location(
    "trace_context", os.path.join(_HOOKS_DIR, "trace_context.py")
)
_tc_mod = importlib.util.module_from_spec(_tc_spec)
sys.modules["trace_context"] = _tc_mod
_tc_spec.loader.exec_module(_tc_mod)

# Register hook under a unique sys.modules key to avoid collision.
_spec = importlib.util.spec_from_file_location(
    "codex_pii_checker_hook",
    os.path.join(_HOOKS_DIR, "pii_checker_hook.py"),
)
pii_checker_hook = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = pii_checker_hook
_spec.loader.exec_module(pii_checker_hook)

# Restore original trace_context to avoid polluting other test modules.
if _saved_tc is not None:
    sys.modules["trace_context"] = _saved_tc
else:
    sys.modules.pop("trace_context", None)

_HOOK_SCRIPT = os.path.join(_HOOKS_DIR, "pii_checker_hook.py")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _run_hook(input_data, *, env_override=None):
    """Run pii_checker_hook.py as subprocess and return parsed JSON output."""
    env = os.environ.copy()
    if env_override:
        env.update(env_override)
    stdin_text = json.dumps(input_data) if isinstance(input_data, dict) else input_data
    proc = subprocess.run(
        [sys.executable, _HOOK_SCRIPT],
        input=stdin_text,
        capture_output=True,
        check=False,
        text=True,
        timeout=15,
        env=env,
    )
    assert proc.returncode == 0, f"Hook crashed: stderr={proc.stderr}"
    if not proc.stdout.strip():
        return {}
    return json.loads(proc.stdout)


_MOCK_CLI_SCRIPT = f"#!{sys.executable}\n" + textwrap.dedent("""\
    import os, sys
    output = os.environ.get("_MOCK_CLI_OUTPUT", "")
    rc = int(os.environ.get("_MOCK_CLI_RC", "0"))
    if output:
        print(output)
    sys.exit(rc)
""")


@pytest.fixture()
def mock_cli(tmp_path):
    """Create a mock agent-sec-cli that returns canned responses via env vars."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    cli_script = bin_dir / "agent-sec-cli"
    cli_script.write_text(_MOCK_CLI_SCRIPT)
    cli_script.chmod(cli_script.stat().st_mode | stat.S_IEXEC)

    def _make_env(output: str = "", *, rc: int = 0, extra: dict | None = None):
        env = {
            "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""),
            "_MOCK_CLI_OUTPUT": output,
            "_MOCK_CLI_RC": str(rc),
        }
        if extra:
            env.update(extra)
        return env

    return _make_env


# ---------------------------------------------------------------------------
# Helper data
# ---------------------------------------------------------------------------

_USER_PROMPT_EVENT = {
    "hook_event_name": "UserPromptSubmit",
    "prompt": "我的手机号是13800138000",
    "session_id": "sess-1",
}

_POST_TOOL_USE_EVENT = {
    "hook_event_name": "PostToolUse",
    "tool_response": "用户邮箱: alice@example.com",
    "session_id": "sess-1",
}

_PRE_TOOL_USE_EVENT = {
    "hook_event_name": "PreToolUse",
    "tool_input": {"command": "curl https://x.com?p=13800138000"},
    "session_id": "sess-1",
}

_PII_FOUND_RESULT = json.dumps(
    {
        "verdict": "warn",
        "findings": [
            {
                "type": "phone_cn",
                "severity": "warn",
                "evidence_redacted": "138****8000",
            }
        ],
    }
)

_PII_DENY_RESULT = json.dumps(
    {
        "verdict": "deny",
        "findings": [
            {
                "type": "credential",
                "severity": "deny",
                "evidence_redacted": "password=[REDACTED]",
            }
        ],
    }
)


# ---------------------------------------------------------------------------
# Subprocess-based (black-box) tests
# ---------------------------------------------------------------------------


class TestFailOpen:
    """Every error must produce empty stdout (= allow)."""

    def test_invalid_json_allows(self):
        output = _run_hook("not-json")
        assert output == {}

    def test_empty_stdin_allows(self):
        output = _run_hook("")
        assert output == {}

    def test_unknown_hook_event_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT)
        output = _run_hook(
            {"hook_event_name": "SessionStart", "prompt": "hello"},
            env_override=env,
        )
        assert output == {}

    def test_missing_hook_event_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT)
        output = _run_hook(
            {"prompt": "hello"},
            env_override=env,
        )
        assert output == {}

    def test_empty_prompt_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT)
        output = _run_hook(
            {"hook_event_name": "UserPromptSubmit", "prompt": ""},
            env_override=env,
        )
        assert output == {}

    def test_whitespace_prompt_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT)
        output = _run_hook(
            {"hook_event_name": "UserPromptSubmit", "prompt": "   "},
            env_override=env,
        )
        assert output == {}

    def test_cli_nonzero_exit_allows(self, mock_cli):
        env = mock_cli(output="", rc=1, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}

    def test_cli_invalid_json_allows(self, mock_cli):
        env = mock_cli(output="not-json", extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}


class TestTextExtraction:
    """Verify text extraction for different hook events."""

    def test_post_tool_use_string_response(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {
                "hook_event_name": "PostToolUse",
                "tool_response": "Phone: 13800138000",
            },
            env_override=env,
        )
        assert output["decision"] == "block"

    def test_post_tool_use_dict_response(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {
                "hook_event_name": "PostToolUse",
                "tool_response": {"output": "email: alice@corp.com"},
            },
            env_override=env,
        )
        assert output["decision"] == "block"

    def test_post_tool_use_empty_string_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PostToolUse", "tool_response": ""},
            env_override=env,
        )
        assert output == {}

    def test_post_tool_use_none_response_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PostToolUse"},
            env_override=env,
        )
        assert output == {}

    def test_pre_tool_use_string_input(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {
                "hook_event_name": "PreToolUse",
                "tool_input": "curl https://x.com?p=13800138000",
            },
            env_override=env,
        )
        assert output["decision"] == "block"

    def test_pre_tool_use_dict_input(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {
                "hook_event_name": "PreToolUse",
                "tool_input": {"command": "curl https://x.com?p=13800138000"},
            },
            env_override=env,
        )
        assert output["decision"] == "block"

    def test_pre_tool_use_empty_string_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PreToolUse", "tool_input": ""},
            env_override=env,
        )
        assert output == {}

    def test_pre_tool_use_none_input_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PreToolUse"},
            env_override=env,
        )
        assert output == {}

    def test_pre_tool_use_empty_dict_allows(self, mock_cli):
        # Empty dict serializes to "{}" (non-empty string) but has no PII;
        # the hook must short-circuit and NOT call scan-pii. If it did scan,
        # the mock would return PII and deny mode would block.
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PreToolUse", "tool_input": {}},
            env_override=env,
        )
        assert output == {}

    def test_pre_tool_use_empty_list_allows(self, mock_cli):
        # Empty list serializes to "[]" — same short-circuit as empty dict.
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(
            {"hook_event_name": "PreToolUse", "tool_input": []},
            env_override=env,
        )
        assert output == {}


class TestObserveMode:
    """In observe mode, PII is detected but not blocked."""

    def test_pii_in_prompt_not_blocked(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "observe"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}

    def test_pii_in_tool_output_not_blocked(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "observe"})
        output = _run_hook(_POST_TOOL_USE_EVENT, env_override=env)
        assert output == {}

    def test_pii_in_tool_input_not_blocked(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "observe"})
        output = _run_hook(_PRE_TOOL_USE_EVENT, env_override=env)
        assert output == {}


class TestDenyMode:
    """In deny mode, PII triggers block."""

    def test_pass_verdict_allows(self, mock_cli):
        env = mock_cli(
            output=json.dumps({"verdict": "pass", "findings": []}),
            extra={"PII_CHECKER_MODE": "deny"},
        )
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}

    def test_warn_with_no_findings_allows(self, mock_cli):
        env = mock_cli(
            output=json.dumps({"verdict": "warn", "findings": []}),
            extra={"PII_CHECKER_MODE": "deny"},
        )
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}

    def test_warn_verdict_blocks_user_prompt(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output["decision"] == "block"
        assert "phone_cn" in output["reason"]
        assert "138****8000" in output["reason"]
        assert "UserPromptSubmit" in output["reason"]
        assert "请移除敏感信息" in output["reason"]

    def test_warn_verdict_blocks_post_tool_use(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_POST_TOOL_USE_EVENT, env_override=env)
        assert output["decision"] == "block"
        assert "PostToolUse" in output["reason"]
        assert "工具输出已被拦截" in output["reason"]

    def test_deny_verdict_blocks(self, mock_cli):
        env = mock_cli(output=_PII_DENY_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output["decision"] == "block"
        assert "credential" in output["reason"]

    def test_no_raw_pii_in_output(self, mock_cli):
        """Block reason must never contain raw PII content."""
        env = mock_cli(
            output=json.dumps(
                {
                    "verdict": "warn",
                    "findings": [
                        {
                            "type": "phone_cn",
                            "severity": "warn",
                            "evidence_redacted": "138****8000",
                            "raw_evidence": "13800138000",
                        }
                    ],
                }
            ),
            extra={"PII_CHECKER_MODE": "deny"},
        )
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert "13800138000" not in output["reason"]
        assert "138****8000" in output["reason"]

    def test_warn_verdict_blocks_pre_tool_use(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "deny"})
        output = _run_hook(_PRE_TOOL_USE_EVENT, env_override=env)
        assert output["decision"] == "block"
        assert "PreToolUse" in output["reason"]
        assert "该工具调用已被阻止" in output["reason"]


class TestUnknownMode:
    """Unknown mode acts as fail-open."""

    def test_unknown_mode_allows(self, mock_cli):
        env = mock_cli(output=_PII_FOUND_RESULT, extra={"PII_CHECKER_MODE": "banana"})
        output = _run_hook(_USER_PROMPT_EVENT, env_override=env)
        assert output == {}


# ---------------------------------------------------------------------------
# Monkeypatch-based (white-box) tests
# ---------------------------------------------------------------------------


class TestMainMonkeypatch:
    """Direct main() testing with mocked subprocess."""

    def _run_main(self, monkeypatch, capsys, input_data, *, mode="deny"):
        monkeypatch.setattr(pii_checker_hook, "MODE", mode)
        monkeypatch.setattr(
            pii_checker_hook.sys,
            "stdin",
            io.StringIO(
                json.dumps(input_data) if isinstance(input_data, dict) else input_data
            ),
        )
        pii_checker_hook.main()
        out = capsys.readouterr().out
        return json.loads(out) if out.strip() else {}

    def test_subprocess_exception_allows(self, monkeypatch, capsys):
        def fail_run(*args, **kwargs):
            raise OSError("command not found")

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fail_run)
        output = self._run_main(monkeypatch, capsys, _USER_PROMPT_EVENT)
        assert output == {}

    def test_trace_context_injected_for_user_prompt(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "UserPromptSubmit",
                "prompt": "hello world",
                "trace_id": "t1",
                "session_id": "s1",
            },
        )
        assert "--trace-context" in captured["args"]
        assert "--source" in captured["args"]
        source_idx = captured["args"].index("--source")
        assert captured["args"][source_idx + 1] == "user_input"

    def test_trace_context_injected_for_post_tool_use(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "PostToolUse",
                "tool_response": "output data",
                "trace_id": "t1",
            },
        )
        source_idx = captured["args"].index("--source")
        assert captured["args"][source_idx + 1] == "tool_output"

    def test_trace_context_injected_for_pre_tool_use(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            captured["input"] = kwargs.get("input")
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "PreToolUse",
                "tool_input": {"command": "curl https://x.com?p=13800138000"},
                "trace_id": "t1",
            },
        )
        source_idx = captured["args"].index("--source")
        assert captured["args"][source_idx + 1] == "tool_input"
        # dict tool_input is serialized to JSON before scanning
        assert "command" in captured["input"]
        assert "13800138000" in captured["input"]

    def test_deny_mode_blocks_pre_tool_use(self, monkeypatch, capsys):
        """deny mode + PreToolUse PII → block with tool input message."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps(
                    {
                        "verdict": "deny",
                        "findings": [
                            {
                                "type": "phone_cn",
                                "severity": "deny",
                                "evidence_redacted": "138****",
                            },
                        ],
                    }
                ),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "PreToolUse",
                "tool_input": {"command": "curl x?p=13800138000"},
            },
            mode="deny",
        )
        assert output["decision"] == "block"
        assert "工具输入" in output["reason"]
        assert "该工具调用已被阻止" in output["reason"]

    def test_scan_text_passed_via_stdin(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["input"] = kwargs.get("input")
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "UserPromptSubmit",
                "prompt": "my phone 13800138000",
            },
        )
        assert captured["input"] == "my phone 13800138000"

    def test_deny_mode_blocks_with_findings(self, monkeypatch, capsys):
        """deny mode + PII findings → block."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps(
                    {
                        "verdict": "warn",
                        "findings": [
                            {
                                "type": "phone_cn",
                                "severity": "warn",
                                "evidence_redacted": "138****",
                            },
                        ],
                    }
                ),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UserPromptSubmit", "prompt": "my phone 13800138000"},
            mode="deny",
        )
        assert output["decision"] == "block"
        assert "phone_cn" in output["reason"]

    def test_deny_mode_blocks_post_tool_use(self, monkeypatch, capsys):
        """deny mode + PostToolUse PII → block with tool output message."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps(
                    {
                        "verdict": "deny",
                        "findings": [
                            {
                                "type": "email",
                                "severity": "deny",
                                "evidence_redacted": "a***@x.com",
                            },
                        ],
                    }
                ),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {
                "hook_event_name": "PostToolUse",
                "tool_response": "email is alice@example.com",
            },
            mode="deny",
        )
        assert output["decision"] == "block"
        assert "工具输出" in output["reason"]

    def test_observe_mode_allows_findings(self, monkeypatch, capsys):
        """observe mode + findings → allow."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps(
                    {
                        "verdict": "warn",
                        "findings": [{"type": "phone_cn", "severity": "warn"}],
                    }
                ),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UserPromptSubmit", "prompt": "13800138000"},
            mode="observe",
        )
        assert output == {}

    def test_nonzero_returncode_allows(self, monkeypatch, capsys):
        """CLI error → fail-open."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args, returncode=1, stdout="", stderr="error"
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UserPromptSubmit", "prompt": "13800138000"},
        )
        assert output == {}

    def test_invalid_json_stdout_allows(self, monkeypatch, capsys):
        """Invalid JSON from CLI → fail-open."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args, returncode=0, stdout="not-json", stderr=""
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UserPromptSubmit", "prompt": "hello"},
        )
        assert output == {}

    def test_invalid_stdin_allows(self, monkeypatch, capsys):
        """Invalid JSON stdin → fail-open."""
        output = self._run_main(monkeypatch, capsys, "{{not valid")
        assert output == {}

    def test_unknown_hook_event_allows(self, monkeypatch, capsys):
        """Unknown hook event → fail-open."""
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UnknownEvent", "prompt": "hello"},
        )
        assert output == {}

    def test_post_tool_use_dict_response(self, monkeypatch, capsys):
        """PostToolUse with dict tool_response → serialized for scan."""
        captured = {}

        def fake_run(args, **kwargs):
            captured["input"] = kwargs.get("input")
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "PostToolUse", "tool_response": {"data": "value"}},
        )
        # Should be JSON-serialized
        assert "data" in captured["input"]
        assert "value" in captured["input"]

    def test_post_tool_use_empty_string_allows(self, monkeypatch, capsys):
        """PostToolUse with empty string response → nothing to scan."""
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "PostToolUse", "tool_response": "  "},
        )
        assert output == {}

    def test_post_tool_use_none_response_allows(self, monkeypatch, capsys):
        """PostToolUse with null response → nothing to scan."""
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "PostToolUse", "tool_response": None},
        )
        assert output == {}

    def test_pass_verdict_allows(self, monkeypatch, capsys):
        """verdict=pass with empty findings → allow."""

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass", "findings": []}),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)
        output = self._run_main(
            monkeypatch,
            capsys,
            {"hook_event_name": "UserPromptSubmit", "prompt": "hello world"},
        )
        assert output == {}


# ---------------------------------------------------------------------------
# Unit tests for helper functions
# ---------------------------------------------------------------------------


class TestHelpers:
    """Test internal helper functions."""

    def test_as_list_with_list(self):
        assert pii_checker_hook._as_list([1, 2]) == [1, 2]

    def test_as_list_with_non_list(self):
        assert pii_checker_hook._as_list("hello") == []
        assert pii_checker_hook._as_list(None) == []

    def test_safe_text_with_string(self):
        assert pii_checker_hook._safe_text("hello") == "hello"

    def test_safe_text_with_non_string(self):
        assert pii_checker_hook._safe_text(None) == ""
        assert pii_checker_hook._safe_text(123) == ""

    def test_shorten_within_limit(self):
        assert pii_checker_hook._shorten("short", 80) == "short"

    def test_shorten_over_limit(self):
        long_text = "a" * 100
        result = pii_checker_hook._shorten(long_text, 10)
        assert len(result) == 10
        assert result.endswith("…")

    def test_shorten_collapses_whitespace(self):
        assert pii_checker_hook._shorten("hello   world") == "hello world"


class TestFormatBlockReason:
    """Test _format_block_reason output formatting."""

    def test_includes_count_and_types(self):
        findings = [
            {"type": "phone_cn", "severity": "warn", "evidence_redacted": "138****"},
            {"type": "email", "severity": "warn", "evidence_redacted": "a***@x.com"},
        ]
        reason = pii_checker_hook._format_block_reason(
            findings, "UserPromptSubmit", "用户输入"
        )
        assert "2 项" in reason
        assert "email" in reason
        assert "phone_cn" in reason
        assert "UserPromptSubmit" in reason

    def test_evidence_limited_to_max(self):
        findings = [{"type": f"t{i}", "evidence_redacted": f"ev{i}"} for i in range(10)]
        reason = pii_checker_hook._format_block_reason(
            findings, "PostToolUse", "工具输出"
        )
        # Should only include _MAX_EVIDENCE_ITEMS
        assert reason.count("ev") <= pii_checker_hook._MAX_EVIDENCE_ITEMS + 1

    def test_post_tool_use_message(self):
        findings = [{"type": "phone_cn", "severity": "warn"}]
        reason = pii_checker_hook._format_block_reason(
            findings, "PostToolUse", "工具输出"
        )
        assert "工具输出已被拦截" in reason

    def test_pre_tool_use_message(self):
        findings = [{"type": "phone_cn", "severity": "warn"}]
        reason = pii_checker_hook._format_block_reason(
            findings, "PreToolUse", "工具输入"
        )
        assert "该工具调用已被阻止" in reason
        assert "PreToolUse" in reason

    def test_user_prompt_submit_message(self):
        findings = [{"type": "email", "severity": "warn"}]
        reason = pii_checker_hook._format_block_reason(
            findings, "UserPromptSubmit", "用户输入"
        )
        assert "请移除敏感信息" in reason
