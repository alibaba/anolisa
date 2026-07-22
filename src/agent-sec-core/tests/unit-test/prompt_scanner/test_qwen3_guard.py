"""Unit tests for the Qwen3Guard Ollama wrapper.

Strategy
--------
No Ollama service is required: all tests exercise the pure parsing and
mapping helpers (``_parse_guard_response`` / ``_response_to_result``)
directly with crafted model output strings.

Test classes
------------
- TestParseGuardResponse – ``Safety:`` / ``Categories:`` line parsing
- TestResponseToResult   – parsed output → ``ClassifierResult`` mapping,
  including the UNKNOWN contract that ``MLClassifier.detect`` relies on
  to make its explicit fail-open decision.
"""

import unittest

from agent_sec_cli.prompt_scanner.models.model_manager import ClassifierResult
from agent_sec_cli.prompt_scanner.models.qwen3_guard import (
    Qwen3GuardClassifier,
    _parse_guard_response,
    is_unknown_label,
)
from agent_sec_cli.prompt_scanner.result import ThreatType


class TestParseGuardResponse(unittest.TestCase):
    """Parsing of ``Safety:`` / ``Categories:`` lines and bare-label fallback."""

    def test_parses_safety_and_categories(self) -> None:
        parsed = _parse_guard_response("Safety: Unsafe\nCategories: Violent")
        self.assertEqual(parsed, {"safety": "Unsafe", "categories": "Violent"})

    def test_keys_are_case_insensitive(self) -> None:
        parsed = _parse_guard_response("SAFETY: Safe")
        self.assertEqual(parsed, {"safety": "Safe"})

    def test_values_are_stripped(self) -> None:
        parsed = _parse_guard_response("  Safety:   Controversial  ")
        self.assertEqual(parsed, {"safety": "Controversial"})

    def test_bare_single_label_fallback(self) -> None:
        parsed = _parse_guard_response("Unsafe")
        self.assertEqual(parsed, {"safety": "Unsafe"})

    def test_verbose_single_line_rejected(self) -> None:
        # More than three words must not be interpreted as a bare label.
        parsed = _parse_guard_response("this is a verbose explanation")
        self.assertEqual(parsed, {})

    def test_multiline_without_keys_rejected(self) -> None:
        parsed = _parse_guard_response("I cannot classify this\nbecause reasons")
        self.assertEqual(parsed, {})

    def test_unrelated_keys_ignored(self) -> None:
        parsed = _parse_guard_response("Reason: none\nVerdict: maybe")
        self.assertEqual(parsed, {})


class TestResponseToResult(unittest.TestCase):
    """Mapping of parsed Qwen3Guard output to ``ClassifierResult``."""

    def _to_result(self, raw_text: str) -> ClassifierResult:
        return Qwen3GuardClassifier._response_to_result(raw_text)

    def test_safe_maps_to_benign(self) -> None:
        result = self._to_result("Safety: Safe")
        self.assertEqual(result.label, "SAFE")
        self.assertEqual(result.threat_type, ThreatType.BENIGN)
        self.assertIsNone(result.confidence)

    def test_controversial_preserves_category(self) -> None:
        result = self._to_result("Safety: Controversial\nCategories: Violent")
        self.assertEqual(result.label, "CONTROVERSIAL_VIOLENT")
        self.assertEqual(result.threat_type, ThreatType.VIOLENT)

    def test_unsafe_jailbreak_maps_to_jailbreak(self) -> None:
        result = self._to_result("Safety: Unsafe\nCategories: Jailbreak")
        self.assertEqual(result.label, "UNSAFE_JAILBREAK")
        self.assertEqual(result.threat_type, ThreatType.JAILBREAK)

    def test_unparseable_output_maps_to_unknown_threat(self) -> None:
        # Contract that MLClassifier.detect() relies on: unparseable output
        # must surface as UNKNOWN (never disguised as SAFE) so the layer can
        # make an explicit fail-open decision.
        result = self._to_result("I'm not sure how to classify that input")
        self.assertEqual(result.label, "UNKNOWN")
        self.assertEqual(result.threat_type, ThreatType.UNCLASSIFIED_VIOLATION)
        self.assertIsNone(result.confidence)
        self.assertEqual(result.probabilities, {"UNKNOWN": 1.0})

    def test_invalid_safety_value_maps_to_unknown(self) -> None:
        result = self._to_result("Safety: Harmless")
        self.assertEqual(result.label, "UNKNOWN")
        self.assertEqual(result.threat_type, ThreatType.UNCLASSIFIED_VIOLATION)


class TestIsUnknownLabel(unittest.TestCase):
    """UNKNOWN label contract consumed by ``MLClassifier.detect``."""

    def test_unknown_label_recognized(self) -> None:
        self.assertTrue(is_unknown_label("UNKNOWN"))

    def test_other_labels_not_unknown(self) -> None:
        # Exact match only: severity labels and casing variants must not
        # be treated as the unparseable-output marker.
        for label in ("SAFE", "CONTROVERSIAL", "UNSAFE_VIOLENT", "unknown"):
            self.assertFalse(is_unknown_label(label))


class TestIsThreat(unittest.TestCase):
    """Label-based threat judgment consumed by ``MLClassifier.detect``."""

    def test_safe_is_not_threat(self) -> None:
        result = Qwen3GuardClassifier._response_to_result("Safety: Safe")
        self.assertFalse(Qwen3GuardClassifier.is_threat(result))

    def test_controversial_is_threat(self) -> None:
        result = Qwen3GuardClassifier._response_to_result(
            "Safety: Controversial\nCategories: Violent"
        )
        self.assertTrue(Qwen3GuardClassifier.is_threat(result))

    def test_unsafe_is_threat(self) -> None:
        result = Qwen3GuardClassifier._response_to_result("Safety: Unsafe")
        self.assertTrue(Qwen3GuardClassifier.is_threat(result))

    def test_unknown_fails_open(self) -> None:
        # Unparseable output is not positive evidence of a threat: the
        # prompt must not be blocked on it.
        result = Qwen3GuardClassifier._response_to_result("garbled model output here")
        self.assertFalse(Qwen3GuardClassifier.is_threat(result))
