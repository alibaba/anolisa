"""Prompt Guard 2 classifier wrapper.

Model IDs (ModelScope mirror):
    22M fast model : ``LLM-Research/Llama-Prompt-Guard-2-22M``
    86M accurate   : ``LLM-Research/Llama-Prompt-Guard-2-86M``

Label mapping (from the model's ``id2label`` config):
    0 → BENIGN   (LABEL_0)
    1 → JAILBREAK (LABEL_1, covers both injection and jailbreak attempts)

Note: Llama-Prompt-Guard-2 is a **binary** classifier.  There is no
separate INJECTION label; the model flags any malicious prompt
(injection or jailbreak) as JAILBREAK (LABEL_1).
"""

import logging
import threading

from agent_sec_cli.prompt_scanner.models.model_manager import (
    ClassifierResult,
    ModelManager,
)
from agent_sec_cli.prompt_scanner.result import ThreatType

log = logging.getLogger(__name__)

# ModelScope mirror model IDs
_MODEL_22M = "LLM-Research/Llama-Prompt-Guard-2-22M"
_MODEL_86M = "LLM-Research/Llama-Prompt-Guard-2-86M"

# Index in the softmax output vector (binary classifier)
_IDX_BENIGN = 0
_IDX_JAILBREAK = 1

# Label names aligned with the model's id2label config (LABEL_0, LABEL_1)
_LABELS = ["BENIGN", "JAILBREAK"]

# Translate PromptGuard raw labels to domain ThreatType.
# Llama-Prompt-Guard-2 is a binary classifier: LABEL_1 covers both
# injection and jailbreak attempts and is unified as JAILBREAK here.
# This mapping lives here (not in MLClassifier) because it is specific
# to PromptGuard's label schema.
_LABEL_TO_THREAT: dict[str, ThreatType] = {
    "JAILBREAK": ThreatType.JAILBREAK,
    "BENIGN": ThreatType.BENIGN,
}

# Default detection threshold (MALICIOUS softmax probability).
#
# Background: Llama-Prompt-Guard-2 is a binary classifier.  Its inference
# code (Meta's official inference.py) applies softmax over the two logits
# and returns ``probabilities[0, 1]`` — i.e. the probability assigned to
# the MALICIOUS class.  This value lives in [0, 1].
#
# 0.5 as decision boundary: Meta's Model Card example code uses
# ``logits.argmax()`` to determine the predicted class.  For a
# two-class softmax, argmax is mathematically equivalent to using 0.5
# as the decision boundary.  Meta never explicitly recommends 0.5 in the
# model card; the only guidance they give is that the threshold should be
# tuned for the target false-positive rate (Model Card v1:
# "In cases where 3–5 % false-positive rate is too high, a higher
# threshold … can be selected").
#
# Per-model guidance (our own assessment, not Meta's recommendation):
#   - 86M model: well-calibrated; 0.5 mirrors argmax and is a safe default
#   - 22M model: less calibrated; raise to 0.75 to reduce false positives
#     at the cost of missing low-confidence attacks
#
# Unknown / custom models fall back to the conservative 0.5.
_MODEL_THRESHOLDS: dict[str, float] = {
    _MODEL_86M: 0.5,
    _MODEL_22M: 0.75,
}


def get_default_threshold(model_name: str) -> float:
    """Return the recommended detection threshold for *model_name*.

    Looks up a per-model table (see ``_MODEL_THRESHOLDS``).  Unknown or
    custom model names fall back to 0.5 (the argmax-equivalent boundary
    for a binary softmax classifier).
    """
    return _MODEL_THRESHOLDS.get(model_name, 0.5)


