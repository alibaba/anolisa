#!/usr/bin/env python3
"""Shared hook configuration helpers for Codex hooks."""

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
