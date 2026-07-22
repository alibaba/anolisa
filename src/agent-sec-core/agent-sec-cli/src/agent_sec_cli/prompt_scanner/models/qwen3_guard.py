"""Qwen3Guard classifier wrapper backed by Ollama.

Qwen3Guard is served by Ollama rather than loaded through transformers.  The
Gen variant returns a structured moderation result with a three-tier severity
label and optional safety categories::

    Safety: Unsafe
    Categories: Violent
"""

import logging
import re

from agent_sec_cli.model_service import ModelServiceClient, create_client
from agent_sec_cli.prompt_scanner.exceptions import (
    ModelInferenceError,
    ModelLoadError,
)
from agent_sec_cli.prompt_scanner.models.model_manager import ClassifierResult
from agent_sec_cli.prompt_scanner.result import ThreatType

log = logging.getLogger(__name__)

_MODEL_QWEN3_GUARD = "qwen3guard:0.6b"

# Label emitted when the model output cannot be parsed into a known safety
# verdict.  Consumers (e.g. MLClassifier.detect) fail open on it — see
# is_unknown_label().
_LABEL_UNKNOWN = "UNKNOWN"

# Single source of truth: official Qwen3Guard category name → ThreatType.
# Keys are lowercase for direct lookup from _parse_categories() output.
_CATEGORY_THREAT_TYPES: dict[str, ThreatType] = {
    "violent": ThreatType.VIOLENT,
    "non-violent illegal acts": ThreatType.NON_VIOLENT_ILLEGAL_ACTS,
    "sexual content or sexual acts": ThreatType.SEXUAL_CONTENT,
    "personally identifiable information": ThreatType.PII,
    "pii": ThreatType.PII,
    "suicide & self-harm": ThreatType.SUICIDE_SELF_HARM,
    "unethical acts": ThreatType.UNETHICAL_ACTS,
    "politically sensitive topics": ThreatType.POLITICALLY_SENSITIVE,
    "copyright violation": ThreatType.COPYRIGHT_VIOLATION,
    "jailbreak": ThreatType.JAILBREAK,
}
# Sort by length descending so longer phrases match first
# (e.g. "Personally Identifiable Information" before "PII").
_CATEGORY_PATTERN = re.compile(
    "|".join(
        re.escape(k) for k in sorted(_CATEGORY_THREAT_TYPES, key=len, reverse=True)
    ),
    re.IGNORECASE,
)
_NON_WORD_RE = re.compile(r"[^a-z0-9]+")

# Currently supported Qwen3Guard model tags.
_SUPPORTED_QWEN3_GUARD_MODELS: frozenset[str] = frozenset({"qwen3guard:0.6b"})


def is_qwen3_guard_model(model_name: str) -> bool:
    """Return whether *model_name* should use the Ollama Qwen3Guard wrapper.

    Checks against ``_SUPPORTED_QWEN3_GUARD_MODELS`` (case-insensitive).
    Currently only ``qwen3guard:0.6b`` is supported; add new tags to the set
    when onboarding additional variants.
    """
    return model_name.strip().lower() in _SUPPORTED_QWEN3_GUARD_MODELS


def is_unknown_label(label: str) -> bool:
    """Return whether *label* marks an unparseable Qwen3Guard response.

    ``Qwen3GuardClassifier`` emits ``_LABEL_UNKNOWN`` when the model output
    cannot be parsed into Safe/Controversial/Unsafe.  It is not positive
    evidence of a threat; callers are expected to fail open on it.
    """
    return label == _LABEL_UNKNOWN


