"""Core scanner – orchestrates the multi-layer detection pipeline."""

import asyncio
import logging
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any

from agent_sec_cli.prompt_scanner.config import ScanConfig, ScanMode, get_config
from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.detectors.ml_classifier import MLClassifier
from agent_sec_cli.prompt_scanner.detectors.rule_engine import RuleEngine
from agent_sec_cli.prompt_scanner.exceptions import (
    LayerNotAvailableError,
    ScannerInputError,
)
from agent_sec_cli.prompt_scanner.preprocessor import Preprocessor
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    ScanResult,
    ThreatType,
    Verdict,
)
from agent_sec_cli.prompt_scanner.verdict import determine_verdict

log = logging.getLogger(__name__)

# Registry: detector name -> class
# Note: L3 "semantic" layer is planned but not yet implemented.
_DETECTOR_REGISTRY: dict[str, type[DetectionLayer]] = {
    "rule_engine": RuleEngine,
    "ml_classifier": MLClassifier,
}

# Detectors that can be skipped silently when unavailable.
# L1 (rule_engine) and L2 (ml_classifier) are mandatory — their deps ship
# with the package.  Only future optional layers (e.g. L3 semantic) go here.
_OPTIONAL_DETECTORS = frozenset({"semantic"})


class PromptScanner:
    """Main entry point for prompt scanning.

    Usage::

        scanner = PromptScanner()                        # default: STANDARD
        result  = scanner.scan("ignore previous instructions")
        result.is_threat   # True / False
        result.risk_score  # 0.0 – 1.0

        # Or pick a preset mode
        scanner = PromptScanner(mode=ScanMode.FAST)      # L1 only
        scanner = PromptScanner(mode=ScanMode.STRICT)    # L1+L2 (L3 planned)

        # Or provide a fully custom config
        scanner = PromptScanner(config=ScanConfig(layers=["rule_engine"], threshold=0.3))
    """

    def __init__(
        self,
        mode: ScanMode = ScanMode.STANDARD,
        config: ScanConfig | None = None,
    ) -> None:
        self._config = config if config is not None else get_config(mode)
        self._preprocessor = Preprocessor(detect_encoding=self._config.detect_encoding)
        self._detectors: list[DetectionLayer] = self._init_detectors()

    def warmup(self) -> None:
        """Pre-download all ML models so the first scan avoids a network fetch.

        Only downloads model files to the local cache with **visible**
        progress output.  Does **not** load models into memory — that
        happens lazily on the first ``scan()`` call via ``load_model``
        (silently, with ``_silent_ml_loading``).

        Call this once after installation::

            agent-sec-cli scan-prompt warmup
        """
        log.info("Warming up %d detector(s)...", len(self._detectors))
        for detector in self._detectors:
            if hasattr(detector, "warmup"):
                detector.warmup()
        log.info("Warmup complete.")

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def scan(self, text: str, source: str | None = None) -> ScanResult:
        """Scan a single prompt text through the detection pipeline.

        Args:
            text:   Raw prompt string.
            source: Optional label for the input origin (e.g. "user_input").

        Returns:
            A fully populated ScanResult.

        Raises:
            ScannerInputError: If *text* is empty after stripping.
        """
        text = text.strip()
        if not text:
            raise ScannerInputError("Input text must not be empty.")
        t0 = time.perf_counter()

        # 1. Preprocess
        prep = self._preprocessor.preprocess(text)
        metadata: dict[str, Any] = prep.metadata
        if source:
            metadata["source"] = source
        # Pass decoded variants to detectors for scanning
        if prep.decoded_variants:
            metadata["decoded_variants"] = prep.decoded_variants

        # 2. Run detectors
        layer_results: list[LayerResult] = []
        for detector in self._detectors:
            lr = detector.detect(prep.normalized_text, metadata)
            layer_results.append(lr)
            if self._config.fast_fail and lr.detected:
                break

        # 3. Verdict
        verdict = determine_verdict(layer_results)

        # 4. Determine threat type
        threat_type = self._determine_threat_type(layer_results)
        is_threat = verdict in (Verdict.WARN, Verdict.DENY)

        elapsed = (time.perf_counter() - t0) * 1000  # ms

        return ScanResult(
            is_threat=is_threat,
            threat_type=threat_type,
            layer_results=layer_results,
            latency_ms=elapsed,
            metadata=metadata,
            verdict=verdict,
        )

    def scan_batch(
        self,
        texts: list[str],
        max_workers: int = 4,
    ) -> list[ScanResult]:
        """Scan multiple prompts, choosing the best execution strategy.

        Strategy selection:
        - **FAST mode** (L1 regex only): Uses a ``ThreadPoolExecutor`` for
          modest I/O parallelism.  Regex detection is thread-safe and
          benefits from concurrent execution.
        - **STANDARD / STRICT mode** (L1 + L2 ML): Iterates sequentially.
          The ML tokenizer (Rust-backed HuggingFace) is NOT thread-safe
          and must be serialised with a lock, so a thread pool would only
          add overhead (thread creation, context switching, lock contention)
          without any throughput gain.

        Args:
            texts:       List of raw prompt strings.
            max_workers: Maximum thread pool size (only used in FAST mode).

        Returns:
            List of ScanResults in the same order as *texts*.
        """
        if not texts:
            return []
        if len(texts) == 1:
            return [self.scan(texts[0])]

        has_ml = any(isinstance(d, MLClassifier) for d in self._detectors)
        if has_ml:
            # Why sequential?  The HuggingFace tokenizer (Rust-backed,
            # uses RefCell internally) is NOT thread-safe.  To prevent
            # "Already borrowed" panics, PromptGuardClassifier serialises
            # all inference behind _inference_lock.  With that lock in
            # place a ThreadPoolExecutor only adds thread-creation and
            # context-switch overhead while every worker queues on the
            # same lock — net effect is *slower* than a plain loop.
            return [self.scan(t) for t in texts]

        # FAST mode (L1 only): pure-regex detectors are thread-safe.
        with ThreadPoolExecutor(max_workers=min(max_workers, len(texts))) as pool:
            futures = [pool.submit(self.scan, t) for t in texts]
            return [f.result() for f in futures]

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _init_detectors(self) -> list[DetectionLayer]:
        """Instantiate detectors listed in config.layers.

        Mandatory detectors (rule_engine, ml_classifier) raise immediately
        if unavailable.  Optional detectors (semantic) log a warning and
        are skipped — this allows the scanner to degrade gracefully when
        future optional layers are not yet installed.
        """
        detectors: list[DetectionLayer] = []
        for name in self._config.layers:
            cls = _DETECTOR_REGISTRY.get(name)
            if cls is None:
                raise ValueError(f"Unknown detector: {name}")
            # Pass model-specific config to detectors that accept it.
            if name == "ml_classifier":
                detector = cls(model_name=self._config.model_name)
            else:
                detector = cls()
            if not detector.is_available():
                if name in _OPTIONAL_DETECTORS:
                    log.warning(
                        "Detector '%s' is not available (missing dependencies) "
                        "and will be skipped.",
                        name,
                    )
                    continue
                raise LayerNotAvailableError(
                    f"Detector '{name}' is not available. "
                    "Check that its dependencies are installed."
                )
            detectors.append(detector)
        return detectors

    @staticmethod
    def _determine_threat_type(layer_results: list[LayerResult]) -> ThreatType:
        """Infer the primary threat type from layer results."""
        for lr in layer_results:
            if lr.detected:
                for detail in lr.details:
                    if detail.category == "jailbreak":
                        return ThreatType.JAILBREAK
                    if detail.category == "direct_injection":
                        return ThreatType.DIRECT_INJECTION
                    if detail.category == "indirect_injection":
                        return ThreatType.INDIRECT_INJECTION
                    if detail.category == "injection":
                        return ThreatType.DIRECT_INJECTION
                # Default to direct_injection if category not explicit
                return ThreatType.DIRECT_INJECTION
        return ThreatType.BENIGN


class AsyncPromptScanner:
    """Async wrapper around PromptScanner for asyncio-based applications.

    ML inference and other CPU-bound work is offloaded to a thread pool
    via ``loop.run_in_executor()``.

    Usage::

        scanner = AsyncPromptScanner(mode=ScanMode.STANDARD)
        result  = await scanner.scan(text)
        results = await scanner.scan_batch(texts)
    """

    def __init__(
        self,
        mode: ScanMode = ScanMode.STANDARD,
        config: ScanConfig | None = None,
    ) -> None:
        self._sync_scanner = PromptScanner(mode=mode, config=config)

    async def scan(self, text: str, source: str | None = None) -> ScanResult:
        """Async scan – offloads to thread pool."""
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(None, self._sync_scanner.scan, text, source)

    async def scan_batch(self, texts: list[str]) -> list[ScanResult]:
        """Async batch scan."""
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(None, self._sync_scanner.scan_batch, texts)
