"""Configuration management for prompt scanner."""

import copy
from enum import Enum

from pydantic import BaseModel, Field


class ScanMode(str, Enum):
    """Predefined detection mode presets.

    - FAST:     L1 only.  Latency < 5ms.   Real-time chat scenarios.
    - STANDARD: L1 + L2.  Latency 20-80ms. Recommended for most production use.
    - STRICT:   L1+L2+L3. Latency 50-200ms. High-security (finance, healthcare).

    Note: L3 (semantic vector search) is planned but not yet implemented.
    STRICT mode is reserved for future use.
    """

    FAST = "fast"
    STANDARD = "standard"
    STRICT = "strict"


class ScanConfig(BaseModel):
    """Full configuration for a PromptScanner instance."""

    # Enabled detector names (ordered)
    layers: list[str] = Field(default_factory=lambda: ["rule_engine", "ml_classifier"])

    # Stop on first positive detection
    fast_fail: bool = True

    # Risk-score threshold for triggering a threat verdict
    threshold: float = 0.5

    # Path to user-supplied custom rules (JSON / YAML)
    custom_rules_path: str | None = None

    # ML model identifier
    model_name: str = "prompt-guard-2-86m"

    # Compute device for ML inference
    model_device: str = "cpu"

    # Attempt to decode obfuscated encodings (Base64, ROT13, etc.)
    detect_encoding: bool = True


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
        fast_fail=True,
    ),
    # L3 (semantic) is planned but not yet implemented.
    # STRICT preset is kept as a placeholder for future use.
    ScanMode.STRICT: ScanConfig(
        layers=["rule_engine", "ml_classifier"],
        fast_fail=False,
    ),
}


def get_config(mode: ScanMode) -> ScanConfig:
    """Return a *copy* of the preset config for the given mode."""
    if mode not in PRESET_CONFIGS:
        raise ValueError(f"Unknown scan mode: {mode}")
    return copy.deepcopy(PRESET_CONFIGS[mode])