class Qwen3GuardClassifier:
    """Wrapper around ``qwen3guard:0.6b`` served by Ollama."""

    def __init__(
        self,
        model_name: str = _MODEL_QWEN3_GUARD,
        client: ModelServiceClient | None = None,
    ) -> None:
        """Initialise the classifier.

        Args:
            model_name: Ollama model name, usually ``qwen3guard:0.6b``.
            client: Optional model-service client.  Tests can inject a fake client.
        """
        self._model_name = model_name
        self._client: ModelServiceClient = client or create_client()

    def check_ready(self) -> bool:
        """Return whether Ollama can serve the configured Qwen3Guard model."""
        return self._client.check_model(self._model_name)

    def warmup(self) -> None:
        """Verify that Qwen3Guard is already available in Ollama.

        Qwen3Guard is not downloaded by ``scan-prompt warmup``.  Operators must
        pull it with ``ollama pull qwen3guard:0.6b`` before using this wrapper.
        """
        if not self.check_ready():
            raise ModelLoadError(
                "Qwen3Guard is not available in Ollama. "
                "Run `ollama pull qwen3guard:0.6b` first."
            )

    def classify(self, text: str) -> ClassifierResult:
        """Classify a single prompt and return a ``ClassifierResult``.

        Raises:
            ModelInferenceError: if Ollama inference fails or returns empty.
        """
        try:
            body = self._client.chat(
                self._model_name,
                [{"role": "user", "content": text}],
                options={"temperature": 0},
            )
        except Exception as exc:
            raise ModelInferenceError(
                f"Qwen3Guard inference failed ({exc}). Ensure Ollama is reachable "
                "and run `ollama pull qwen3guard:0.6b` before scanning."
            ) from exc
        raw_text = _extract_response_text(body)
        if not raw_text:
            # Empty response indicates service error, not a valid classification.
            raise ModelInferenceError(
                f"Qwen3Guard returned empty response for model={self._model_name}. "
                "Check Ollama service status."
            )
        return self._response_to_result(raw_text)

    def classify_batch(self, texts: list[str]) -> list[ClassifierResult]:
        """Classify prompts sequentially (non-batched) through the Ollama service.

        Each text triggers a separate HTTP call; there is no parallel or
        batched inference.  Sufficient for current scanner throughput.
        """
        return [self.classify(text) for text in texts]

    @staticmethod
    def is_threat(result: ClassifierResult, threshold: float | None = None) -> bool:
        """Judge whether a classification result is a confirmed threat.

        Qwen3Guard judgment is purely label-based: Controversial/Unsafe are
        threats, Safe is benign.  UNKNOWN (unparseable output) fails open by
        design — it is not positive evidence of a threat, so the prompt is
        never blocked on it.  ``threshold`` is accepted for interface
        compatibility with other backends and ignored (no confidence scores).
        """
        if is_unknown_label(result.label):
            return False
        return result.label.startswith(("CONTROVERSIAL", "UNSAFE"))

    @property
    def model_name(self) -> str:
        """Ollama model name used by this classifier."""
        return self._model_name

    @staticmethod
    def _response_to_result(raw_text: str) -> ClassifierResult:
        """Convert Qwen3Guard text output into ``ClassifierResult``."""
        parsed = _parse_guard_response(raw_text)
        safety = _normalize_safety(parsed.get("safety", ""))
        categories = _parse_categories(parsed.get("categories", ""))

        # Qwen3Guard does not return probabilities. `probabilities` here is a
        # one-hot representation of the parsed label for interface compatibility.
        if safety == "safe":
            label = "SAFE"
            threat_type = ThreatType.BENIGN
            probabilities = {"SAFE": 1.0}
        elif safety == "controversial":
            label = _build_label("CONTROVERSIAL", categories)
            threat_type = _threat_type_for_categories(categories)
            probabilities = {label: 1.0}
        elif safety == "unsafe":
            label = _build_label("UNSAFE", categories)
            threat_type = _threat_type_for_categories(categories)
            probabilities = {label: 1.0}
        else:
            # Unparseable output — surfaced as UNKNOWN and logged for
            # observability. MLClassifier.detect() fails open on UNKNOWN:
            # an unparseable response is not positive evidence of a threat,
            # so the prompt is not blocked.
            log.warning("Unparseable Qwen3Guard output: %r", raw_text)
            label = _LABEL_UNKNOWN
            threat_type = ThreatType.UNCLASSIFIED_VIOLATION
            probabilities = {_LABEL_UNKNOWN: 1.0}

        return ClassifierResult(
            label=label,
            confidence=None,  # Qwen3Guard does not output confidence scores
            probabilities=probabilities,
            threat_type=threat_type,
        )


def _extract_response_text(body: dict[str, object]) -> str:
    """Extract assistant text from an Ollama chat response."""
    message = body.get("message")
    if isinstance(message, dict):
        content = message.get("content")
        return content.strip() if isinstance(content, str) else ""
    response = body.get("response")
    return response.strip() if isinstance(response, str) else ""


def _parse_guard_response(raw_text: str) -> dict[str, str]:
    """Parse ``Safety: ...`` / ``Categories: ...`` lines from Qwen3Guard output."""
    parsed: dict[str, str] = {}
    for line in raw_text.splitlines():
        key, sep, value = line.partition(":")
        if not sep:
            continue
        normalized_key = key.strip().lower()
        if normalized_key in {"safety", "categories"}:
            parsed[normalized_key] = value.strip()
    # Fallback: only treat as a bare safety label when the text is short
    # and single-line — avoids interpreting verbose explanations as labels.
    text = raw_text.strip()
    if not parsed and text and "\n" not in text and len(text.split()) <= 3:
        parsed["safety"] = text
    return parsed


def _normalize_safety(raw_safety: str) -> str:
    """Normalize the Qwen3Guard three-tier safety label."""
    safety = raw_safety.strip().lower()
    if safety in {"safe", "unsafe", "controversial"}:
        return safety
    return ""


def _parse_categories(raw_categories: str) -> list[str]:
    """Extract known Qwen3Guard categories as normalized lowercase strings.

    Follows the official Qwen3Guard-Gen regex approach: ``re.findall``
    against known category names, then normalise casing and deduplicate.
    """
    if not raw_categories:
        return []

    matches = _CATEGORY_PATTERN.findall(raw_categories)
    # dict.fromkeys deduplicates while preserving first-seen order.
    categories = list(dict.fromkeys(m.lower() for m in matches))
    if categories:
        return categories

    # Strip whitespace / sentinel values before logging.
    stripped = raw_categories.strip()
    if stripped.lower() not in {"none", "null", "n/a", "na", "safe", ""}:
        log.warning("Qwen3Guard returned non-standard categories: %r", raw_categories)
    return []


def _build_label(severity: str, categories: list[str]) -> str:
    """Build a stable raw label that preserves Qwen3Guard severity."""
    if categories:
        return f"{severity}_{_normalize_label(categories[0])}"
    return severity


def _normalize_label(value: str) -> str:
    """Normalize a model label/category into an uppercase identifier."""
    label = _NON_WORD_RE.sub("_", value.strip().lower()).strip("_")
    return label.upper() if label else "UNCLASSIFIED_VIOLATION"


def _threat_type_for_categories(categories: list[str]) -> ThreatType:
    """Translate Qwen3Guard categories to the prompt scanner threat taxonomy.

    Qwen3Guard may return multiple categories for a single prompt; Jailbreak
    takes priority (it is an injection technique rather than a content
    violation), otherwise the first known category wins.  Unknown or empty
    categories fall back to UNCLASSIFIED_VIOLATION so the threat is still
    surfaced.
    """
    lowered = [category.strip().lower() for category in categories]
    if "jailbreak" in lowered:
        return ThreatType.JAILBREAK
    for category in lowered:
        threat_type = _CATEGORY_THREAT_TYPES.get(category)
        if threat_type is not None:
            return threat_type
    return ThreatType.UNCLASSIFIED_VIOLATION
