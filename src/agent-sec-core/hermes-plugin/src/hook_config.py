"""Shared hook configuration helpers for Hermes capabilities."""

from __future__ import annotations

import os


def env_flag_enabled(name: str, default: bool = True) -> bool:
    """Read a strict true/false environment flag."""
    value = os.environ.get(name)
    if value is None:
        return default
    normalized = value.strip().lower()
    if normalized == "true":
        return True
    if normalized == "false":
        return False
    return default
