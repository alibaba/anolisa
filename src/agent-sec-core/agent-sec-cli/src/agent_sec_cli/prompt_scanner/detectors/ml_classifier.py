"""L2 ML Classifier detector – Transformer-based classification."""

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.exceptions import LayerNotAvailableError
from agent_sec_cli.prompt_scanner.result import LayerResult


class MLClassifier(DetectionLayer):
    """L2 detection layer: ML-based semantic classification.

    Wraps DeBERTa-v3 or Prompt Guard 2 via the model manager.
    This is a stub – real inference logic will be implemented in a
    subsequent commit.
    """

    @property
    def name(self) -> str:
        return "ml_classifier"

    def is_available(self) -> bool:
        """Check whether torch and transformers are installed."""
        try:
            import torch  # noqa: F401
            import transformers  # noqa: F401

            return True
        except ImportError:
            return False

    def detect(self, text: str, metadata: dict | None = None) -> LayerResult:
        """Classify *text* using the configured ML model.  (stub)"""
        if not self.is_available():
            raise LayerNotAvailableError(
                "ML classifier requires torch and transformers. "
                "Install with: pip install prompt-scanner[ml]"
            )
        # TODO: tokenize -> forward pass -> softmax -> ClassifierResult
        return LayerResult(layer_name=self.name, detected=False, score=0.0)
