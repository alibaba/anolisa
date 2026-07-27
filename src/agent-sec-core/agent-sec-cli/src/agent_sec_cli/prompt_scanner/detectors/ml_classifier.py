"""L2 ML Classifier detector – Transformer-based classification."""

import threading
import time
from typing import Any

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.exceptions import LayerNotAvailableError
from agent_sec_cli.prompt_scanner.models.model_manager import ModelManager
from agent_sec_cli.prompt_scanner.models.prompt_guard import (
    PromptGuardClassifier,
    get_default_threshold,
)
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ThreatDetail,
    ThreatType,
)


class MLClassifier(DetectionLayer):
    """L2 detection layer: ML-based semantic classification.

    Uses ``PromptGuardClassifier`` (Meta Llama Prompt Guard 2) by default.
    A shared ``ModelManager`` is used so the model is loaded at most once
    across multiple ``MLClassifier`` instances.
    """

    # Singleton ModelManager shared across all MLClassifier instances
    _shared_manager: ModelManager | None = None
    _manager_lock: threading.Lock = threading.Lock()

    def __init__(
        self,
        model_name: str = "LLM-Research/Llama-Prompt-Guard-2-86M",
        threshold: float | None = None,
    ) -> None:
        with MLClassifier._manager_lock:
            if MLClassifier._shared_manager is None:
                MLClassifier._shared_manager = ModelManager()
        self._classifier = PromptGuardClassifier(
            model_name=model_name,
            manager=MLClassifier._shared_manager,
        )
        # Fall back to the per-model recommended threshold when no override given.
        self._threshold = (
            threshold if threshold is not None else get_default_threshold(model_name)
        )

    @property
    def name(self) -> str:
        return "ml_classifier"

    def warmup(self) -> None:
        """Eagerly download and load the ML model.

        Triggers ModelScope snapshot_download and transformers model load
        so that the first detect() call has no cold-start delay.
        Idempotent: subsequent calls return immediately from the in-memory cache.
        """
        self._classifier.warmup()

    def detect(self, text: str, metadata: dict[str, Any] | None = None) -> LayerResult:
        """Classify *text* via PromptGuardClassifier and return a LayerResult.

        Args:
            text:     Prompt text to classify (should be preprocessed).
            metadata: Optional metadata dict; unused by this layer.

        Returns:
            ``LayerResult`` with score = max(injection_prob, jailbreak_prob)
            when a threat is detected, otherwise the benign probability.

        Raises:
            LayerNotAvailableError: if torch / transformers are not installed.
        """
        if not self.is_available():
            raise LayerNotAvailableError(
                "ML classifier requires torch and transformers. "
            )

        t0 = time.perf_counter()
        result = self._classifier.classify(text)
        latency_ms = (time.perf_counter() - t0) * 1000

        threat_type = result.threat_type
        is_threat = (
            threat_type != ThreatType.BENIGN and result.confidence >= self._threshold
        )

        details: list[ThreatDetail] = []
        if is_threat:
            details.append(
                ThreatDetail(
                    rule_id=f"ML-{result.label}",
                    description=(
                        f"ML classifier detected {threat_type.value} "
                        f"(confidence {result.confidence:.2%})"
                    ),
                    matched_text=text[:200],
                    category=threat_type.value,
                )
            )

        return LayerResult(
            layer_name=self.name,
            detected=is_threat,
            score=result.confidence if is_threat else 0.0,
            details=details,
            latency_ms=latency_ms,
        )
