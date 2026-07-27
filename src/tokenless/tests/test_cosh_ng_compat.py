#!/usr/bin/env python3
"""Tests for Cosh-NG compatibility in tokenless hooks.

Tests the acceptance criteria from issue #1615:
- Cosh-NG runtime detection
- llmContent extraction (only model-visible content compressed)
- Replacement field emission for PostToolUse
- tool_input field emission for PreToolUse
- Cosh-NG agent ID attribution
- Version detection and fail-open for unsupported versions
- Unsupported runtimes pass through without duplicate injection
"""

import json
import os
import sys
import unittest
from unittest.mock import patch

# Add the hooks directory to path
HOOK_DIR = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "adapters", "tokenless", "common", "hooks"
)
sys.path.insert(0, HOOK_DIR)

from hook_utils import (
    build_cosh_ng_post_tool_output,
    build_cosh_ng_pre_tool_output,
    cosh_ng_supports_replacement,
    detect_cosh_ng_version,
    extract_llm_content,
    is_cosh_ng_runtime,
    parse_version,
)


class TestCoshNGDetection(unittest.TestCase):
    """Test Cosh-NG runtime detection from hook input."""

    def test_detect_cosh_ng_wrapped_response(self):
        """Cosh-NG wraps tool_response as {llmContent, returnDisplay}."""
        input_data = {
            "tool_name": "Bash",
            "tool_response": {
                "llmContent": "command output here",
                "returnDisplay": "Bash completed",
            },
        }
        self.assertTrue(is_cosh_ng_runtime(input_data))

    def test_detect_copilot_shell_string_response(self):
        """Copilot-Shell passes tool_response as a plain string."""
        input_data = {
            "tool_name": "Bash",
            "tool_response": '{"exit_code":0,"stdout":"hello"}',
        }
        self.assertFalse(is_cosh_ng_runtime(input_data))

    def test_detect_empty_response(self):
        """Empty tool_response is not Cosh-NG."""
        input_data = {"tool_name": "Bash", "tool_response": ""}
        self.assertFalse(is_cosh_ng_runtime(input_data))

    def test_detect_missing_response(self):
        """Missing tool_response is not Cosh-NG."""
        input_data = {"tool_name": "Bash"}
        self.assertFalse(is_cosh_ng_runtime(input_data))

    def test_detect_dict_without_llm_content(self):
        """Dict without llmContent is not Cosh-NG wrapper."""
        input_data = {
            "tool_name": "Bash",
            "tool_response": {"data": "some value"},
        }
        self.assertFalse(is_cosh_ng_runtime(input_data))


class TestLLMContentExtraction(unittest.TestCase):
    """Test extraction of model-visible llmContent from Cosh-NG wrapper."""

    def test_extract_llm_content_string(self):
        """Extract llmContent when it's a string."""
        input_data = {
            "tool_response": {
                "llmContent": "model visible content",
                "returnDisplay": "display text",
            }
        }
        self.assertEqual(extract_llm_content(input_data), "model visible content")

    def test_extract_llm_content_missing(self):
        """Return None when llmContent is missing."""
        input_data = {
            "tool_response": {
                "returnDisplay": "display text",
            }
        }
        self.assertIsNone(extract_llm_content(input_data))

    def test_extract_llm_content_empty(self):
        """Return None when llmContent is empty."""
        input_data = {
            "tool_response": {
                "llmContent": "",
                "returnDisplay": "display text",
            }
        }
        self.assertIsNone(extract_llm_content(input_data))

    def test_extract_llm_content_non_dict(self):
        """Return None when tool_response is not a dict."""
        input_data = {"tool_response": "plain string"}
        self.assertIsNone(extract_llm_content(input_data))

    def test_extract_llm_content_non_string(self):
        """Return None when llmContent is not a string."""
        input_data = {
            "tool_response": {
                "llmContent": {"nested": "object"},
                "returnDisplay": "display",
            }
        }
        self.assertIsNone(extract_llm_content(input_data))

    def test_return_display_not_extracted(self):
        """returnDisplay must never be extracted as model-visible content."""
        input_data = {
            "tool_response": {
                "llmContent": "for model",
                "returnDisplay": "for display only",
            }
        }
        result = extract_llm_content(input_data)
        self.assertEqual(result, "for model")
        self.assertNotIn("display", result)


class TestBuildCoshNGOutput(unittest.TestCase):
    """Test building Cosh-NG-compatible hook output."""

    def test_post_tool_output_with_replacement(self):
        """PostToolUse output includes replacement field."""
        output = build_cosh_ng_post_tool_output(
            replacement="compressed content",
            additional_context="[tokenless:env] error info",
        )
        self.assertIn("hookSpecificOutput", output)
        specific = output["hookSpecificOutput"]
        self.assertEqual(specific["hookEventName"], "PostToolUse")
        self.assertEqual(specific["replacement"], "compressed content")
        self.assertEqual(specific["additionalContext"], "[tokenless:env] error info")

    def test_post_tool_output_no_replacement(self):
        """PostToolUse output without replacement (env-only attribution)."""
        output = build_cosh_ng_post_tool_output(
            replacement=None,
            additional_context="env attribution",
        )
        specific = output["hookSpecificOutput"]
        self.assertNotIn("replacement", specific)
        self.assertEqual(specific["additionalContext"], "env attribution")

    def test_post_tool_output_no_additional_context(self):
        """PostToolUse output without additional context."""
        output = build_cosh_ng_post_tool_output(
            replacement="compressed",
            additional_context=None,
        )
        specific = output["hookSpecificOutput"]
        self.assertEqual(specific["replacement"], "compressed")
        self.assertNotIn("additionalContext", specific)

    def test_pre_tool_output_dual_field(self):
        """PreToolUse output includes both tool_input and updatedInput."""
        output = build_cosh_ng_pre_tool_output(
            tool_input={"command": "ls -la"},
            decision="allow",
        )
        specific = output["hookSpecificOutput"]
        self.assertEqual(specific["hookEventName"], "PreToolUse")
        self.assertEqual(specific["permissionDecision"], "allow")
        # Cosh-NG reads tool_input
        self.assertEqual(specific["tool_input"], {"command": "ls -la"})
        # Codex reads updatedInput
        self.assertEqual(specific["updatedInput"], {"command": "ls -la"})

    def test_return_display_absent_from_post_output(self):
        """returnDisplay must never appear in the hook output."""
        output = build_cosh_ng_post_tool_output(
            replacement="content",
            additional_context="ctx",
        )
        serialized = json.dumps(output)
        self.assertNotIn("returnDisplay", serialized)


