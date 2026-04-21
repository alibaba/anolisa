"""Unit tests for prompt_scanner.verdict.determine_verdict."""

import unittest

from agent_sec_cli.prompt_scanner.result import LayerResult, Verdict
from agent_sec_cli.prompt_scanner.verdict import determine_verdict


def _lr(name: str, detected: bool, score: float = 0.5) -> LayerResult:
    """Helper: build a minimal LayerResult."""
    return LayerResult(layer_name=name, detected=detected, score=score)


class TestDetermineVerdict(unittest.TestCase):
    """Tests for determine_verdict semantic layer logic."""

    # --- PASS ---

    def test_no_layers_is_pass(self) -> None:
        self.assertEqual(determine_verdict([]), Verdict.PASS)

    def test_all_layers_clean_is_pass(self) -> None:
        results = [
            _lr("rule_engine", detected=False),
            _lr("ml_classifier", detected=False),
        ]
        self.assertEqual(determine_verdict(results), Verdict.PASS)

    # --- DENY via L2 (ml_classifier) ---

    def test_ml_classifier_detected_is_deny(self) -> None:
        results = [_lr("ml_classifier", detected=True)]
        self.assertEqual(determine_verdict(results), Verdict.DENY)

    def test_ml_detected_overrides_rule_engine(self) -> None:
        results = [
            _lr("rule_engine", detected=True),
            _lr("ml_classifier", detected=True),
        ]
        self.assertEqual(determine_verdict(results), Verdict.DENY)

    def test_ml_detected_even_if_rule_engine_clean(self) -> None:
        results = [
            _lr("rule_engine", detected=False),
            _lr("ml_classifier", detected=True),
        ]
        self.assertEqual(determine_verdict(results), Verdict.DENY)

    # --- DENY via L1 only (FAST mode, no L2 present) ---

    def test_rule_engine_only_detected_is_deny(self) -> None:
        """FAST mode: L1 is sole authority → DENY."""
        results = [_lr("rule_engine", detected=True)]
        self.assertEqual(determine_verdict(results), Verdict.DENY)

    # --- WARN: L1 fired but L2 present and did not confirm ---

    def test_rule_engine_fired_ml_did_not_is_warn(self) -> None:
        results = [
            _lr("rule_engine", detected=True),
            _lr("ml_classifier", detected=False),
        ]
        self.assertEqual(determine_verdict(results), Verdict.WARN)

    # --- Edge cases ---

    def test_only_rule_engine_clean_is_pass(self) -> None:
        results = [_lr("rule_engine", detected=False)]
        self.assertEqual(determine_verdict(results), Verdict.PASS)

    def test_unknown_layer_detected_no_ml_is_deny(self) -> None:
        """Unknown layer with no confirm-layer present → DENY (L1 path)."""
        results = [_lr("custom_layer", detected=True)]
        self.assertEqual(determine_verdict(results), Verdict.DENY)

    def test_unknown_layer_detected_ml_clean_is_warn(self) -> None:
        """Unknown layer fires, ML ran but clean → WARN."""
        results = [
            _lr("custom_layer", detected=True),
            _lr("ml_classifier", detected=False),
        ]
        self.assertEqual(determine_verdict(results), Verdict.WARN)
