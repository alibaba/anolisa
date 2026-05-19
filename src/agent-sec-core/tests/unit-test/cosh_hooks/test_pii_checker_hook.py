"""Unit tests for cosh-extension/hooks/pii_checker_hook.py."""

import io
import json
import subprocess
import sys
from pathlib import Path

import pytest

_COSH_HOOK = str(
    Path(__file__).resolve().parents[2]
    / ".."
    / "cosh-extension"
    / "hooks"
    / "pii_checker_hook.py"
)

sys.path.insert(0, str(Path(_COSH_HOOK).parent))
import pii_checker_hook  # noqa: E402
from pii_checker_hook import _format_cosh  # noqa: E402


class TestFormatCosh:
    def test_pass_returns_allow(self):
        result = json.loads(_format_cosh({"verdict": "pass", "findings": []}))
        assert result == {"decision": "allow"}

    def test_warn_returns_allow_with_reason(self):
        result = json.loads(
            _format_cosh(
                {
                    "verdict": "warn",
                    "findings": [
                        {
                            "type": "email",
                            "severity": "warn",
                            "evidence_redacted": "a***@example.com",
                            "raw_evidence": "alice@example.com",
                        }
                    ],
                }
            )
        )

        assert result["decision"] == "allow"
        assert "[pii-checker]" in result["reason"]
        assert "email" in result["reason"]
        assert "a***@example.com" in result["reason"]
        assert "alice@example.com" not in result["reason"]
        assert "raw_evidence" not in result["reason"]

    def test_deny_returns_allow_with_high_risk_reason(self):
        result = json.loads(
            _format_cosh(
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
        )

        assert result["decision"] == "allow"
        assert "高风险" in result["reason"]
        assert "credential" in result["reason"]

    def test_warn_without_findings_allows(self):
        result = json.loads(_format_cosh({"verdict": "warn", "findings": []}))
        assert result == {"decision": "allow"}

    @pytest.mark.parametrize("verdict", ["error", "unknown", ""])
    def test_error_and_unknown_verdicts_allow(self, verdict):
        result = json.loads(_format_cosh({"verdict": verdict, "findings": [{}]}))
        assert result == {"decision": "allow"}


class TestCoshHookMain:
    def _run_main(self, monkeypatch, capsys, input_data):
        monkeypatch.setattr(pii_checker_hook.sys, "stdin", io.StringIO(input_data))
        pii_checker_hook.main()
        return json.loads(capsys.readouterr().out)

    def test_empty_prompt_allows_without_cli(self, monkeypatch, capsys):
        def fail_run(*args, **kwargs):
            raise AssertionError("CLI should not be called")

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fail_run)

        output = self._run_main(monkeypatch, capsys, '{"prompt": ""}')
        assert output == {"decision": "allow"}

    def test_invalid_json_allows_without_cli(self, monkeypatch, capsys):
        def fail_run(*args, **kwargs):
            raise AssertionError("CLI should not be called")

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fail_run)

        output = self._run_main(monkeypatch, capsys, "not-json")
        assert output == {"decision": "allow"}

    def test_missing_prompt_allows_without_cli(self, monkeypatch, capsys):
        def fail_run(*args, **kwargs):
            raise AssertionError("CLI should not be called")

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fail_run)

        output = self._run_main(monkeypatch, capsys, '{"session_id": "abc"}')
        assert output == {"decision": "allow"}

    def test_calls_scan_pii_with_user_input_source(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            captured["kwargs"] = kwargs
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
                                "evidence_redacted": "138****8000",
                            }
                        ],
                    }
                ),
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)

        output = self._run_main(
            monkeypatch,
            capsys,
            json.dumps({"prompt": "Phone: 13800138000"}),
        )

        assert captured["args"] == [
            "agent-sec-cli",
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "user_input",
        ]
        assert captured["kwargs"]["input"] == "Phone: 13800138000"
        assert captured["kwargs"]["timeout"] == 10
        assert output["decision"] == "allow"
        assert "phone_cn" in output["reason"]

    def test_injects_trace_context_into_scan_pii_command(self, monkeypatch, capsys):
        captured = {}

        def fake_run(args, **kwargs):
            captured["args"] = args
            captured["kwargs"] = kwargs
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
            json.dumps(
                {
                    "prompt": "Phone: 13800138000",
                    "trace_id": "",
                    "traceId": "trace-1",
                    "session_id": "session-1",
                    "sessionId": "wrong-session",
                    "run_id": "run-1",
                    "tool_use_id": "tool-1",
                }
            ),
        )

        expected_context = json.dumps(
            {
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
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "user_input",
        ]
        assert captured["kwargs"]["check"] is False

    def test_cli_nonzero_allows(self, monkeypatch, capsys):
        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=1,
                stdout="",
                stderr="boom",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)

        output = self._run_main(monkeypatch, capsys, '{"prompt": "hello"}')
        assert output == {"decision": "allow"}

    def test_cli_bad_json_allows(self, monkeypatch, capsys):
        def fake_run(args, **kwargs):
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout="not-json",
                stderr="",
            )

        monkeypatch.setattr(pii_checker_hook.subprocess, "run", fake_run)

        output = self._run_main(monkeypatch, capsys, '{"prompt": "hello"}')
        assert output == {"decision": "allow"}


def test_manifest_registers_only_user_prompt_submit_for_pii():
    manifest_path = (
        Path(__file__).resolve().parents[2]
        / ".."
        / "cosh-extension"
        / "cosh-extension.json"
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    pii_locations = []
    for hook_name, groups in manifest["hooks"].items():
        for group in groups:
            for hook in group.get("hooks", []):
                if hook.get("name") == "pii-checker":
                    pii_locations.append(hook_name)

    assert pii_locations == ["UserPromptSubmit"]
