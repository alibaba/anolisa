"""Unit tests covering DetectionLayer base, SemanticDetector,
DeBERTaClassifier, and result helper functions."""

import unittest

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.detectors.semantic import SemanticDetector
from agent_sec_cli.prompt_scanner.models.deberta_classifier import (
    DeBERTaClassifier,
)
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ScanResult,
    ThreatType,
    Verdict,
    _best_confidence,
    _verdict_to_risk_level,
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
# Tests: _verdict_to_risk_level
# ---------------------------------------------------------------------------


class TestVerdictToRiskLevel(unittest.TestCase):
    def test_pass_is_low(self) -> None:
        self.assertEqual(_verdict_to_risk_level(Verdict.PASS), "low")

    def test_warn_is_medium(self) -> None:
        self.assertEqual(_verdict_to_risk_level(Verdict.WARN), "medium")

    def test_deny_is_high(self) -> None:
        self.assertEqual(_verdict_to_risk_level(Verdict.DENY), "high")

    def test_error_is_unknown(self) -> None:
        self.assertEqual(_verdict_to_risk_level(Verdict.ERROR), "unknown")


# ---------------------------------------------------------------------------
# Tests: _best_confidence
# ---------------------------------------------------------------------------


def _lr(name: str, score: float, detected: bool = True) -> LayerResult:
    return LayerResult(layer_name=name, detected=detected, score=score)


class TestBestConfidence(unittest.TestCase):
    def test_empty_returns_zero(self) -> None:
        self.assertEqual(_best_confidence([]), 0.0)

    def test_prefers_ml_classifier(self) -> None:
        results = [_lr("rule_engine", 0.9), _lr("ml_classifier", 0.75)]
        self.assertAlmostEqual(_best_confidence(results), 0.75)

    def test_falls_back_to_rule_engine(self) -> None:
        results = [_lr("rule_engine", 0.8)]
        self.assertAlmostEqual(_best_confidence(results), 0.8)

    def test_no_detected_returns_zero(self) -> None:
        results = [_lr("rule_engine", 0.8, detected=False)]
        self.assertEqual(_best_confidence(results), 0.0)

    def test_ml_not_detected_ignored(self) -> None:
        results = [
            _lr("rule_engine", 0.8),
            _lr("ml_classifier", 0.9, detected=False),
        ]
        self.assertAlmostEqual(_best_confidence(results), 0.8)


# ---------------------------------------------------------------------------
# Tests: ScanResult._build_summary
# ---------------------------------------------------------------------------


class TestScanResultBuildSummary(unittest.TestCase):
    def _make(self, is_threat: bool, threat_type: ThreatType) -> ScanResult:
        return ScanResult(
            is_threat=is_threat,
            threat_type=threat_type,
            verdict=Verdict.DENY if is_threat else Verdict.PASS,
        )

    def test_benign_summary(self) -> None:
        result = self._make(False, ThreatType.BENIGN)
        self.assertEqual(result._build_summary(), "No threats detected")

    def test_injection_summary(self) -> None:
        result = self._make(True, ThreatType.DIRECT_INJECTION)
        summary = result._build_summary()
        self.assertIn("Direct Injection", summary)

    def test_jailbreak_summary(self) -> None:
        result = self._make(True, ThreatType.JAILBREAK)
        summary = result._build_summary()
        self.assertIn("Jailbreak", summary)

    def test_indirect_injection_summary(self) -> None:
        result = self._make(True, ThreatType.INDIRECT_INJECTION)
        summary = result._build_summary()
        self.assertIn("Indirect Injection", summary)
