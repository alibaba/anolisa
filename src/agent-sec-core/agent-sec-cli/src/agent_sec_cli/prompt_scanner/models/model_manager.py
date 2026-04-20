"""Model manager – lazy loading, caching, and device selection.

Model download strategy
-----------------------
Models are downloaded via **ModelScope SDK** (``modelscope.snapshot_download``)
rather than HuggingFace Hub directly.  This is the preferred approach for
production deployments in mainland China where HuggingFace may be slow or
unavailable.  The downloaded model path is then loaded with ``transformers``
``AutoModel`` / ``AutoTokenizer`` as usual.

ModelScope mirror IDs for Llama Prompt Guard 2:
    22M fast model : ``LLM-Research/Llama-Prompt-Guard-2-22M``
    86M accurate   : ``LLM-Research/Llama-Prompt-Guard-2-86M``
"""

import logging
import os

from pydantic import BaseModel

log = logging.getLogger(__name__)


class ClassifierResult(BaseModel):
    """Unified result returned by any ML classifier wrapper."""

    label: str  # e.g. "JAILBREAK", "BENIGN"
    confidence: float  # Probability of the predicted label (0.0–1.0)
    probabilities: dict[str, float]  # Full label -> prob mapping


class ModelManager:
    """Centralized model lifecycle management.

    Responsibilities:
    - Lazy-load models on first use.
    - Cache loaded (model, tokenizer) pairs in memory.
    - Auto-detect best available device (CPU / CUDA / MPS).
    - Provide ``clear_cache()`` for memory reclamation.

    Each cache entry is a ``(model, tokenizer)`` tuple so callers
    never need to manage the tokenizer separately.
    """

    _DEFAULT_CACHE_DIR = "~/.cache/prompt_scanner/models"

    def __init__(self, cache_dir: str | None = None, device: str | None = None) -> None:
        self._cache_dir = cache_dir or self._DEFAULT_CACHE_DIR
        self._device = device or self.detect_device()
        # cache: model_name -> (model, tokenizer)
        self._loaded_models: dict[str, tuple[object, object]] = {}

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def load_model(self, model_name: str) -> tuple[object, object]:
        """Return a cached ``(model, tokenizer)`` pair, loading on demand.

        Args:
            model_name: HuggingFace model identifier or local path.

        Returns:
            ``(model, tokenizer)`` tuple ready for inference.

        Raises:
            agent_sec_cli.prompt_scanner.exceptions.ModelLoadError: if the
                model cannot be loaded (missing deps, download failure, etc.).
        """
        if model_name in self._loaded_models:
            return self._loaded_models[model_name]

        pair = self._do_load(model_name)
        self._loaded_models[model_name] = pair
        return pair

    def get_model(self, model_name: str) -> tuple[object, object] | None:
        """Return the cached ``(model, tokenizer)`` pair if already loaded."""
        return self._loaded_models.get(model_name)

    def clear_cache(self) -> None:
        """Release all loaded models from memory."""
        self._loaded_models.clear()

    @property
    def device(self) -> str:
        """The compute device used for inference (``cpu``, ``cuda``, ``mps``)."""
        return self._device

    # ------------------------------------------------------------------
    # Device detection
    # ------------------------------------------------------------------

    @staticmethod
    def detect_device() -> str:
        """Auto-detect the best available compute device.

        Priority: CUDA > MPS (Apple Silicon) > CPU.
        Falls back to ``"cpu"`` if *torch* is not installed.
        """
        try:
            import torch  # noqa: PLC0415

            if torch.cuda.is_available():
                return "cuda"
            if torch.backends.mps.is_available():
                return "mps"
        except ImportError:
            pass
        return "cpu"

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _do_load(self, model_name: str) -> tuple[object, object]:
        """Download (via ModelScope) and load a model+tokenizer, then move to device.

        Download flow
        -------------
        1. Try ``modelscope.snapshot_download`` to get a local model path.
           The model is cached under ``self._cache_dir`` (default:
           ``~/.cache/prompt_scanner/models``); subsequent calls return
           immediately from cache.
        2. Load the local path with ``transformers`` ``AutoModelForSequenceClassification``
           and ``AutoTokenizer``.
        3. Move the model to the target device and set ``eval()`` mode.

        Args:
            model_name: ModelScope model ID, e.g.
                ``"LLM-Research/Llama-Prompt-Guard-2-86M"``.

        Raises:
            ModelLoadError: missing deps, download failure, or load failure.
        """
        from agent_sec_cli.prompt_scanner.exceptions import ModelLoadError

        # --- dependency checks -------------------------------------------
        try:
            import torch  # noqa: PLC0415
        except ImportError as exc:
            raise ModelLoadError(
                "torch is required. " "Install with: uv add 'agent-sec-cli[ml]'"
            ) from exc

        try:
            from transformers import (  # noqa: PLC0415
                AutoModelForSequenceClassification,
                AutoTokenizer,
            )
        except ImportError as exc:
            raise ModelLoadError(
                "transformers is required. " "Install with: uv add 'agent-sec-cli[ml]'"
            ) from exc

        try:
            from modelscope import snapshot_download  # noqa: PLC0415
        except ImportError as exc:
            raise ModelLoadError(
                "modelscope is required for model download. Install with: uv sync --extra ml"
            ) from exc

        log.info(
            "Downloading model '%s' via ModelScope (cached after first run).",
            model_name,
        )
        import os  # noqa: PLC0415

        cache_dir = os.path.expanduser(self._cache_dir)
        already_cached = os.path.isdir(
            os.path.join(cache_dir, model_name.replace("/", "---"))
        ) or any(
            model_name.split("/")[-1] in d
            for d in (os.listdir(cache_dir) if os.path.isdir(cache_dir) else [])
        )
        try:
            _ms_logger = logging.getLogger("modelscope")
            _orig_level = _ms_logger.level
            _ms_logger.setLevel(logging.ERROR)
            local_model_path = snapshot_download(model_name, cache_dir=cache_dir)
            _ms_logger.setLevel(_orig_level)
        except Exception as exc:
            raise ModelLoadError(
                f"ModelScope download failed for '{model_name}': {exc}"
            ) from exc

        # --- load from local path ----------------------------------------
        log.info(
            "Loading model from '%s' onto device '%s'.", local_model_path, self._device
        )
        try:
            tokenizer = AutoTokenizer.from_pretrained(local_model_path)
            model = AutoModelForSequenceClassification.from_pretrained(local_model_path)
            model.to(torch.device(self._device))
            model.eval()
        except Exception as exc:
            raise ModelLoadError(
                f"Failed to load model from '{local_model_path}': {exc}"
            ) from exc

        log.info("Model '%s' loaded successfully.", model_name)
        return model, tokenizer
