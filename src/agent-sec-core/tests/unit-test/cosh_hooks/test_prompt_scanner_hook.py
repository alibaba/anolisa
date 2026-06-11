"""Unit tests for cosh-extension/hooks/prompt_scanner_hook.py.

The hook is self-contained (no agent_sec_cli imports), so we test it
by importing helpers directly and piping JSON via subprocess for
integration-style tests.

Tests cover:
1. verdict → decision mapping (pass, warn, deny, error, unknown)
2. Error/unknown verdicts fail-ask (not fail-open)
3. Subprocess integration: pipe JSON into the hook and verify stdout
"""

import io
import json
import subprocess
import sys
from pathlib import Path

# Path to the standalone cosh hook script
_COSH_HOOK = str(
    Path(__file__).resolve().parents[2]
    / ".."
    / "cosh-extension"
    / "hooks"
    / "prompt_scanner_hook.py"
)

# Import helpers for direct unit testing
sys.path.insert(0, str(Path(_COSH_HOOK).parent))
import prompt_scanner_hook  # noqa: E402
from prompt_scanner_hook import _format_cosh  # noqa: E402

# ---------------------------------------------------------------------------
# Unit tests: _format_cosh
# ---------------------------------------------------------------------------


class TestFormatCoshPass:
    """verdict=pass → decision=allow."""

    def test_pass_returns_allow(self):
        result = json.loads(_format_cosh({"verdict": "pass"}))
        assert result["decision"] == "allow"

    def test_pass_ignores_summary(self):
        result = json.loads(_format_cosh({"verdict": "pass", "summary": "anything"}))
        assert result["decision"] == "allow"


class TestFormatCoshWarn:
    """verdict=warn → decision=ask with reason."""

    def test_warn_returns_ask(self):
        result = json.loads(
            _format_cosh(
                {"verdict": "warn", "threat_type": "jailbreak", "risk_level": "medium"}
            )
        )
        assert result["decision"] == "ask"
        assert "[prompt-scanner]" in result["reason"]
        assert "攻击类型" in result["reason"]
        assert "jailbreak" in result["reason"]

    def test_warn_uses_threat_type_when_provided(self):
        result = json.loads(
            _format_cosh({"verdict": "warn", "threat_type": "direct_injection"})
        )
        assert result["decision"] == "ask"
        assert "direct_injection" in result["reason"]

    def test_warn_includes_structured_fields(self):
        result = json.loads(_format_cosh({"verdict": "warn", "confidence": 0.85}))
        assert result["decision"] == "ask"
        assert "模型置信度" in result["reason"]
        assert "85.0%" in result["reason"]


class TestFormatCoshDeny:
    """verdict=deny → decision=ask with reason."""

    def test_deny_returns_ask(self):
        result = json.loads(
            _format_cosh(
                {"verdict": "deny", "threat_type": "jailbreak", "risk_level": "high"}
            )
        )
        assert result["decision"] == "ask"
        assert "jailbreak" in result["reason"]
        assert "拦截环节" in result["reason"]


class TestFormatCoshError:
    """verdict=error → fail-ask: surface to user."""

    def test_error_returns_ask(self):
        result = json.loads(
            _format_cosh(
                {
                    "verdict": "error",
                    "summary": "agent-sec daemon is unavailable",
                }
            )
        )
        assert result["decision"] == "ask"
        assert "扫描异常" in result["reason"]
        assert "agent-sec daemon is unavailable" in result["reason"]

    def test_error_with_no_detail_returns_ask(self):
        result = json.loads(_format_cosh({"verdict": "error"}))
        assert result["decision"] == "ask"
        assert "error" in result["reason"]


class TestFormatCoshUnknown:
    """Unknown verdict → fail-ask: surface to user."""

    def test_unknown_verdict_returns_ask(self):
        result = json.loads(_format_cosh({"verdict": "unknown"}))
        assert result["decision"] == "ask"

    def test_missing_verdict_returns_ask(self):
        """When verdict key is missing, fail-ask (not fail-open)."""
        result = json.loads(_format_cosh({}))
        assert result["decision"] == "ask"
        assert "扫描异常" in result["reason"]


