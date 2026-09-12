"""Capability registry — config loading, safe wrapping, and registration."""

from __future__ import annotations

import logging
import os
import time
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .hook_config import (  # noqa: TID252 - standalone plugin package
    env_flag_enabled,
)

if TYPE_CHECKING:
    from .capabilities.base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")

_CAPABILITY_ENABLED_ENV = {
    "code-scan": "CODE_SCANNER_HOOK_ENABLED",
    "pii-scan-user-input": "PII_CHECKER_HOOK_ENABLED",
    "skill-ledger": "SKILL_LEDGER_HOOK_ENABLED",
}

# If a single hook invocation exceeds this threshold (seconds), emit a warning.
_SLOW_HOOK_THRESHOLD = 2.0

# ---------------------------------------------------------------------------
# transform_llm_output composition
#
# Hermes applies transform_llm_output hooks with "first hook returning a
# non-empty string wins" semantics (see Hermes agent/turn_finalizer.py).
# Multiple agent-sec-core capabilities (pii-scan, prompt-scan, skill-ledger)
# each register a transform_llm_output hook; under "first-wins", a later
# capability's warning is silently dropped whenever an earlier capability also
# fires in the same turn — a security warning (e.g. "status=tampered") becomes
# invisible to the user. To avoid this, collect all capability transform
# callbacks here and register a single composed hook that chains them, so every
# capability's warning is prepended to the response.
# ---------------------------------------------------------------------------
_TRANSFORM_HOOKS: list[tuple[str, Any]] = []


def _collect_transform_hook(capability_id: str, callback: Any) -> None:
    """Called by AgentSecCoreCapability.register to collect (not register)
    a capability's transform_llm_output callback for composition."""
    _TRANSFORM_HOOKS.append((capability_id, callback))


def _compose_transform_chain(callbacks: list[tuple[str, Any]]):
    """Return a single transform_llm_output callback that chains ``callbacks``.

    Each callback receives the accumulated text as ``response_text`` and
    prepends its own warnings, so the composed result carries every
    capability's warning instead of only the first one to fire.
    """

    def _chain(response_text: str = "", **kwargs: Any) -> str:
        current = response_text
        for _capability_id, callback in callbacks:
            result = callback(response_text=current, **kwargs)
            if isinstance(result, str) and result:
                current = result
        return current

    return _chain


def load_config(plugin_dir: Path) -> dict[str, Any]:
    """Load config.toml from the plugin directory.

    Returns an empty dict on any failure (fail-open).
    """
    config_path = plugin_dir / "config.toml"
    try:
        with open(config_path, "rb") as f:
            return tomllib.load(f)
    except (FileNotFoundError, tomllib.TOMLDecodeError, OSError) as e:
        logger.warning(f"[agent-sec-core] Failed to load config: {e}")
        return {}


def safe_hook_wrapper(callback, capability_id: str):
    """Wrap a hook callback with try/except and performance logging.

    - Catches all exceptions → logs and returns None (fail-open)
    - Logs a warning when execution exceeds _SLOW_HOOK_THRESHOLD
    """

    def wrapper(*args, **kwargs):
        start = time.monotonic()
        try:
            result = callback(*args, **kwargs)
        except Exception as e:
            logger.error(f"[agent-sec-core] {capability_id} hook error: {e}")
            return None
        elapsed = time.monotonic() - start
        if elapsed > _SLOW_HOOK_THRESHOLD:
            logger.warning(
                f"[agent-sec-core] {capability_id} slow hook: {elapsed:.2f}s"
            )
        return result

    return wrapper


def register_capabilities(
    ctx, capabilities: list[AgentSecCoreCapability], config: dict
) -> None:
    """Register all enabled capabilities with the Hermes plugin context."""
    global _TRANSFORM_HOOKS
    _TRANSFORM_HOOKS = []
    if "capabilities" not in config:
        logger.error(
            f"[agent-sec-core] config missing [capabilities] section, no capabilities registered"
        )
        return
    caps_config = config["capabilities"]

    for cap in capabilities:
        if cap.id not in caps_config:
            logger.error(
                f"[agent-sec-core] {cap.id} config section [capabilities.{cap.id}] not found, skipping"
            )
            continue
        cap_config = caps_config[cap.id]
        if "enabled" not in cap_config:
            logger.error(
                f"[agent-sec-core] {cap.id} config missing required key 'enabled', skipping"
            )
            continue
        enabled = bool(cap_config["enabled"])
        enabled_env = _CAPABILITY_ENABLED_ENV.get(cap.id)
        if enabled_env is not None:
            raw_enabled = os.environ.get(enabled_env, "").strip().lower()
            if raw_enabled in {"true", "false"}:
                enabled = env_flag_enabled(enabled_env, enabled)
        if not enabled:
            logger.info(f"[agent-sec-core] {cap.id} disabled by config, skipping")
            continue
        try:
            cap.register(ctx, cap_config)
            logger.info(f"[agent-sec-core] {cap.id} registered successfully")
        except Exception as e:
            logger.error(f"[agent-sec-core] {cap.id} registration failed: {e}")

    # Register a single composed transform_llm_output hook that chains every
    # capability's transform callback, so all security warnings (prompt-scan,
    # pii-scan, skill-ledger) are preserved instead of one silently dropping
    # another under Hermes' "first non-empty wins" semantics.
    if _TRANSFORM_HOOKS:
        composed = _compose_transform_chain(_TRANSFORM_HOOKS)
        ctx.register_hook(
            "transform_llm_output",
            safe_hook_wrapper(composed, "transform-compose"),
        )
        _TRANSFORM_HOOKS = []
