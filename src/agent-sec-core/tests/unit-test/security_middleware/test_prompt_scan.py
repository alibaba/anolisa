"""Unit tests for the prompt_scan security-middleware backend."""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.prompt_scanner.result import Verdict
from agent_sec_cli.security_middleware.backends.prompt_scan import (
    PromptScanBackend,
)
from agent_sec_cli.security_middleware.context import RequestContext


class TestModelValidation(unittest.TestCase):
    """Model-handling behaviour for the prompt_scan backend."""

    def setUp(self) -> None:
        self.backend = PromptScanBackend()
        self.ctx = RequestContext(action="prompt_scan")

    def test_unsupported_model_rejected(self) -> None:
        # The backend does not run an entry-point allow-list; an unknown
        # model name falls through to model loading, which reports it as
        # missing locally.  The backend must still surface this as a failed
        # result rather than propagating the exception.
        result = self.backend.execute(self.ctx, text="hello", model="bad/model")
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 1)
        self.assertIn("bad/model", result.error)

    def test_supported_model_accepted(self) -> None:
        # fast mode runs L1 only, so this scan completes without ML deps;
        # the point is that a valid model passes entry validation.
        result = self.backend.execute(
            self.ctx, text="hello", mode="fast", model="qwen3guard:0.6b"
        )
        self.assertTrue(result.success)

    def test_model_override_applied_for_standard_mode(self) -> None:
        scan_result = MagicMock()
        scan_result.verdict = Verdict.PASS
        scan_result.to_dict.return_value = {"verdict": "pass"}
        with patch(
            "agent_sec_cli.security_middleware.backends.prompt_scan.PromptScanner"
        ) as mock_scanner_cls:
            mock_scanner_cls.return_value.scan.return_value = scan_result
            result = self.backend.execute(
                self.ctx, text="hello", mode="standard", model="qwen3guard:0.6b"
            )
        self.assertTrue(result.success)
        config = mock_scanner_cls.call_args.kwargs["config"]
        self.assertEqual(config.model_name, "qwen3guard:0.6b")


if __name__ == "__main__":
    unittest.main()