class PromptGuardClassifier:
    """Wrapper around Meta Llama Prompt Guard 2 (22M / 86M).

    Features:
    - Lazy model loading on first inference call.
    - Shared ``ModelManager`` for caching and device management.
    - Text preprocessing: strips whitespace and re-aligns token
      boundaries to avoid tokenizer boundary artifacts (mirrors the
      upstream ``_preprocess_text_for_promptguard`` approach).
    - Single-item and batch inference.
    - Returns ``ClassifierResult`` with per-class probabilities.
    """

    def __init__(
        self,
        model_name: str = _MODEL_86M,
        device: str | None = None,
        manager: ModelManager | None = None,
        temperature: float = 1.0,
    ) -> None:
        """Initialise the classifier.

        Args:
            model_name: HuggingFace model ID or local path.
                        Use ``_MODEL_86M`` for higher accuracy.
            device:     Compute device override; ``None`` means auto-detect.
            manager:    Optional shared ``ModelManager`` instance.
                        If ``None`` a private instance is created.
            temperature: Softmax temperature (1.0 = no scaling).
        """
        self._model_name = model_name
        self._temperature = temperature
        self._manager = manager or ModelManager(device=device)
        # The HuggingFace tokenizer (Rust-backed) uses RefCell internally
        # and is NOT thread-safe.  Concurrent calls from scan_batch's
        # ThreadPoolExecutor cause "Already borrowed" panics.  We
        # serialise inference with a lock; L1 (regex) still runs in
        # parallel so overall throughput is barely affected.
        self._inference_lock = threading.Lock()

    # ------------------------------------------------------------------
    # Public inference API
    # ------------------------------------------------------------------

    def warmup(self) -> None:
        """Pre-download the model so that the first scan avoids a download.

        Only fetches model files to the local cache; does **not** load
        the model into memory.  The actual load happens lazily on the
        first ``classify`` / ``classify_batch`` call via ``load_model``.

        Progress output (tqdm, modelscope print) is visible — this is
        intended for the ``scan-prompt warmup`` CLI command.
        """
        self._manager.download_model(self._model_name)
        log.info("Model '%s' downloaded to local cache.", self._model_name)

    def classify(self, text: str) -> ClassifierResult:
        """Classify a single prompt and return a ``ClassifierResult``.

        Lazy-loads the model on the first call.  Thread-safe: concurrent
        calls are serialised via ``_inference_lock`` because the Rust-backed
        tokenizer is not re-entrant.

        Args:
            text: Raw prompt text to classify.

        Returns:
            ``ClassifierResult`` with the predicted label, its confidence,
            and the full probability distribution.

        Raises:
            ModelLoadError: if the model cannot be loaded.
        """
        model, tokenizer = self._manager.load_model(self._model_name)
        with self._inference_lock:
            probs = self._get_probabilities(text, model, tokenizer)
        return self._probs_to_result(probs)

    def classify_batch(self, texts: list[str]) -> list[ClassifierResult]:
        """Classify a batch of prompts for higher throughput.

        Args:
            texts: List of raw prompt texts.

        Returns:
            List of ``ClassifierResult`` in the same order as *texts*.

        Raises:
            ModelLoadError: if the model cannot be loaded.
        """
        if not texts:
            return []
        model, tokenizer = self._manager.load_model(self._model_name)

        import torch  # lazy import: only needed when actually running ML inference
        from torch.nn.functional import softmax  # lazy import

        with self._inference_lock:
            preprocessed = [self._preprocess(t, tokenizer) for t in texts]
            inputs = tokenizer(
                preprocessed,
                return_tensors="pt",
                padding=True,
                truncation=True,
                max_length=512,
            )
            device = torch.device(self._manager.device)
            inputs = {k: v.to(device) for k, v in inputs.items()}

            with torch.no_grad():
                logits = model(**inputs).logits  # (batch, num_labels)

        scaled = logits / self._temperature
        probs_tensor = softmax(scaled, dim=-1)  # (batch, num_labels)
        return [self._probs_to_result(probs_tensor[i]) for i in range(len(texts))]

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _preprocess(text: str, tokenizer: object) -> str:  # type: ignore[override]
        """Strip whitespace and re-align token boundaries.

        Mirrors the ``_preprocess_text_for_promptguard`` logic from the
        upstream LlamaFirewall implementation to avoid tokenizer edge cases.
        Silently returns the original text on any error.
        """
        try:
            cleaned = "".join(ch for ch in text if not ch.isspace())
            index_map = [i for i, ch in enumerate(text) if not ch.isspace()]
            tokens = tokenizer.tokenize(cleaned)  # type: ignore[union-attr]
            result: list[str] = []
            last_end = 0
            for token in tokens:
                token_str = tokenizer.convert_tokens_to_string([token])  # type: ignore[union-attr]
                start = cleaned.index(token_str, last_end)
                end = start + len(token_str)
                orig_start = index_map[start]
                if orig_start > 0 and text[orig_start - 1].isspace():
                    result.append(" ")
                result.append(token_str)
                last_end = end
            return "".join(result)
        except Exception:  # noqa: BLE001
            return text

    def _get_probabilities(self, text: str, model: object, tokenizer: object) -> object:
        """Run a single forward pass and return the softmax probability tensor."""
        import torch  # lazy import: only needed when actually running ML inference
        from torch.nn.functional import softmax  # lazy import

        preprocessed = self._preprocess(text, tokenizer)
        inputs = tokenizer(  # type: ignore[operator]
            preprocessed,
            return_tensors="pt",
            padding=True,
            truncation=True,
            max_length=512,
        )
        device = torch.device(self._manager.device)
        inputs = {k: v.to(device) for k, v in inputs.items()}

        with torch.no_grad():
            logits = model(**inputs).logits  # type: ignore[operator]

        scaled = logits / self._temperature
        return softmax(scaled, dim=-1)

    @staticmethod
    def _probs_to_result(probs: object) -> ClassifierResult:
        """Convert a probability tensor row to a ``ClassifierResult``.

        Accepts both 2-D tensors of shape ``(1, num_labels)`` (from
        :py:meth:`classify`) and 1-D tensors of shape ``(num_labels,)``
        (from :py:meth:`classify_batch`).
        """
        # Normalize to 1-D so that both classify() and classify_batch() paths work.
        if hasattr(probs, "dim") and probs.dim() == 2:  # type: ignore[union-attr]
            probs = probs[0]  # type: ignore[index]
        prob_list: list[float] = (
            probs.tolist()  # type: ignore[union-attr]
            if hasattr(probs, "tolist")
            else [float(p) for p in probs]  # type: ignore[union-attr]
        )
        prob_map = {label: prob_list[i] for i, label in enumerate(_LABELS)}
        best_idx = int(max(range(len(prob_list)), key=lambda i: prob_list[i]))
        best_label = _LABELS[best_idx]
        return ClassifierResult(
            label=best_label,
            confidence=prob_list[best_idx],
            probabilities=prob_map,
            threat_type=_LABEL_TO_THREAT.get(best_label, ThreatType.BENIGN),
        )
