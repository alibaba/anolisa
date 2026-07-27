"""L4 multi-turn intent detector — classifies assistant responses via model_service.

L4 is an **optional** detection layer.  It requires a running Ollama
instance with the target model loaded.  When unavailable, the detector is
silently skipped and MULTI_TURN mode returns a pass-through verdict.

L4 is only invoked when the user explicitly selects ``--mode multi_turn``;
there is no need for a separate disable flag.
"""

import logging
import time
from typing import Any

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.models.multi_turn_intent import (
    MultiTurnIntentClassifier,
)
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ThreatDetail,
)

log = logging.getLogger(__name__)

# Metadata keys the scanner injects when running MULTI_TURN.
META_HISTORY = "conversation_history"
META_ASSISTANT_RESPONSE = "assistant_response"


class MultiTurnIntentDetector(DetectionLayer):
    """L4 multi-turn intent detector: classifies (history, query, response) via model service."""

    def __init__(self, harmful_threshold: float = 0.55) -> None:
        self._classifier: MultiTurnIntentClassifier | None = None
        self._harmful_threshold = harmful_threshold

    @property
    def name(self) -> str:
        return "multi_turn_intent"

    # ------------------------------------------------------------------

    def is_available(self) -> bool:
        """Check if L4 multi-turn intent detection is available.

        Returns False (detector will be silently skipped) when the Ollama
        service is unreachable or the target model is not loaded.
        """
        try:
            return self._get_classifier().check_ready()
        except Exception as exc:  # noqa: BLE001
            log.warning("L4 availability check failed: %s", exc)
            return False

    # ------------------------------------------------------------------

    def detect(self, text: str, metadata: dict[str, Any] | None = None) -> LayerResult:
        meta = metadata or {}
        history = meta.get(META_HISTORY)
        assistant_response = meta.get(META_ASSISTANT_RESPONSE)

        if not isinstance(history, list) or assistant_response is None:
            return self._passthrough(reason="missing_conversation_context")

        t0 = time.perf_counter()

        try:
            classifier = self._get_classifier()
            response = classifier.classify(
                history=history,
                current_query=text,
                assistant_response=str(assistant_response),
            )
        except Exception as exc:  # noqa: BLE001
            log.warning("Intent classifier call failed: %s", exc)
            return self._passthrough(
                reason=f"classifier_error:{type(exc).__name__}",
                latency_ms=(time.perf_counter() - t0) * 1000,
            )

        latency_ms = (time.perf_counter() - t0) * 1000
        verdict = response.get("verdict", "pass")

        if verdict == "block":
            details = [
                ThreatDetail(
                    rule_id="L4-MULTI-TURN",
                    description=(
                        "Multi-turn intent classifier flagged the assistant response "
                        "as harmful given the conversation history."
                    ),
                    matched_text=text[:200],
                    category="jailbreak",
                )
            ]
            return LayerResult(
                layer_name=self.name,
                detected=True,
                score=response.get("p_harmful", 0.95),
                details=details,
                latency_ms=latency_ms,
            )

        return LayerResult(
            layer_name=self.name,
            detected=False,
            score=0.0,
            details=[],
            latency_ms=latency_ms,
        )

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _get_classifier(self) -> MultiTurnIntentClassifier:
        """Return the shared MultiTurnIntentClassifier instance (lazy-init)."""
        if self._classifier is None:
            self._classifier = MultiTurnIntentClassifier(
                harmful_threshold=self._harmful_threshold
            )
        return self._classifier

    @staticmethod
    def _passthrough(*, reason: str, latency_ms: float = 0.0) -> LayerResult:
        log.debug("multi_turn_intent passthrough: %s", reason)
        return LayerResult(
            layer_name="multi_turn_intent",
            detected=False,
            score=0.0,
            details=[],
            latency_ms=latency_ms,
        )
