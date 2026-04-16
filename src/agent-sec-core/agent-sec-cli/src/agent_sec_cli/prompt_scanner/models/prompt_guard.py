"""Prompt Guard 2 classifier wrapper."""

from agent_sec_cli.prompt_scanner.models.model_manager import ClassifierResult


class PromptGuardClassifier:
    """Wrapper around Meta Prompt Guard 2 (22M / 86M).

    Provides three-class classification: INJECTION / JAILBREAK / BENIGN.
    Supports multilingual inputs.

    This is a stub – real implementation in Commit 4.
    """

    def __init__(
        self, model_name: str = "prompt-guard-2-86m", device: str = "cpu"
    ) -> None:
        self._model_name = model_name
        self._device = device
        self._model = None
        self._tokenizer = None

    def classify(self, text: str) -> ClassifierResult:
        """Classify a single prompt text.  (stub)"""
        # TODO: lazy-load model, tokenize, infer, return result
        raise NotImplementedError("Prompt Guard classification is not yet implemented.")

    def classify_batch(self, texts: list[str]) -> list[ClassifierResult]:
        """Classify a batch of prompts for higher throughput.  (stub)"""
        raise NotImplementedError("Batch classification is not yet implemented.")
