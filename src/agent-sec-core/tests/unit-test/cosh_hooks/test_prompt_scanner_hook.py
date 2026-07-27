"""Unit tests for cosh-extension/hooks/prompt_scanner_hook.py.

The hook is self-contained (no agent_sec_cli imports), so we test it
by importing helpers directly and piping JSON via subprocess for
integration-style tests.

Tests cover:
1. verdict → decision mapping (pass, warn, deny, error, unknown)
2. Error verdict fails open
3. Subprocess integration: pipe JSON into the hook and verify stdout
"""

import io
import json
import subprocess
import sys
from pathlib import Path

from standalone_hook_test_loader import load_standalone_hook

# Path to the standalone cosh hook script
_COSH_HOOK = str(
    Path(__file__).resolve().parents[2]
    / ".."
    / "cosh-extension"
    / "hooks"
    / "prompt_scanner_hook.py"
)

# Import helpers for direct unit testing
prompt_scanner_hook = load_standalone_hook(
    "cosh_prompt_scanner_hook",
    Path(_COSH_HOOK),
)
_format_cosh = prompt_scanner_hook._format_cosh

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
    """verdict=error → fail-open allow."""

    def test_error_returns_allow(self):
        result = json.loads(
            _format_cosh(
                {
                    "verdict": "error",
                    "summary": "internal scanner failure",
                }
            )
        )
        assert result["decision"] == "allow"

    def test_error_with_empty_summary_returns_allow(self):
        result = json.loads(_format_cosh({"verdict": "error"}))
        assert result["decision"] == "allow"


class TestFormatCoshUnknown:
    """Unknown verdict → fail-open allow."""

    def test_unknown_verdict_returns_allow(self):
        result = json.loads(_format_cosh({"verdict": "unknown"}))
        assert result["decision"] == "allow"

    def test_missing_verdict_defaults_to_allow(self):
        """When verdict key is missing, default is 'pass' → allow."""
        result = json.loads(_format_cosh({}))
        assert result["decision"] == "allow"


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
                "agent_name": "cosh",
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
