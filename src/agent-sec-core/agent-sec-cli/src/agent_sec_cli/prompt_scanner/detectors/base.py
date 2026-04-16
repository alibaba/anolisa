"""Abstract base class for all detection layers."""

from abc import ABC, abstractmethod

from agent_sec_cli.prompt_scanner.result import LayerResult


class DetectionLayer(ABC):
    """Base class that every detector (L1 / L2 / L3) must implement."""

    @property
    @abstractmethod
    def name(self) -> str:
        """Unique identifier for this layer, e.g. 'rule_engine'."""
        pass

    @abstractmethod
    def detect(self, text: str, metadata: dict | None = None) -> LayerResult:
        """Run detection on *text* and return a LayerResult.

        Args:
            text: The (pre-processed) prompt string.
            metadata: Optional context produced by the preprocessor
                      (language, decoded variants, etc.).
        """
        pass

    def is_available(self) -> bool:
        """Return True if all required dependencies are installed.

        Subclasses that rely on optional packages (torch, faiss, …)
        should override this method.
        """
        return True
