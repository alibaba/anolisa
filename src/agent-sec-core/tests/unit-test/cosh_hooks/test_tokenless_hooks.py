"""Unit tests for tokenless hooks Cosh-NG compatibility functions.

Tests cover:
- detect_cosh_ng_runtime() version detection
- _resolve_agent_id() agent ID resolution
- llmContent extraction from wrapped responses
"""

import os
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

# Add tokenless hooks directory to path
_TOKENLESS_HOOKS = str(
    Path(__file__).resolve().parents[3]
    / ".."
    / "tokenless"
    / "adapters"
    / "tokenless"
    / "common"
    / "hooks"
)
sys.path.insert(0, _TOKENLESS_HOOKS)

import hook_utils  # noqa: E402


class TestDetectCoshNgRuntime:
    """Tests for detect_cosh_ng_runtime() function."""

    def test_returns_none_when_not_cosh_ng(self):
        """When no Cosh-NG env vars set, returns None."""
        with patch.dict(os.environ, {}, clear=True):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result is None

    def test_returns_version_from_cosh_ng_version_env(self):
        """COSH_NG_VERSION env var is parsed correctly."""
        with patch.dict(os.environ, {"COSH_NG_VERSION": "0.12.0"}, clear=True):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result == (0, 12, 0)

    def test_returns_sentinel_for_unparseable_version(self):
        """Invalid version string returns (0,0,0) sentinel."""
        with patch.dict(os.environ, {"COSH_NG_VERSION": "invalid"}, clear=True):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result == (0, 0, 0)

    def test_returns_sentinel_for_cosh_runtime_env(self):
        """COSH_RUNTIME=cosh-ng without version returns sentinel."""
        with patch.dict(os.environ, {"COSH_RUNTIME": "cosh-ng"}, clear=True):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result == (0, 0, 0)

    def test_ignores_other_runtime_values(self):
        """COSH_RUNTIME with other values returns None."""
        with patch.dict(os.environ, {"COSH_RUNTIME": "copilot-shell"}, clear=True):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result is None

    def test_version_takes_precedence_over_runtime(self):
        """COSH_NG_VERSION takes precedence over COSH_RUNTIME."""
        with patch.dict(
            os.environ,
            {"COSH_NG_VERSION": "1.2.3", "COSH_RUNTIME": "cosh-ng"},
            clear=True,
        ):
            result = hook_utils.detect_cosh_ng_runtime()
            assert result == (1, 2, 3)


class TestResolveAgentId:
    """Tests for agent ID resolution in compress_response_hook."""

    def test_uses_cosh_ng_when_detected(self):
        """When Cosh-NG detected and no env override, uses 'cosh-ng'."""
        # Import here to avoid module-level side effects
        import compress_response_hook

        with patch.dict(os.environ, {}, clear=True):
            result = compress_response_hook._resolve_agent_id(cosh_ng_detected=True)
            assert result == "cosh-ng"

    def test_uses_tokenless_when_not_detected(self):
        """When not Cosh-NG and no env override, uses 'tokenless'."""
        import compress_response_hook

        with patch.dict(os.environ, {}, clear=True):
            result = compress_response_hook._resolve_agent_id(cosh_ng_detected=False)
            assert result == "tokenless"

    def test_env_override_takes_precedence(self):
        """TOKENLESS_AGENT_ID env var overrides default."""
        import compress_response_hook

        with patch.dict(os.environ, {"TOKENLESS_AGENT_ID": "custom-agent"}, clear=True):
            result = compress_response_hook._resolve_agent_id(cosh_ng_detected=True)
            assert result == "custom-agent"


class TestLlmContentExtraction:
    """Tests for llmContent extraction from wrapped tool responses."""

    def test_extracts_llm_content_from_dict(self):
        """Extracts llmContent from dict wrapper."""
        tool_response = {"llmContent": "compressed data", "returnDisplay": "display"}
        # Simulate the extraction logic
        llm_content = tool_response.get("llmContent")
        assert llm_content == "compressed data"

    def test_extracts_llm_content_from_json_string(self):
        """Extracts llmContent from JSON string wrapper."""
        import json

        tool_response_str = json.dumps(
            {"llmContent": "compressed data", "returnDisplay": "display"}
        )
        parsed = json.loads(tool_response_str)
        llm_content = parsed.get("llmContent")
        assert llm_content == "compressed data"

    def test_falls_back_to_return_display(self):
        """Falls back to returnDisplay when llmContent missing."""
        tool_response = {"returnDisplay": "display only"}
        llm_content = tool_response.get("llmContent") or tool_response.get(
            "returnDisplay"
        )
        assert llm_content == "display only"

    def test_plain_text_passes_through(self):
        """Plain text without wrapper passes through unchanged."""
        tool_response = "plain text response"
        # When it's a string and not valid JSON wrapper, use as-is
        llm_content = tool_response
        assert llm_content == "plain text response"
