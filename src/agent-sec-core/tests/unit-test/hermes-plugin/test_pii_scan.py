"""Unit tests for hermes-plugin pii_scan capability."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

# Add hermes-plugin/ to sys.path so 'src' is importable as a package
_HERMES_PLUGIN_DIR = Path(__file__).resolve().parents[3] / "hermes-plugin"
sys.path.insert(0, str(_HERMES_PLUGIN_DIR))

from src.capabilities.pii_scan import PiiScanCapability  # noqa: E402
from src.cli_runner import CliResult  # noqa: E402


def _make_capability(
    *,
    include_low_confidence: bool = False,
    warning_ttl_seconds: float = 300,
) -> PiiScanCapability:
    """Create a PiiScanCapability with test config."""
    cap = PiiScanCapability()
    cap._timeout = 5.0
    cap._include_low_confidence = include_low_confidence
    cap._warning_ttl_seconds = warning_ttl_seconds
    return cap


def _scan_result(verdict: str, findings: list[dict] | None = None) -> CliResult:
    """Build a mock scan-pii CLI result."""
    return CliResult(
        stdout=json.dumps({"verdict": verdict, "findings": findings or []}),
        stderr="",
        exit_code=0,
    )


@pytest.fixture
def capability():
    """Create a default PII scan capability."""
    return _make_capability()


class TestPiiScanCapability:
    """Tests for PiiScanCapability hook behavior."""

    def test_registers_expected_hooks(self, capability):
        """Capability should register Hermes input/output lifecycle hooks."""
        hooks = capability.get_hooks_define()

        assert list(hooks) == [
            "pre_llm_call",
            "transform_llm_output",
            "on_session_end",
        ]

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_empty_input_passthrough(self, mock_cli, capability):
        """Empty user input should not call scan-pii."""
        result = capability._on_pre_llm_call(
            user_message="   ",
            session_id="session-1",
        )

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_missing_user_fields_passthrough(self, mock_cli, capability):
        """Missing user text fields should fail open without invoking scan-pii."""
        result = capability._on_pre_llm_call(session_id="session-1")
        transformed = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None
        assert transformed is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_pass_verdict_does_not_transform_output(self, mock_cli, capability):
        """Pass verdict should not cache a warning."""
        mock_cli.return_value = _scan_result("pass")

        pre_result = capability._on_pre_llm_call(
            user_message="hello",
            session_id="session-1",
        )
        transform_result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert pre_result is None
        assert transform_result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_passes_hermes_trace_context_to_cli(self, mock_cli, capability):
        """Hermes session tracing should be propagated to scan-pii."""
        mock_cli.return_value = _scan_result("pass")

        result = capability._on_pre_llm_call(
            user_message="hello",
            session_id="session-1",
        )

        assert result is None
        assert mock_cli.call_args.kwargs["trace_context"] == {"session_id": "session-1"}
        assert "run_id" not in mock_cli.call_args.kwargs["trace_context"]

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_warn_verdict_prepends_warning_once(self, mock_cli, capability):
        """Warn verdict should prepend one redacted warning to final output."""
        mock_cli.return_value = _scan_result(
            "warn",
            [
                {
                    "type": "email",
                    "severity": "warn",
                    "evidence_redacted": "a***@example.com",
                    "raw_evidence": "alice@example.com",
                }
            ],
        )

        capability._on_pre_llm_call(
            user_message="email alice@example.com",
            session_id="session-1",
        )
        first = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )
        second = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert first is not None
        assert first.endswith("\n\nassistant reply")
        assert "[pii-checker]" in first
        assert "敏感信息" in first
        assert "email" in first
        assert "a***@example.com" in first
        assert "alice@example.com" not in first
        assert "raw_evidence" not in first
        assert second is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_deny_verdict_uses_high_risk_warning(self, mock_cli, capability):
        """Deny verdict should still be warning-only but marked high risk."""
        mock_cli.return_value = _scan_result(
            "deny",
            [
                {
                    "type": "generic_secret_field",
                    "severity": "deny",
                    "evidence_redacted": "password=[REDACTED]",
                }
            ],
        )

        capability._on_pre_llm_call(
            user_message="password=super-secret",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is not None
        assert "高风险敏感信息" in result
        assert "password=[REDACTED]" in result
        assert "assistant reply" in result

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_include_low_confidence_adds_cli_arg(self, mock_cli):
        """include_low_confidence should pass through to scan-pii."""
        cap = _make_capability(include_low_confidence=True)
        mock_cli.return_value = _scan_result("pass")

        cap._on_pre_llm_call(user_message="hello", session_id="session-1")

        call_args = mock_cli.call_args[0][0]
        assert call_args == [
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "user_input",
            "--include-low-confidence",
        ]
        assert mock_cli.call_args.kwargs["stdin"] == "hello"

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_extracts_last_user_message_from_messages(self, mock_cli, capability):
        """Fallback should scan only the last user message."""
        mock_cli.return_value = _scan_result("pass")

        capability._on_pre_llm_call(
            messages=[
                {"role": "user", "content": "old email alice@example.com"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": [{"type": "text", "text": "new text"}]},
            ],
            session_id="session-1",
        )

        call_args = mock_cli.call_args[0][0]
        assert call_args == [
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "user_input",
        ]
        assert mock_cli.call_args.kwargs["stdin"] == "new text"

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_missing_cache_key_fails_open(self, mock_cli, capability):
        """Missing session/task/run key should avoid session-level leakage."""
        mock_cli.return_value = _scan_result(
            "warn",
            [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
        )

        result = capability._on_pre_llm_call(user_message="alice@example.com")
        transformed = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None
        assert transformed is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_cli_nonzero_fails_open(self, mock_cli, capability):
        """CLI failure should not change final output."""
        mock_cli.return_value = CliResult(stdout="", stderr="boom", exit_code=1)

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_invalid_json_fails_open(self, mock_cli, capability):
        """Invalid CLI JSON should not change final output."""
        mock_cli.return_value = CliResult(stdout="not-json", stderr="", exit_code=0)

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_unknown_verdict_fails_open(self, mock_cli, capability):
        """Unknown verdicts should not change final output."""
        mock_cli.return_value = _scan_result(
            "maybe",
            [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
        )

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_ttl_expiry_drops_warning(self, mock_cli):
        """Expired warnings should not be delivered."""
        cap = _make_capability(warning_ttl_seconds=0)
        mock_cli.return_value = _scan_result(
            "warn",
            [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
        )

        cap._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        result = cap._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_session_end_clears_warning(self, mock_cli, capability):
        """Session end should drop pending warnings."""
        mock_cli.return_value = _scan_result(
            "warn",
            [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
        )

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        capability._on_session_end(session_id="session-1")
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_next_turn_clears_stale_warning(self, mock_cli, capability):
        """A new pre_llm_call should clear stale warnings for the same session."""
        mock_cli.side_effect = [
            _scan_result(
                "warn",
                [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
            ),
            _scan_result("pass"),
        ]

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        capability._on_pre_llm_call(
            user_message="hello",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is None

    @patch("src.capabilities.pii_scan.call_agent_sec_cli")
    def test_duplicate_warning_is_delivered_once(self, mock_cli, capability):
        """Repeated identical findings in one turn should not duplicate text."""
        mock_cli.return_value = _scan_result(
            "warn",
            [{"type": "email", "severity": "warn", "evidence_redacted": "a***"}],
        )

        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        capability._on_pre_llm_call(
            user_message="alice@example.com",
            session_id="session-1",
        )
        result = capability._on_transform_llm_output(
            "assistant reply",
            session_id="session-1",
        )

        assert result is not None
        assert result.count("[pii-checker]") == 1