# ---------------------------------------------------------------------------
# Integration tests: subprocess (pipe JSON into hook, verify stdout)
# ---------------------------------------------------------------------------


class TestCoshHookSubprocess:
    """Integration tests: pipe JSON into prompt_scanner_hook.py and verify stdout."""

    def _run_hook(self, input_data: dict) -> dict:
        proc = subprocess.run(
            [sys.executable, _COSH_HOOK],
            input=json.dumps(input_data),
            capture_output=True,
            check=False,
            text=True,
            timeout=15,
        )
        # Hook always exits 0
        assert proc.returncode == 0, f"Hook stderr: {proc.stderr}"
        return json.loads(proc.stdout)

    def test_empty_prompt_allows(self):
        output = self._run_hook({"prompt": ""})
        assert output["decision"] == "allow"

    def test_invalid_json_allows(self):
        """Malformed stdin should fail-open with allow."""
        proc = subprocess.run(
            [sys.executable, _COSH_HOOK],
            input="not-json",
            capture_output=True,
            check=False,
            text=True,
            timeout=15,
        )
        assert proc.returncode == 0
        output = json.loads(proc.stdout)
        assert output["decision"] == "allow"

    def test_missing_prompt_key_allows(self):
        output = self._run_hook({"session_id": "abc"})
        assert output["decision"] == "allow"

    def test_injects_trace_context_into_scan_prompt_command(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=json.dumps({"verdict": "pass"}),
                stderr="",
            )

        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        monkeypatch.setattr(
            prompt_scanner_hook.sys,
            "stdin",
            io.StringIO(
                json.dumps(
                    {
                        "prompt": "hello",
                        "session_id": "session-1",
                        "run_id": "run-1",
                        "trace": {"callId": "nested-call-is-not-hook-input"},
                    }
                )
            ),
        )

        prompt_scanner_hook.main()

        output = json.loads(capsys.readouterr().out)
        expected_context = json.dumps(
            {
                "session_id": "session-1",
                "run_id": "run-1",
            },
            ensure_ascii=False,
            separators=(",", ":"),
        )
        assert output == {"decision": "allow"}
        assert captured["args"] == [
            "agent-sec-cli",
            "--trace-context",
            expected_context,
            "scan-prompt",
            "--text",
            "hello",
            "--mode",
            "standard",
            "--format",
            "json",
            "--source",
            "user_input",
        ]
        assert captured["kwargs"]["check"] is False

    def _mock_stdin(self, monkeypatch):
        """Set stdin to a valid prompt so main() reaches the subprocess call."""
        monkeypatch.setattr(
            prompt_scanner_hook.sys,
            "stdin",
            io.StringIO(json.dumps({"prompt": "test prompt"})),
        )

    def test_cli_timeout_returns_ask(self, monkeypatch, capsys):
        self._mock_stdin(monkeypatch)

        def fake_run(args, **kwargs):
            raise subprocess.TimeoutExpired(args, kwargs.get("timeout", 10))

        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        prompt_scanner_hook.main()
        output = json.loads(capsys.readouterr().out)
        assert output["decision"] == "ask"
        assert "超时" in output["reason"]

    def test_cli_exception_returns_ask(self, monkeypatch, capsys):
        self._mock_stdin(monkeypatch)

        def fake_run(args, **kwargs):
            raise OSError("No such file")

        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        prompt_scanner_hook.main()
        output = json.loads(capsys.readouterr().out)
        assert output["decision"] == "ask"
        assert "调用失败" in output["reason"]

    def test_cli_nonzero_exit_returns_ask(self, monkeypatch, capsys):
        self._mock_stdin(monkeypatch)

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args, returncode=1, stdout="", stderr="segfault"
            )

        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        prompt_scanner_hook.main()
        output = json.loads(capsys.readouterr().out)
        assert output["decision"] == "ask"
        assert "异常退出" in output["reason"]

    def test_cli_bad_json_returns_ask(self, monkeypatch, capsys):
        self._mock_stdin(monkeypatch)

        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args, returncode=0, stdout="not-json", stderr=""
            )

        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        prompt_scanner_hook.main()
        output = json.loads(capsys.readouterr().out)
        assert output["decision"] == "ask"
        assert "解析失败" in output["reason"]
