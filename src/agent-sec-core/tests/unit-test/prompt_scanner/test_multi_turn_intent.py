"""Unit tests for prompt_scanner.detectors.multi_turn_intent.

Tests cover:
- MultiTurnIntentDetector.detect: block/pass/passthrough scenarios
- MultiTurnIntentDetector.is_available: delegates to classifier.check_ready
- MultiTurnIntentDetector.name property
- Edge cases: missing context, classifier errors
"""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.prompt_scanner.detectors.multi_turn_intent import (
    META_ASSISTANT_RESPONSE,
    META_HISTORY,
    MultiTurnIntentDetector,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_mock_classifier(
    classify_return: dict | None = None,
    check_ready_return: bool = True,
    classify_side_effect: Exception | None = None,
) -> MagicMock:
    """Create a mock MultiTurnIntentClassifier."""
    clf = MagicMock()
    clf.check_ready.return_value = check_ready_return
    if classify_side_effect is not None:
        clf.classify.side_effect = classify_side_effect
    else:
        clf.classify.return_value = classify_return or {
            "verdict": "pass",
            "p_harmful": 0.1,
            "raw_text": "1",
            "raw_token": "1",
            "latency_ms": 5.0,
        }
    return clf


# ---------------------------------------------------------------------------
# Test: name property
# ---------------------------------------------------------------------------


class TestDetectorName(unittest.TestCase):
    def test_name(self) -> None:
        detector = MultiTurnIntentDetector()
        self.assertEqual(detector.name, "multi_turn_intent")


# ---------------------------------------------------------------------------
# Test: is_available
# ---------------------------------------------------------------------------


class TestIsAvailable(unittest.TestCase):
    def test_available_when_model_ready(self) -> None:
        detector = MultiTurnIntentDetector()
        with patch.object(
            detector,
            "_get_classifier",
            return_value=_make_mock_classifier(check_ready_return=True),
        ):
            self.assertTrue(detector.is_available())

    def test_unavailable_when_model_not_ready(self) -> None:
        detector = MultiTurnIntentDetector()
        with patch.object(
            detector,
            "_get_classifier",
            return_value=_make_mock_classifier(check_ready_return=False),
        ):
            self.assertFalse(detector.is_available())

    def test_unavailable_on_exception(self) -> None:
        """is_available should return False (not raise) on exceptions."""
        detector = MultiTurnIntentDetector()
        mock_clf = MagicMock()
        mock_clf.check_ready.side_effect = RuntimeError("connection failed")
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            self.assertFalse(detector.is_available())


# ---------------------------------------------------------------------------
# Test: detect — block verdict
# ---------------------------------------------------------------------------


class TestDetectBlock(unittest.TestCase):
    def test_detect_returns_block(self) -> None:
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(
            classify_return={
                "verdict": "block",
                "p_harmful": 0.92,
                "raw_text": "0",
                "raw_token": "0",
                "latency_ms": 15.0,
            }
        )
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "what is 2+2?",
                metadata={
                    META_HISTORY: [{"role": "user", "content": "hi"}],
                    META_ASSISTANT_RESPONSE: "4",
                },
            )
        self.assertTrue(result.detected)
        self.assertEqual(result.layer_name, "multi_turn_intent")
        self.assertAlmostEqual(result.score, 0.92)
        self.assertEqual(len(result.details), 1)
        self.assertEqual(result.details[0].rule_id, "L4-MULTI-TURN")
        self.assertEqual(result.details[0].category, "jailbreak")

    def test_detect_block_default_score(self) -> None:
        """When p_harmful missing from response, default to 0.95."""
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(classify_return={"verdict": "block"})
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "query",
                metadata={
                    META_HISTORY: [],
                    META_ASSISTANT_RESPONSE: "resp",
                },
            )
        self.assertTrue(result.detected)
        self.assertAlmostEqual(result.score, 0.95)


# ---------------------------------------------------------------------------
# Test: detect — pass verdict
# ---------------------------------------------------------------------------


