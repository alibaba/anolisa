"""Shared model service client for all scanners.

Provides a unified interface to local model inference backends (Ollama, vLLM, etc.).
Each scanner instantiates a client via ``create_client()`` and passes its own model name.
"""

import os
from typing import Any

from agent_sec_cli.model_service.base import ModelServiceClient

__all__ = ["ModelServiceClient", "create_client"]

# ---------------------------------------------------------------------------
# Environment variables for default configuration
# ---------------------------------------------------------------------------

_ENV_BACKEND = "AGENT_SEC_MODEL_SERVICE_BACKEND"
_ENV_BASE_URL = "AGENT_SEC_MODEL_SERVICE_BASE_URL"
_ENV_TIMEOUT = "AGENT_SEC_MODEL_SERVICE_TIMEOUT"

_DEFAULT_BACKEND = "ollama"
_DEFAULT_BASE_URL = "http://localhost:11434"
_DEFAULT_TIMEOUT = 30


def create_client(
    backend: str | None = None,
    base_url: str | None = None,
    timeout: int | None = None,
    **kwargs: Any,
) -> ModelServiceClient:
    """Create a model service client instance.

    Parameters are resolved in order: explicit argument > environment variable > default.
    """
    resolved_backend = (
        backend
        if backend is not None
        else os.environ.get(_ENV_BACKEND, _DEFAULT_BACKEND)
    )
    resolved_base_url = (
        base_url
        if base_url is not None
        else os.environ.get(_ENV_BASE_URL, _DEFAULT_BASE_URL)
    )
    try:
        resolved_timeout = (
            timeout
            if timeout is not None
            else int(os.environ.get(_ENV_TIMEOUT, str(_DEFAULT_TIMEOUT)))
        )
    except ValueError:
        resolved_timeout = _DEFAULT_TIMEOUT

    if resolved_backend == "ollama":
        from agent_sec_cli.model_service.ollama import (  # noqa: PLC0415
            OllamaClient,
        )

        return OllamaClient(
            base_url=resolved_base_url, timeout=resolved_timeout, **kwargs
        )

    raise ValueError(f"Unsupported model service backend: {resolved_backend!r}")
