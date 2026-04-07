"""Router — action name to backend module registry with lazy imports."""

from __future__ import annotations

import importlib
from typing import Any, Dict

# ---------------------------------------------------------------------------
# Action → backend module mapping
# ---------------------------------------------------------------------------

_REGISTRY: Dict[str, str] = {
    "sandbox_prehook": "agent_sec_cli.security_middleware.backends.sandbox",
    "harden":          "agent_sec_cli.security_middleware.backends.hardening",
    "verify":          "agent_sec_cli.security_middleware.backends.asset_verify",
    "summary":         "agent_sec_cli.security_middleware.backends.summary",
}

# Module base name → expected class name convention.
_CLASS_SUFFIX = "Backend"

# Cache of already-instantiated backends keyed by action.
_backend_cache: Dict[str, Any] = {}


def _module_to_class_name(module_path: str) -> str:
    """Derive the class name from the last segment of the module path.

    Convention:  ``security_middleware.backends.sandbox``
                 → module name ``sandbox``
                 → class ``SandboxBackend``
    """
    base = module_path.rsplit(".", 1)[-1]
    # Convert snake_case to PascalCase and append "Backend"
    parts = base.split("_")
    pascal = "".join(p.capitalize() for p in parts)
    return f"{pascal}{_CLASS_SUFFIX}"


def get_backend(action: str) -> Any:
    """Return the backend instance responsible for *action*.

    The backend module is imported lazily on first access and the instance is
    cached for subsequent calls.

    Raises:
        ValueError:  If *action* is not found in the registry.
        ImportError:  If the backend module cannot be imported.
        AttributeError:  If the expected class is missing from the module.
    """
    if action not in _REGISTRY:
        registered = ", ".join(sorted(_REGISTRY))
        raise ValueError(
            f"Unknown action {action!r}. Registered actions: {registered}"
        )

    if action in _backend_cache:
        return _backend_cache[action]

    module_path = _REGISTRY[action]
    module = importlib.import_module(module_path)

    class_name = _module_to_class_name(module_path)
    backend_cls = getattr(module, class_name)
    instance = backend_cls()

    _backend_cache[action] = instance
    return instance


def register_action(action: str, module_path: str) -> None:
    """Dynamically register a new action → module mapping.

    This is primarily useful for plugins or tests.
    """
    _REGISTRY[action] = module_path
    # Invalidate any cached instance for this action.
    _backend_cache.pop(action, None)
