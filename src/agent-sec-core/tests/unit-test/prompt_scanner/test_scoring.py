"""Unit tests for prompt_scanner.scoring."""

import unittest

from agent_sec_cli.prompt_scanner.result import LayerResult, Verdict
from agent_sec_cli.prompt_scanner.scoring import (
    WEIGHTS,
    compute_score,
    determine_verdict,
)


def _lr(name: str, score: float, detected: bool = True) -> LayerResult:
    """Helper: build a minimal LayerResult."""
    return LayerResult(layer_name=name, detected=detected, score=score)


class TestComputeScore(unittest.TestCase):
    """Tests for compute_score (weighted-max)."""

    def test_empty_returns_zero(self) -> None:
        self.assertEqual(compute_score([]), 0.0)

    def test_single_rule_engine(self) -> None:
        result = compute_score([_lr("rule_engine", 1.0)])
        self.assertAlmostEqual(result, 1.0 * WEIGHTS["rule_engine"])

    def test_single_ml_classifier(self) -> None:
        result = compute_score([_lr("ml_classifier", 0.9)])
        self.assertAlmostEqual(result, 0.9 * WEIGHTS["ml_classifier"])

    def test_max_wins(self) -> None:
        # rule_engine score=1.0 → 0.7; ml_classifier score=0.8 → 0.8 → max=0.8
        result = compute_score(
            [
                _lr("rule_engine", 1.0),
                _lr("ml_classifier", 0.8),
            ]
        )
        self.assertAlmostEqual(result, 0.8)

    def test_unknown_layer_weight_defaults_to_one(self) -> None:
        result = compute_score([_lr("custom_layer", 0.6)])
        self.assertAlmostEqual(result, 0.6)

    def test_all_layers(self) -> None:
        # rule_engine=0.5 → 0.35; ml=1.0 → 1.0; semantic=0.9 → 0.72 → max=1.0
        result = compute_score(
            [
                _lr("rule_engine", 0.5),
                _lr("ml_classifier", 1.0),
                _lr("semantic", 0.9),
            ]
        )
        self.assertAlmostEqual(result, 1.0)

    def test_low_scores_no_detection(self) -> None:
        result = compute_score(
            [
                _lr("rule_engine", 0.1, detected=False),
                _lr("ml_classifier", 0.1, detected=False),
            ]
        )
        # rule_engine: 0.1*0.7=0.07; ml: 0.1*1.0=0.1 → max=0.1
        self.assertAlmostEqual(result, 0.1)


class TestDetermineVerdict(unittest.TestCase):
    """Tests for determine_verdict."""

    def test_below_threshold_is_pass(self) -> None:
        self.assertEqual(determine_verdict(0.0), Verdict.PASS)
        self.assertEqual(determine_verdict(0.49), Verdict.PASS)

    def test_at_threshold_is_warn(self) -> None:
        self.assertEqual(determine_verdict(0.5), Verdict.WARN)

    def test_mid_range_is_warn(self) -> None:
        self.assertEqual(determine_verdict(0.7), Verdict.WARN)
        self.assertEqual(determine_verdict(0.79), Verdict.WARN)

    def test_at_0_8_is_deny(self) -> None:
        self.assertEqual(determine_verdict(0.8), Verdict.DENY)

    def test_above_0_8_is_deny(self) -> None:
        self.assertEqual(determine_verdict(1.0), Verdict.DENY)

    def test_custom_threshold(self) -> None:
        self.assertEqual(determine_verdict(0.3, threshold=0.4), Verdict.PASS)
        self.assertEqual(determine_verdict(0.4, threshold=0.4), Verdict.WARN)
        self.assertEqual(determine_verdict(0.8, threshold=0.4), Verdict.DENY)
