"""Shared filesystem roots for skill security configuration."""

import os
from pathlib import Path

SYSTEM_CONFIG_ROOT = Path("/etc/agent-sec/skill-security")

ENV_CONFIG_ROOT = "AGENT_SEC_SKILL_SECURITY_DIR"


def _expand(path: str) -> Path:
    return Path(path).expanduser()


def config_root() -> Path:
    """Return the skill-security configuration root."""
    override = os.environ.get(ENV_CONFIG_ROOT)
    if override:
        return _expand(override)
    return SYSTEM_CONFIG_ROOT
