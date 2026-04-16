"""Model manager – lazy loading, caching, and device selection."""

from pydantic import BaseModel


class ClassifierResult(BaseModel):
    """Unified result returned by any ML classifier wrapper."""

    label: str  # e.g. "INJECTION", "BENIGN"
    confidence: float  # Probability of the predicted label
    probabilities: dict[str, float]  # Full label -> prob mapping


class ModelManager:
    """Centralized model lifecycle management.

    Responsibilities:
    - Lazy-load models on first use.
    - Cache loaded models in memory.
    - Auto-detect best available device (CPU / CUDA / MPS).
    - Provide ``clear_cache()`` for memory reclamation.

    This is a stub – real implementation in Commit 4.
    """

    _DEFAULT_CACHE_DIR = "~/.cache/prompt_scanner/models"

    def __init__(self, cache_dir: str | None = None, device: str = "cpu") -> None:
        self._cache_dir = cache_dir or self._DEFAULT_CACHE_DIR
        self._device = device
        self._loaded_models: dict[str, object] = {}

    def load_model(self, model_name: str) -> object:
        """Load (or return cached) model by name.  (stub)

        Args:
            model_name: HuggingFace model identifier or local path.

        Returns:
            The loaded model object.
        """
        if model_name in self._loaded_models:
            return self._loaded_models[model_name]
        # TODO: download / load model, move to device, cache
        raise NotImplementedError("Model loading is not yet implemented.")

    def get_model(self, model_name: str) -> object | None:
        """Return the cached model if already loaded, else None."""
        return self._loaded_models.get(model_name)

    def clear_cache(self) -> None:
        """Release all loaded models from memory."""
        self._loaded_models.clear()

    @staticmethod
    def detect_device() -> str:
        """Auto-detect the best available compute device.  (stub)"""
        # TODO: check torch.cuda.is_available(), torch.backends.mps.is_available()
        return "cpu"
