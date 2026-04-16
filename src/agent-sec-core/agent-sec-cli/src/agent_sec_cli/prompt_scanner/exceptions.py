"""Custom exceptions for prompt_scanner module."""


class PromptScannerError(Exception):
    """Base exception for all prompt scanner errors."""

    pass


class LayerNotAvailableError(PromptScannerError):
    """Raised when a detection layer's dependencies are not installed.

    Example:
        raise LayerNotAvailableError(
            "ML classifier requires torch and transformers. "
            "Install with: pip install prompt-scanner[ml]"
        )
    """

    pass


class ModelLoadError(PromptScannerError):
    """Raised when a model fails to load (download, corrupt, incompatible)."""

    pass


class ConfigError(PromptScannerError):
    """Raised when scanner configuration is invalid."""

    pass