class TestVersionDetection(unittest.TestCase):
    """Test Cosh-NG version detection and replacement support."""

    def test_parse_version(self):
        """Parse standard version strings."""
        self.assertEqual(parse_version("0.6.0"), (0, 6, 0))
        self.assertEqual(parse_version("1.2.3"), (1, 2, 3))
        self.assertEqual(parse_version("v0.5.1-beta"), (0, 5, 1))

    def test_parse_version_invalid(self):
        """Return None for invalid version strings."""
        self.assertIsNone(parse_version(""))
        self.assertIsNone(parse_version("abc"))
        self.assertIsNone(parse_version("0.1"))

    @patch.dict(os.environ, {"COSH_NG_VERSION": "0.6.0"})
    def test_detect_version_set(self):
        """Detect version from COSH_NG_VERSION env var."""
        self.assertEqual(detect_cosh_ng_version(), (0, 6, 0))

    @patch.dict(os.environ, {}, clear=True)
    def test_detect_version_unset(self):
        """Return None when COSH_NG_VERSION is not set."""
        # Remove the key if present
        os.environ.pop("COSH_NG_VERSION", None)
        self.assertIsNone(detect_cosh_ng_version())

    @patch.dict(os.environ, {"COSH_NG_VERSION": "0.6.0"})
    def test_supports_replacement_supported_version(self):
        """Return True for Cosh-NG >= 0.6.0."""
        self.assertTrue(cosh_ng_supports_replacement())

    @patch.dict(os.environ, {"COSH_NG_VERSION": "0.5.0"})
    def test_supports_replacement_old_version(self):
        """Return False for Cosh-NG < 0.6.0."""
        self.assertFalse(cosh_ng_supports_replacement())

    @patch.dict(os.environ, {"COSH_NG_VERSION": "1.0.0"})
    def test_supports_replacement_future_version(self):
        """Return True for Cosh-NG >= 1.0.0."""
        self.assertTrue(cosh_ng_supports_replacement())

    def test_supports_replacement_no_version(self):
        """Return False when version not set (Cosh-NG running but version missing)."""
        os.environ.pop("COSH_NG_VERSION", None)
        self.assertFalse(cosh_ng_supports_replacement())

    @patch.dict(os.environ, {"COSH_NG_VERSION": "invalid-version"})
    def test_supports_replacement_unparseable_version(self):
        """Return False when version is set but unparseable."""
        self.assertFalse(cosh_ng_supports_replacement())


class TestAgentIDAttribution(unittest.TestCase):
    """Test that agent ID is correctly set for different runtimes."""

    @patch.dict(os.environ, {"COSH_NG_VERSION": "0.6.0"}, clear=False)
    def test_cosh_ng_agent_id(self):
        """Cosh-NG hooks should use 'cosh-ng' as agent ID."""
        from hook_utils import resolve_agent_id
        self.assertEqual(resolve_agent_id(), "cosh-ng")

    @patch.dict(os.environ, {"TOKENLESS_AGENT_ID": "tokenless"}, clear=False)
    def test_non_cosh_ng_agent_id(self):
        """Non-Cosh-NG hooks use the TOKENLESS_AGENT_ID env var."""
        from hook_utils import resolve_agent_id
        # Remove Cosh-NG env vars to ensure non-Cosh-NG path
        os.environ.pop("COSH_NG_VERSION", None)
        os.environ.pop("COSH_RUNTIME", None)
        result = resolve_agent_id()
        self.assertNotEqual(result, "cosh-ng")


class TestOutputFormat(unittest.TestCase):
    """Test that hook output JSON is well-formed and complete."""

    def test_post_tool_output_serializable(self):
        """Cosh-NG PostToolUse output is valid JSON."""
        output = build_cosh_ng_post_tool_output(
            replacement="test",
            additional_context="ctx",
        )
        serialized = json.dumps(output, ensure_ascii=False)
        reparsed = json.loads(serialized)
        self.assertEqual(reparsed["hookSpecificOutput"]["replacement"], "test")

    def test_pre_tool_output_serializable(self):
        """Cosh-NG PreToolUse output is valid JSON."""
        output = build_cosh_ng_pre_tool_output(
            tool_input={"command": "echo hello"},
        )
        serialized = json.dumps(output, ensure_ascii=False)
        reparsed = json.loads(serialized)
        self.assertIn("tool_input", reparsed["hookSpecificOutput"])
        self.assertIn("updatedInput", reparsed["hookSpecificOutput"])

    def test_original_sentinel_absent(self):
        """The original response sentinel must not appear in replacement."""
        original = "original full output with lots of data"
        compressed = "compressed"
        output = build_cosh_ng_post_tool_output(
            replacement=compressed,
        )
        serialized = json.dumps(output)
        self.assertNotIn(original, serialized)
        self.assertIn(compressed, serialized)


if __name__ == "__main__":
    unittest.main(verbosity=2)
