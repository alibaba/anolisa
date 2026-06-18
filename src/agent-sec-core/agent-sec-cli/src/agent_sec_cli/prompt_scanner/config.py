"""Configuration management for prompt scanner."""

import copy
from enum import Enum

from pydantic import BaseModel, Field


class ScanMode(str, Enum):
    """Predefined detection mode presets.

    - FAST:         L1 only.  Latency < 5ms.   Real-time chat scenarios.
    - STANDARD:     L1 + L2.  Latency 20-80ms. Recommended for most production use.
    - STRICT:       L1+L2+L3. Latency 50-200ms. High-security (finance, healthcare).
    - MULTI_TURN:   L4 only.  Multi-turn intent detection over a full
                    conversation triple (history, current query, assistant
                    response).  Calls Ollama via HTTP.  L4 is optional —
                    only invoked when the user explicitly selects this mode.

    Note: L3 (semantic vector search) is planned but not yet implemented.
    STRICT mode is reserved for future use.
    """

    FAST = "fast"
    STANDARD = "standard"
    STRICT = "strict"
    MULTI_TURN = "multi_turn"


class ScanConfig(BaseModel):
    """Full configuration for a PromptScanner instance."""

    # Enabled detector names (ordered)
    layers: list[str] = Field(default_factory=lambda: ["rule_engine", "ml_classifier"])

    # Stop on first positive detection
    fast_fail: bool = True

    # Path to user-supplied custom rules (JSON / YAML)
    custom_rules_path: str | None = None

    # ML model identifier (ModelScope ID)
    model_name: str = "LLM-Research/Llama-Prompt-Guard-2-86M"

    # Compute device for ML inference
    model_device: str = "cpu"

    # Attempt to decode obfuscated encodings (Base64, ROT13, etc.)
    detect_encoding: bool = True

    # L4 multi-turn intent: p_harmful threshold for BLOCK verdict.
    # Only used when "multi_turn_intent" is in layers.
    multi_turn_threshold: float = 0.55


# ---------------------------------------------------------------------------
# Preset configurations
# ---------------------------------------------------------------------------

PRESET_CONFIGS: dict[ScanMode, ScanConfig] = {
    ScanMode.FAST: ScanConfig(
        layers=["rule_engine"],
        fast_fail=True,
    ),
    ScanMode.STANDARD: ScanConfig(
        layers=["rule_engine", "ml_classifier"],
        fast_fail=False,
    ),
    # L3 (semantic) is planned but not yet implemented.
    # STRICT preset is kept as a placeholder for future use.
    ScanMode.STRICT: ScanConfig(
        layers=["rule_engine", "ml_classifier"],
        fast_fail=False,
    ),
    # L4 multi-turn intent detection — runs only the multi_turn_intent
    # detector.  Decoupled from L1-L3 because it consumes a richer input
    # (conversation triple) and delegates to an external Ollama service.
    # L4 is optional: when Ollama is unreachable, the detector is silently
    # skipped and the scan returns PASS.
    ScanMode.MULTI_TURN: ScanConfig(
        layers=["multi_turn_intent"],
        fast_fail=False,
    ),
}


def get_config(mode: ScanMode) -> ScanConfig:
    """Return a *copy* of the preset config for the given mode."""
    if mode not in PRESET_CONFIGS:
        raise ValueError(f"Unknown scan mode: {mode}")
    return copy.deepcopy(PRESET_CONFIGS[mode])