class TestDetectPass(unittest.TestCase):
    def test_detect_returns_pass(self) -> None:
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(
            classify_return={
                "verdict": "pass",
                "p_harmful": 0.1,
                "raw_text": "1",
                "raw_token": "1",
                "latency_ms": 8.0,
            }
        )
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "query",
                metadata={
                    META_HISTORY: [{"role": "user", "content": "hi"}],
                    META_ASSISTANT_RESPONSE: "safe response",
                },
            )
        self.assertFalse(result.detected)
        self.assertEqual(result.score, 0.0)
        self.assertEqual(len(result.details), 0)

    def test_detect_unknown_verdict_treated_as_pass(self) -> None:
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(
            classify_return={"verdict": "unknown", "p_harmful": 0.3}
        )
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "query",
                metadata={
                    META_HISTORY: [],
                    META_ASSISTANT_RESPONSE: "resp",
                },
            )
        self.assertFalse(result.detected)


# ---------------------------------------------------------------------------
# Test: detect — passthrough (missing context)
# ---------------------------------------------------------------------------


class TestDetectPassthrough(unittest.TestCase):
    def test_missing_history(self) -> None:
        detector = MultiTurnIntentDetector()
        result = detector.detect("query", metadata={})
        self.assertFalse(result.detected)
        self.assertEqual(result.score, 0.0)

    def test_none_metadata(self) -> None:
        detector = MultiTurnIntentDetector()
        result = detector.detect("query", metadata=None)
        self.assertFalse(result.detected)

    def test_history_not_list(self) -> None:
        detector = MultiTurnIntentDetector()
        result = detector.detect(
            "query",
            metadata={
                META_HISTORY: "not a list",
                META_ASSISTANT_RESPONSE: "resp",
            },
        )
        self.assertFalse(result.detected)

    def test_assistant_response_none(self) -> None:
        detector = MultiTurnIntentDetector()
        result = detector.detect(
            "query",
            metadata={
                META_HISTORY: [{"role": "user", "content": "hi"}],
                META_ASSISTANT_RESPONSE: None,
            },
        )
        self.assertFalse(result.detected)


# ---------------------------------------------------------------------------
# Test: detect — classifier error
# ---------------------------------------------------------------------------


class TestDetectClassifierError(unittest.TestCase):
    def test_classifier_runtime_error_passthrough(self) -> None:
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(
            classify_side_effect=RuntimeError("Ollama unreachable")
        )
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "query",
                metadata={
                    META_HISTORY: [{"role": "user", "content": "hi"}],
                    META_ASSISTANT_RESPONSE: "resp",
                },
            )
        self.assertFalse(result.detected)
        self.assertEqual(result.score, 0.0)
        # Latency should be recorded even on error
        self.assertGreaterEqual(result.latency_ms, 0.0)

    def test_classifier_connection_error_passthrough(self) -> None:
        detector = MultiTurnIntentDetector()
        mock_clf = _make_mock_classifier(
            classify_side_effect=ConnectionError("refused")
        )
        with patch.object(detector, "_get_classifier", return_value=mock_clf):
            result = detector.detect(
                "query",
                metadata={
                    META_HISTORY: [],
                    META_ASSISTANT_RESPONSE: "resp",
                },
            )
        self.assertFalse(result.detected)


# ---------------------------------------------------------------------------
# Test: lazy classifier initialization
# ---------------------------------------------------------------------------


class TestLazyInit(unittest.TestCase):
    def test_classifier_created_on_first_use(self) -> None:
        detector = MultiTurnIntentDetector()
        self.assertIsNone(detector._classifier)

    def test_get_classifier_creates_instance(self) -> None:
        detector = MultiTurnIntentDetector()
        with patch(
            "agent_sec_cli.prompt_scanner.detectors.multi_turn_intent.MultiTurnIntentClassifier"
        ) as mock_cls:
            mock_instance = MagicMock()
            mock_instance.check_ready.return_value = True
            mock_cls.return_value = mock_instance
            clf1 = detector._get_classifier()
            clf2 = detector._get_classifier()
            # Only one instance should be created (lazy singleton)
            self.assertIs(clf1, clf2)
            mock_cls.assert_called_once()


if __name__ == "__main__":
    unittest.main()
