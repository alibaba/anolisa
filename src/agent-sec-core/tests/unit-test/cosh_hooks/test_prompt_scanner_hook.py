"""Unit tests for cosh-extension/hooks/prompt_scanner_hook.py.

The hook is self-contained (no agent_sec_cli imports), so we test it
by importing helpers directly and piping JSON via subprocess for
integration-style tests.

Tests cover:
1. verdict → decision mapping (pass, warn, deny, error, unknown)
2. Error verdict fails open (warmup no longer handled in _format_cosh)
3. Model directory detection + permanent warmup suppression logic
4. Subprocess integration: pipe JSON into the hook and verify stdout
"""

import io
import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

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
from prompt_scanner_hook import (  # noqa: E402
    _cleanup_warmup_marker,
    _format_cosh,
    _is_model_downloaded,
    _is_warmup_reminded,
    _mark_warmup_reminded,
)

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
            _format_cosh({"verdict": "warn", "summary": "suspicious prompt"})
        )
        assert result["decision"] == "ask"
        assert "suspicious prompt" in result["reason"]
        assert "[prompt-scanner]" in result["reason"]

    def test_warn_uses_threat_type_when_no_summary(self):
        result = json.loads(
            _format_cosh({"verdict": "warn", "threat_type": "direct_injection"})
        )
        assert result["decision"] == "ask"
        assert "direct_injection" in result["reason"]

    def test_warn_uses_default_when_no_summary_no_threat_type(self):
        result = json.loads(_format_cosh({"verdict": "warn"}))
        assert result["decision"] == "ask"
        assert "Prompt rejected by security policy" in result["reason"]


class TestFormatCoshDeny:
    """verdict=deny → decision=ask with reason."""

    def test_deny_returns_ask(self):
        result = json.loads(
            _format_cosh({"verdict": "deny", "summary": "jailbreak detected"})
        )
        assert result["decision"] == "ask"
        assert "jailbreak detected" in result["reason"]


class TestFormatCoshError:
    """verdict=error → fail-open allow (warmup handled in main, not _format_cosh)."""

    def test_error_with_warmup_hint_returns_allow(self):
        """_format_cosh no longer handles warmup — it just fails open."""
        result = json.loads(
            _format_cosh(
                {
                    "verdict": "error",
                    "summary": "Model not found. Run agent-sec-cli scan-prompt warmup",
                }
            )
        )
        assert result["decision"] == "allow"

    def test_error_without_warmup_hint_returns_allow(self):
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
# Unit tests: model detection & suppression
# ---------------------------------------------------------------------------


class TestModelDetection:
    """_is_model_downloaded checks for config.json two levels under cache dir,
    mirroring ModelManager._resolve_local_model_path."""

    def test_returns_false_when_cache_dir_missing(self):
        with patch("prompt_scanner_hook._MODEL_CACHE_DIR", Path("/nonexistent")):
            assert _is_model_downloaded() is False

    def test_returns_false_when_no_config_json(self, tmp_path):
        model_dir = tmp_path / "some-org" / "some-model"
        model_dir.mkdir(parents=True)
        # No config.json → not downloaded
        with patch("prompt_scanner_hook._MODEL_CACHE_DIR", tmp_path):
            assert _is_model_downloaded() is False

    def test_returns_true_when_config_json_exists(self, tmp_path):
        model_dir = tmp_path / "some-org" / "some-model"
        model_dir.mkdir(parents=True)
        (model_dir / "config.json").write_text("{}")
        with patch("prompt_scanner_hook._MODEL_CACHE_DIR", tmp_path):
            assert _is_model_downloaded() is True

    def test_decoupled_from_specific_model_name(self, tmp_path):
        """Any model with config.json qualifies — no hardcoded model name."""
        model_dir = tmp_path / "future-org" / "future-model-v3"
        model_dir.mkdir(parents=True)
        (model_dir / "config.json").write_text("{}")
        with patch("prompt_scanner_hook._MODEL_CACHE_DIR", tmp_path):
            assert _is_model_downloaded() is True

    def test_model_safetensors_alone_does_not_count(self, tmp_path):
        """model.safetensors without config.json (e.g. incomplete download) → False."""
        model_dir = tmp_path / "some-org" / "some-model"
        model_dir.mkdir(parents=True)
        (model_dir / "model.safetensors").write_text("")
        with patch("prompt_scanner_hook._MODEL_CACHE_DIR", tmp_path):
            assert _is_model_downloaded() is False


