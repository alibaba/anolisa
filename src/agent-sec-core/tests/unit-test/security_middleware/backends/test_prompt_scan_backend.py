"""Unit tests for security_middleware.backends.prompt_scan."""

import json
import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.prompt_scanner.config import ScanMode
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ScanResult,
    ThreatDetail,
    ThreatType,
    Verdict,
)
from agent_sec_cli.security_middleware.backends.prompt_scan import (
    PromptScanBackend,
)
from agent_sec_cli.security_middleware.context import RequestContext

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_scan_result(
    verdict: Verdict = Verdict.PASS,
    is_threat: bool = False,
    threat_type: ThreatType = ThreatType.BENIGN,
    layer_results: list | None = None,
    latency_ms: float = 5.0,
) -> ScanResult:
    return ScanResult(
        verdict=verdict,
        is_threat=is_threat,
        threat_type=threat_type,
        layer_results=layer_results or [],
        latency_ms=latency_ms,
    )


def _make_layer_result(
    layer_name: str = "rule_engine",
    detected: bool = False,
    score: float = 0.1,
    details: list | None = None,
) -> LayerResult:
    return LayerResult(
        layer_name=layer_name,
        detected=detected,
        score=score,
        details=details or [],
    )


def _make_threat_detail(
    rule_id: str = "INJ-001",
    description: str = "Injection attempt",
    matched_text: str = "ignore previous instructions",
    category: str = "direct_injection",
) -> ThreatDetail:
    return ThreatDetail(
        rule_id=rule_id,
        description=description,
        matched_text=matched_text,
        category=category,
    )


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestPromptScanBackendInit(unittest.TestCase):
    """Verify that the backend initialises correctly."""

    def test_backend_is_instantiable(self):
        backend = PromptScanBackend()
        self.assertIsNotNone(backend)

    def test_backend_is_base_backend_subclass(self):
        from agent_sec_cli.security_middleware.backends.base import BaseBackend

        backend = PromptScanBackend()
        self.assertIsInstance(backend, BaseBackend)


class TestPromptScanBackendEmptyInput(unittest.TestCase):
    """execute() must reject empty / whitespace-only text."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    def test_empty_string_returns_failure(self):
        result = self.backend.execute(self.ctx, text="")
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertIn("no input text provided", result.error)

    def test_whitespace_only_returns_failure(self):
        result = self.backend.execute(self.ctx, text="   \t\n  ")
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertIn("no input text provided", result.error)

    def test_missing_text_kwarg_returns_failure(self):
        result = self.backend.execute(self.ctx)
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)


class TestPromptScanBackendInvalidMode(unittest.TestCase):
    """execute() must reject unknown scan modes."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    def test_invalid_mode_returns_failure(self):
        result = self.backend.execute(self.ctx, text="hello", mode="turbo")
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertIn("invalid mode", result.error)
        self.assertIn("turbo", result.error)

    def test_error_message_mentions_valid_modes(self):
        result = self.backend.execute(self.ctx, text="hello", mode="unknown_mode")
        self.assertIn("fast", result.error)
        self.assertIn("standard", result.error)
        self.assertIn("strict", result.error)


