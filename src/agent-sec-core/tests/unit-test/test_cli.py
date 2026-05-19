"""Unit tests for the top-level CLI entry points."""

import unittest
from pathlib import Path
from unittest.mock import patch

from agent_sec_cli.cli import app
from agent_sec_cli.security_middleware.result import ActionResult
from typer.testing import CliRunner


class TestHardenCli(unittest.TestCase):
    def setUp(self):
        self.runner = CliRunner()

    def test_harden_help_shows_concise_summary(self):
        result = self.runner.invoke(app, ["harden", "--help"])

        self.assertEqual(result.exit_code, 0)
        self.assertIn("Usage: agent-sec-cli harden [SEHARDEN_ARGS]...", result.output)
        self.assertIn("Defaults:", result.output)
        self.assertIn("--scan --config agentos_baseline", result.output)
        self.assertIn("Examples:", result.output)
        self.assertIn("Common SEHarden flags:", result.output)
        self.assertIn("--downstream-help", result.output)
        self.assertNotIn(
            "Pass arguments through to `loongshield seharden`.", result.output
        )
        self.assertNotIn("-- --help", result.output)

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_adds_default_scan_and_config_on_zero_args(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(app, ["harden"])

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--config", "agentos_baseline"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_forwards_unknown_args_to_backend(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(
            app,
            ["harden", "--scan", "--config", "agentos_baseline", "--dry-run"],
        )

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--config", "agentos_baseline", "--dry-run"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_adds_default_config_when_missing(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(app, ["harden", "--scan"])

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--config", "agentos_baseline"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_adds_default_scan_when_only_config_is_provided(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(app, ["harden", "--config", "custom_profile"])

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--config", "custom_profile"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_keeps_explicit_equals_style_config(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(
            app, ["harden", "--scan", "--config=custom_profile"]
        )

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--config=custom_profile"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_does_not_add_default_scan_for_reinforce(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(app, ["harden", "--reinforce", "--dry-run"])

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--reinforce", "--dry-run", "--config", "agentos_baseline"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_keeps_explicit_verbose(self, mock_invoke):
        mock_invoke.return_value = ActionResult(success=True, exit_code=0)

        result = self.runner.invoke(app, ["harden", "--scan", "--verbose"])

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once_with(
            "harden",
            args=["--scan", "--verbose", "--config", "agentos_baseline"],
        )

    @patch("agent_sec_cli.cli.invoke")
    def test_harden_downstream_help_uses_backend_help(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout="seharden help\n",
        )

        result = self.runner.invoke(app, ["harden", "--downstream-help"])

        self.assertEqual(result.exit_code, 0)
        self.assertEqual(result.output, "seharden help\n")
        mock_invoke.assert_called_once_with("harden", args=["--help"])