class TestWarmupSuppression:
    """Permanent marker-based suppression: ask once, then allow forever."""

    def test_not_reminded_initially(self, tmp_path):
        marker = tmp_path / "warmup-reminded"
        with patch("prompt_scanner_hook._REMINDER_MARKER_FILE", marker):
            assert _is_warmup_reminded() is False

    def test_mark_creates_marker(self, tmp_path):
        marker = tmp_path / "warmup-reminded"
        with patch("prompt_scanner_hook._REMINDER_MARKER_FILE", marker):
            with patch("prompt_scanner_hook._REMINDER_MARKER_DIR", tmp_path):
                _mark_warmup_reminded()
                assert marker.exists()
                assert _is_warmup_reminded() is True

    def test_suppression_is_permanent(self, tmp_path):
        """Once reminded, marker persists — no TTL expiry."""
        marker = tmp_path / "warmup-reminded"
        with patch("prompt_scanner_hook._REMINDER_MARKER_FILE", marker):
            with patch("prompt_scanner_hook._REMINDER_MARKER_DIR", tmp_path):
                _mark_warmup_reminded()
                # Even after "a long time", still suppressed
                assert _is_warmup_reminded() is True

    def test_mark_best_effort_on_failure(self):
        """_mark_warmup_reminded should not raise on permission errors."""
        with patch("prompt_scanner_hook._REMINDER_MARKER_DIR", Path("/nonexistent")):
            with patch(
                "prompt_scanner_hook._REMINDER_MARKER_FILE",
                Path("/nonexistent/warmup-reminded"),
            ):
                # Should not raise
                _mark_warmup_reminded()


class TestWarmupCleanup:
    """_cleanup_warmup_marker removes the marker when the model is present."""

    def test_cleanup_removes_existing_marker(self, tmp_path):
        marker = tmp_path / "warmup-reminded"
        marker.write_text("reminded")
        with patch("prompt_scanner_hook._REMINDER_MARKER_FILE", marker):
            _cleanup_warmup_marker()
            assert not marker.exists()

    def test_cleanup_is_noop_when_no_marker(self, tmp_path):
        marker = tmp_path / "warmup-reminded"
        # marker does not exist yet
        with patch("prompt_scanner_hook._REMINDER_MARKER_FILE", marker):
            _cleanup_warmup_marker()  # should not raise
            assert not marker.exists()

    def test_cleanup_best_effort_on_failure(self):
        """_cleanup_warmup_marker should not raise on permission errors."""
        with patch(
            "prompt_scanner_hook._REMINDER_MARKER_FILE",
            Path("/nonexistent/warmup-reminded"),
        ):
            _cleanup_warmup_marker()  # should not raise


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

    def test_injects_trace_context_into_scan_prompt_command(
        self, monkeypatch, capsys, tmp_path
    ):
        model_dir = tmp_path / "org" / "model"
        model_dir.mkdir(parents=True)
        (model_dir / "config.json").write_text("{}")
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

        monkeypatch.setattr(prompt_scanner_hook, "_MODEL_CACHE_DIR", tmp_path)
        monkeypatch.setattr(prompt_scanner_hook.subprocess, "run", fake_run)
        monkeypatch.setattr(
            prompt_scanner_hook.sys,
            "stdin",
            io.StringIO(
                json.dumps(
                    {
                        "prompt": "hello",
                        "sessionId": "session-1",
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
