"""Unit tests covering remaining branches in detectors/base, semantic,
deberta_classifier, and result._score_to_severity."""

import unittest

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.detectors.semantic import SemanticDetector
from agent_sec_cli.prompt_scanner.models.deberta_classifier import (
    DeBERTaClassifier,
)
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ScanResult,
    Severity,
    ThreatType,
    Verdict,
    _score_to_severity,
)

# ---------------------------------------------------------------------------
# Tests: DetectionLayer abstract base
# ---------------------------------------------------------------------------


class TestDetectionLayerBase(unittest.TestCase):
    """Verify DetectionLayer contract via a minimal concrete subclass."""

    def _make_concrete(self, available: bool = True) -> DetectionLayer:
        """Build a minimal concrete DetectionLayer for testing."""

        class _Stub(DetectionLayer):
            @property
            def name(self) -> str:
                return "stub"

            def detect(self, text: str, metadata: dict | None = None) -> LayerResult:
                return LayerResult(layer_name=self.name, detected=False, score=0.0)

            def is_available(self) -> bool:
                return available

        return _Stub()

    def test_name_property(self) -> None:
        layer = self._make_concrete()
        self.assertEqual(layer.name, "stub")

    def test_detect_returns_layer_result(self) -> None:
        layer = self._make_concrete()
        result = layer.detect("hello")
        self.assertIsInstance(result, LayerResult)
        self.assertEqual(result.layer_name, "stub")

    def test_is_available_default_true(self) -> None:
        # Default implementation in base returns True
        layer = self._make_concrete(available=True)
        self.assertTrue(layer.is_available())

    def test_is_available_overrideable(self) -> None:
        layer = self._make_concrete(available=False)
        self.assertFalse(layer.is_available())

    def test_detect_with_metadata(self) -> None:
        layer = self._make_concrete()
        result = layer.detect("hello", metadata={"source": "user"})
        self.assertIsInstance(result, LayerResult)


# ---------------------------------------------------------------------------
# Tests: SemanticDetector (L3 stub)
# ---------------------------------------------------------------------------


class TestSemanticDetector(unittest.TestCase):
    def setUp(self) -> None:
        self.detector = SemanticDetector()

    def test_name(self) -> None:
        self.assertEqual(self.detector.name, "semantic")

    def test_is_available_returns_false(self) -> None:
        self.assertFalse(self.detector.is_available())

    def test_detect_raises_not_implemented(self) -> None:
        with self.assertRaises(NotImplementedError):
            self.detector.detect("any text")

    def test_detect_raises_with_metadata(self) -> None:
        with self.assertRaises(NotImplementedError):
            self.detector.detect("text", metadata={"key": "val"})


# ---------------------------------------------------------------------------
# Tests: DeBERTaClassifier (stub)
# ---------------------------------------------------------------------------


class TestDeBERTaClassifier(unittest.TestCase):
    def setUp(self) -> None:
        self.clf = DeBERTaClassifier()

    def test_default_model_name(self) -> None:
        self.assertEqual(self.clf._model_name, "deberta-v3-base-injection")

    def test_custom_model_name(self) -> None:
        clf = DeBERTaClassifier(model_name="my-model", device="cuda")
        self.assertEqual(clf._model_name, "my-model")
        self.assertEqual(clf._device, "cuda")

    def test_classify_raises_not_implemented(self) -> None:
        with self.assertRaises(NotImplementedError):
            self.clf.classify("test text")

    def test_classify_batch_raises_not_implemented(self) -> None:
        with self.assertRaises(NotImplementedError):
            self.clf.classify_batch(["text1", "text2"])

    def test_model_and_tokenizer_initially_none(self) -> None:
        self.assertIsNone(self.clf._model)
        self.assertIsNone(self.clf._tokenizer)


# ---------------------------------------------------------------------------
# Tests: result._score_to_severity boundary values
# ---------------------------------------------------------------------------


class TestScoreToSeverity(unittest.TestCase):
    def test_critical_at_0_9(self) -> None:
        self.assertEqual(_score_to_severity(0.9), Severity.CRITICAL)

    def test_critical_above_0_9(self) -> None:
        self.assertEqual(_score_to_severity(1.0), Severity.CRITICAL)

    def test_high_at_0_7(self) -> None:
        self.assertEqual(_score_to_severity(0.7), Severity.HIGH)

    def test_high_below_0_9(self) -> None:
        self.assertEqual(_score_to_severity(0.89), Severity.HIGH)

    def test_medium_at_0_4(self) -> None:
        self.assertEqual(_score_to_severity(0.4), Severity.MEDIUM)

    def test_medium_below_0_7(self) -> None:
        self.assertEqual(_score_to_severity(0.69), Severity.MEDIUM)

    def test_low_below_0_4(self) -> None:
        self.assertEqual(_score_to_severity(0.39), Severity.LOW)

    def test_low_at_zero(self) -> None:
        self.assertEqual(_score_to_severity(0.0), Severity.LOW)


# ---------------------------------------------------------------------------
# Tests: ScanResult._build_summary
# ---------------------------------------------------------------------------


class TestScanResultBuildSummary(unittest.TestCase):
    def _make(self, is_threat: bool, threat_type: ThreatType) -> ScanResult:
        return ScanResult(
            is_threat=is_threat,
            threat_type=threat_type,
            risk_score=0.9 if is_threat else 0.1,
            confidence=0.9 if is_threat else 0.1,
            verdict=Verdict.DENY if is_threat else Verdict.PASS,
        )

    def test_benign_summary(self) -> None:
        result = self._make(False, ThreatType.BENIGN)
        self.assertEqual(result._build_summary(), "No threats detected")

    def test_injection_summary(self) -> None:
        result = self._make(True, ThreatType.DIRECT_INJECTION)
        self.assertIn("direct_injection", result._build_summary())

    def test_jailbreak_summary(self) -> None:
        result = self._make(True, ThreatType.JAILBREAK)
        self.assertIn("jailbreak", result._build_summary())

    def test_indirect_injection_summary(self) -> None:
        result = self._make(True, ThreatType.INDIRECT_INJECTION)
        self.assertIn("indirect_injection", result._build_summary())