class TestScanPiiCli(unittest.TestCase):
    def setUp(self):
        self.runner = CliRunner()

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_text_json(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout='{"ok": true, "verdict": "warn"}',
            data={
                "ok": True,
                "verdict": "warn",
                "summary": {"total": 1},
                "findings": [],
            },
        )

        result = self.runner.invoke(
            app,
            ["scan-pii", "--text", "alice@example.com", "--source", "manual"],
        )

        self.assertEqual(result.exit_code, 0)
        self.assertIn('"verdict": "warn"', result.output)
        mock_invoke.assert_called_once()
        _, kwargs = mock_invoke.call_args
        self.assertEqual(mock_invoke.call_args.args[0], "pii_scan")
        self.assertEqual(kwargs["text"], "alice@example.com")
        self.assertEqual(kwargs["source"], "manual")
        self.assertFalse(kwargs["raw_evidence"])
        self.assertIsNone(kwargs["max_bytes"])

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_stdin_json(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout='{"ok": true, "verdict": "warn"}',
            data={
                "ok": True,
                "verdict": "warn",
                "summary": {"total": 1},
                "findings": [],
            },
        )

        result = self.runner.invoke(
            app,
            ["scan-pii", "--stdin", "--source", "manual"],
            input="alice@example.com",
        )

        self.assertEqual(result.exit_code, 0)
        mock_invoke.assert_called_once()
        _, kwargs = mock_invoke.call_args
        self.assertEqual(kwargs["text"], "alice@example.com")
        self.assertEqual(kwargs["source"], "manual")

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_stdin_reports_byte_limit(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout='{"ok": true, "verdict": "pass"}',
            data={
                "ok": True,
                "verdict": "pass",
                "summary": {"total": 0},
                "findings": [],
            },
        )

        text = "备注🙂 alice@example.com"
        max_bytes = len("备注".encode("utf-8")) + 1
        result = self.runner.invoke(
            app,
            ["scan-pii", "--stdin", "--max-bytes", str(max_bytes)],
            input=text,
        )

        self.assertEqual(result.exit_code, 0)
        _, kwargs = mock_invoke.call_args
        self.assertEqual(kwargs["text"], "备注")
        self.assertTrue(kwargs["input_truncated"])
        self.assertEqual(kwargs["input_bytes_scanned"], max_bytes)
        self.assertNotIn("\ufffd", kwargs["text"])

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_stdin_rejects_invalid_utf8(self, mock_invoke):
        result = self.runner.invoke(app, ["scan-pii", "--stdin"], input=b"\xff")

        self.assertEqual(result.exit_code, 1)
        self.assertIn("--stdin must be valid UTF-8", result.output)
        mock_invoke.assert_not_called()

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_text_output(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            data={
                "ok": True,
                "verdict": "deny",
                "summary": {"total": 1, "source": "manual"},
                "findings": [
                    {
                        "type": "api_key",
                        "severity": "deny",
                        "confidence": 0.99,
                        "evidence_redacted": "sk-a...[REDACTED]...7890",
                    }
                ],
            },
        )

        result = self.runner.invoke(
            app,
            ["scan-pii", "--text", "api_key=secret", "--format", "text"],
        )

        self.assertEqual(result.exit_code, 0)
        self.assertIn("Verdict: deny", result.output)
        self.assertIn("api_key", result.output)

    def test_scan_pii_requires_one_input(self):
        result = self.runner.invoke(app, ["scan-pii"])

        self.assertEqual(result.exit_code, 1)
        self.assertIn("provide exactly one", result.output)

    def test_scan_pii_rejects_multiple_inputs(self):
        result = self.runner.invoke(
            app,
            ["scan-pii", "--text", "hello", "--stdin"],
            input="alice@example.com",
        )

        self.assertEqual(result.exit_code, 1)
        self.assertIn("provide exactly one", result.output)

    def test_scan_pii_rejects_invalid_source(self):
        result = self.runner.invoke(
            app,
            ["scan-pii", "--text", "hello", "--source", "browser"],
        )

        self.assertEqual(result.exit_code, 1)
        self.assertIn("--source must be one of", result.output)

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_input_default_reads_full_file(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout='{"ok": true, "verdict": "warn"}',
            data={
                "ok": True,
                "verdict": "warn",
                "summary": {"total": 1},
                "findings": [],
            },
        )

        with self.runner.isolated_filesystem():
            text = "备注🙂 alice@example.com"
            Path("input.txt").write_text(text, encoding="utf-8")

            result = self.runner.invoke(
                app,
                ["scan-pii", "--input", "input.txt"],
            )

        self.assertEqual(result.exit_code, 0)
        _, kwargs = mock_invoke.call_args
        self.assertEqual(kwargs["text"], text)
        self.assertFalse(kwargs["input_truncated"])
        self.assertEqual(kwargs["input_bytes_scanned"], len(text.encode("utf-8")))
        self.assertIsNone(kwargs["max_bytes"])

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_input_reports_file_byte_limit(self, mock_invoke):
        mock_invoke.return_value = ActionResult(
            success=True,
            exit_code=0,
            stdout='{"ok": true, "verdict": "pass"}',
            data={
                "ok": True,
                "verdict": "pass",
                "summary": {"total": 0},
                "findings": [],
            },
        )

        with self.runner.isolated_filesystem():
            Path("input.txt").write_bytes("备注🙂 alice".encode("utf-8"))
            max_bytes = len("备注".encode("utf-8")) + 1

            result = self.runner.invoke(
                app,
                [
                    "scan-pii",
                    "--input",
                    "input.txt",
                    "--max-bytes",
                    str(max_bytes),
                ],
            )

        self.assertEqual(result.exit_code, 0)
        _, kwargs = mock_invoke.call_args
        self.assertTrue(kwargs["input_truncated"])
        self.assertEqual(kwargs["input_bytes_scanned"], max_bytes)
        self.assertNotIn("\ufffd", kwargs["text"])

    @patch("agent_sec_cli.pii_checker.cli.invoke")
    def test_scan_pii_input_rejects_invalid_utf8(self, mock_invoke):
        with self.runner.isolated_filesystem():
            Path("input.txt").write_bytes(b"\xff")

            result = self.runner.invoke(
                app,
                ["scan-pii", "--input", "input.txt"],
            )

        self.assertEqual(result.exit_code, 1)
        self.assertIn("--input must be valid UTF-8", result.output)
        mock_invoke.assert_not_called()

    def test_scan_pii_rejects_zero_max_bytes(self):
        result = self.runner.invoke(
            app,
            ["scan-pii", "--text", "hello", "--max-bytes", "0"],
        )

        self.assertEqual(result.exit_code, 1)
        self.assertIn("--max-bytes must be greater than zero", result.output)


if __name__ == "__main__":
    unittest.main()
