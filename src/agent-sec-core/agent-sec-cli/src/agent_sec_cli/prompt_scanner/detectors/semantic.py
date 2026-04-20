"""L3 Semantic detector – vector similarity search against known attack patterns.

This module is a placeholder.  The semantic detection layer (L3) will be
implemented in a future commit.
"""

from typing import Any

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.result import LayerResult


class SemanticDetector(DetectionLayer):
    """L3 detection layer: semantic similarity search.  (not yet implemented)

    Will embed the input and compare against a curated library of known
    attack pattern vectors (PINT, AdvBench, HarmBench, DAN/AIM templates).
    """

    @property
    def name(self) -> str:
        return "semantic"

    def is_available(self) -> bool:
        # L3 is not yet implemented.
        return False

    def detect(self, text: str, metadata: dict[str, Any] | None = None) -> LayerResult:
        """Not implemented – reserved for future semantic prompt guard."""
        raise NotImplementedError(
            "Semantic detection layer (L3) is not yet implemented. "
            "Use ScanMode.STANDARD (L1+L2) for now."
        )