class TestPromptScanBackendScannerCreation(unittest.TestCase):
    """execute() must create a new PromptScanner with the resolved ScanMode."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_scanner_instantiated_with_correct_mode(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="hello", mode="fast")

        MockScanner.assert_called_once_with(mode=ScanMode.FAST)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_each_call_creates_new_scanner(self, MockScanner):
        """Without caching each execute() call creates a fresh PromptScanner."""
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="first call", mode="fast")
        self.backend.execute(self.ctx, text="second call", mode="fast")

        self.assertEqual(MockScanner.call_count, 2)


class TestPromptScanBackendCleanResult(unittest.TestCase):
    """execute() must handle a clean (PASS) scan result correctly."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_pass_verdict_sets_success_true(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Hello, how are you?")

        self.assertTrue(result.success)
        self.assertEqual(result.exit_code, 0)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_pass_verdict_stdout_is_valid_json(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Normal query")

        parsed = json.loads(result.stdout)
        self.assertEqual(parsed["verdict"], "pass")
        self.assertTrue(parsed["ok"])

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_pass_result_data_matches_stdout(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Normal query")

        self.assertEqual(result.data, json.loads(result.stdout))


class TestPromptScanBackendThreatResult(unittest.TestCase):
    """execute() must handle threat (WARN/DENY) scan results correctly."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_warn_verdict_sets_success_true_exit_code_0(self, MockScanner):
        scan_result = _make_scan_result(
            verdict=Verdict.WARN,
            is_threat=True,
            threat_type=ThreatType.DIRECT_INJECTION,
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="ignore previous instructions")

        self.assertTrue(result.success)
        self.assertEqual(result.exit_code, 0)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_deny_verdict_sets_success_true_exit_code_0(self, MockScanner):
        scan_result = _make_scan_result(
            verdict=Verdict.DENY,
            is_threat=True,
            threat_type=ThreatType.JAILBREAK,
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="DAN jailbreak attempt")

        self.assertTrue(result.success)
        self.assertEqual(result.exit_code, 0)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_deny_verdict_stdout_reflects_threat(self, MockScanner):
        layer = _make_layer_result(
            detected=True,
            score=0.95,
            details=[_make_threat_detail()],
        )
        scan_result = _make_scan_result(
            verdict=Verdict.DENY,
            is_threat=True,
            threat_type=ThreatType.DIRECT_INJECTION,
            layer_results=[layer],
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="ignore previous instructions")

        parsed = json.loads(result.stdout)
        self.assertEqual(parsed["verdict"], "deny")
        self.assertFalse(parsed["ok"])
        self.assertGreater(len(parsed["findings"]), 0)


class TestPromptScanBackendErrorVerdict(unittest.TestCase):
    """execute() must report scanner ERROR verdict as failure."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_error_verdict_sets_success_false(self, MockScanner):
        scan_result = _make_scan_result(
            verdict=Verdict.ERROR,
            is_threat=False,
            threat_type=ThreatType.BENIGN,
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Some text")

        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_error_verdict_stdout_is_still_valid_json(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.ERROR)
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Some text")

        parsed = json.loads(result.stdout)
        self.assertEqual(parsed["verdict"], "error")


class TestPromptScanBackendScannerException(unittest.TestCase):
    """execute() must convert scanner exceptions to structured error verdicts."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_scan_exception_returns_error_verdict(self, MockScanner):
        mock_scanner = MagicMock()
        mock_scanner.scan.side_effect = RuntimeError("model exploded")
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="Some text")

        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertEqual(result.error, "Scanner error: model exploded")
        self.assertEqual(result.data["verdict"], "error")
        self.assertEqual(result.data["summary"], "Scanner error: model exploded")

        parsed = json.loads(result.stdout)
        self.assertEqual(parsed["verdict"], "error")
        self.assertEqual(parsed["summary"], "Scanner error: model exploded")


class TestPromptScanBackendModeHandling(unittest.TestCase):
    """execute() must correctly map mode strings to ScanMode enum values."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_fast_mode_creates_fast_scanner(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test", mode="fast")

        MockScanner.assert_called_once_with(mode=ScanMode.FAST)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_standard_mode_is_default(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test")

        MockScanner.assert_called_once_with(mode=ScanMode.STANDARD)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_strict_mode_creates_strict_scanner(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test", mode="strict")

        MockScanner.assert_called_once_with(mode=ScanMode.STRICT)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_mode_string_is_case_insensitive(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test", mode="FAST")

        MockScanner.assert_called_once_with(mode=ScanMode.FAST)


class TestPromptScanBackendSourcePropagation(unittest.TestCase):
    """execute() must forward the source kwarg to scanner.scan()."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_source_is_forwarded_to_scanner(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test text", source="user_input")

        mock_scanner.scan.assert_called_once_with("test text", source="user_input")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_empty_source_is_passed_as_none(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test text", source="")

        mock_scanner.scan.assert_called_once_with("test text", source=None)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_missing_source_defaults_to_none(self, MockScanner):
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(self.ctx, text="test text")

        mock_scanner.scan.assert_called_once_with("test text", source=None)


class TestPromptScanBackendOutputFormat(unittest.TestCase):
    """Verify the stdout JSON output schema matches the expected contract."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_output_contains_required_schema_fields(self, MockScanner):
        layer = _make_layer_result(
            layer_name="rule_engine",
            detected=True,
            score=0.8,
            details=[_make_threat_detail()],
        )
        scan_result = _make_scan_result(
            verdict=Verdict.DENY,
            is_threat=True,
            threat_type=ThreatType.DIRECT_INJECTION,
            layer_results=[layer],
            latency_ms=12.5,
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="inject payload")

        parsed = json.loads(result.stdout)
        required_fields = {
            "schema_version",
            "ok",
            "verdict",
            "risk_level",
            "threat_type",
            "summary",
            "findings",
            "layer_results",
            "engine_version",
            "elapsed_ms",
        }
        for field in required_fields:
            self.assertIn(field, parsed, msg=f"Missing field: {field}")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_output_json_is_pretty_printed(self, MockScanner):
        """stdout must be indented (indent=2) for human readability."""
        scan_result = _make_scan_result()
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="hello")

        self.assertIn("\n", result.stdout)
        self.assertIn("  ", result.stdout)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_findings_populated_for_threat(self, MockScanner):
        detail = _make_threat_detail(
            rule_id="INJ-002",
            description="Indirect injection via tool output",
            matched_text="<tool_output>IGNORE PREVIOUS</tool_output>",
            category="indirect_injection",
        )
        layer = _make_layer_result(
            layer_name="rule_engine", detected=True, score=0.9, details=[detail]
        )
        scan_result = _make_scan_result(
            verdict=Verdict.DENY,
            is_threat=True,
            threat_type=ThreatType.INDIRECT_INJECTION,
            layer_results=[layer],
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="test")

        parsed = json.loads(result.stdout)
        self.assertEqual(len(parsed["findings"]), 1)
        finding = parsed["findings"][0]
        self.assertEqual(finding["rule_id"], "INJ-002")
        self.assertEqual(finding["category"], "indirect_injection")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_confidence_field_present_for_threat(self, MockScanner):
        layer = _make_layer_result(detected=True, score=0.75)
        scan_result = _make_scan_result(
            verdict=Verdict.WARN,
            is_threat=True,
            threat_type=ThreatType.JAILBREAK,
            layer_results=[layer],
        )
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="jailbreak attempt")

        parsed = json.loads(result.stdout)
        self.assertIn("confidence", parsed)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_confidence_field_absent_for_clean_scan(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS, is_threat=False)
        mock_scanner = MagicMock()
        mock_scanner.scan.return_value = scan_result
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(self.ctx, text="hello world")

        parsed = json.loads(result.stdout)
        self.assertNotIn("confidence", parsed)


class TestPromptScanBackendMultiTurn(unittest.TestCase):
    """execute() must handle MULTI_TURN mode by calling scan_multi_turn()."""

    def setUp(self):
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_multi_turn_calls_scan_multi_turn(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan_multi_turn.return_value = scan_result
        MockScanner.return_value = mock_scanner

        history = [{"role": "user", "content": "hello"}]
        result = self.backend.execute(
            self.ctx,
            text="what is 1+1?",
            mode="multi_turn",
            history=history,
            assistant_response="2",
        )

        MockScanner.assert_called_once_with(mode=ScanMode.MULTI_TURN)
        mock_scanner.scan_multi_turn.assert_called_once_with(
            history=history,
            current_query="what is 1+1?",
            assistant_response="2",
            source=None,
        )
        mock_scanner.scan.assert_not_called()
        self.assertTrue(result.success)

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_multi_turn_with_source(self, MockScanner):
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan_multi_turn.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(
            self.ctx,
            text="query",
            mode="multi_turn",
            history=[],
            assistant_response="resp",
            source="cosh_after_model",
        )

        mock_scanner.scan_multi_turn.assert_called_once_with(
            history=[],
            current_query="query",
            assistant_response="resp",
            source="cosh_after_model",
        )

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_multi_turn_defaults_history_and_response(self, MockScanner):
        """Missing history/assistant_response default to empty values."""
        scan_result = _make_scan_result(verdict=Verdict.PASS)
        mock_scanner = MagicMock()
        mock_scanner.scan_multi_turn.return_value = scan_result
        MockScanner.return_value = mock_scanner

        self.backend.execute(
            self.ctx,
            text="query",
            mode="multi_turn",
        )

        mock_scanner.scan_multi_turn.assert_called_once_with(
            history=[],
            current_query="query",
            assistant_response="",
            source=None,
        )

    @patch("agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner")
    def test_multi_turn_exception_returns_error(self, MockScanner):
        mock_scanner = MagicMock()
        mock_scanner.scan_multi_turn.side_effect = RuntimeError("ollama down")
        MockScanner.return_value = mock_scanner

        result = self.backend.execute(
            self.ctx,
            text="query",
            mode="multi_turn",
            history=[],
            assistant_response="",
        )

        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertIn("Scanner error: ollama down", result.error)


if __name__ == "__main__":
    unittest.main()
