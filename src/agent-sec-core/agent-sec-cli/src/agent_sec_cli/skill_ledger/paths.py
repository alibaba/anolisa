"""Path resolution for skill-ledger configuration and key material.

Provides ``get_key_dir()`` and ``get_config_dir()`` so that every module can
resolve paths without pulling in unrelated dependencies.

The skill-ledger uses the shared skill-security namespace:
``/etc/agent-sec/skill-security/ledger/`` for configuration and
``/etc/agent-sec/skill-security/ledger/keys/`` for signing keys and key history.

Tests and development runs may redirect those paths with explicit
``AGENT_SEC_*`` environment overrides.
"""

import os
from pathlib import Path

from agent_sec_cli.skill_security.paths import config_root

_CONFIG_FILENAME = "config.json"

ENV_LEDGER_CONFIG = "AGENT_SEC_SKILL_LEDGER_CONFIG"
ENV_LEDGER_CONFIG_DIR = "AGENT_SEC_SKILL_LEDGER_CONFIG_DIR"
ENV_LEDGER_KEY_DIR = "AGENT_SEC_SKILL_LEDGER_KEY_DIR"


def _expand(path: str) -> Path:
    return Path(path).expanduser()


def system_config_dir() -> Path:
    """Return the system skill-ledger config directory."""
    return config_root() / "ledger"


def system_key_dir() -> Path:
    """Return the system skill-ledger key directory."""
    return system_config_dir() / "keys"


def get_key_dir() -> Path:
    """Return the skill-ledger key directory.

    System installs use ``/etc/agent-sec/skill-security/ledger/keys/`` unless
    an explicit skill-ledger key directory override is provided.
    """
    explicit = os.environ.get(ENV_LEDGER_KEY_DIR)
    if explicit:
        return _expand(explicit)

    return system_key_dir()


def get_config_dir() -> Path:
    """Return the preferred skill-ledger config directory.

    System installs use ``/etc/agent-sec/skill-security/ledger/`` unless an
    explicit skill-ledger or skill-security config override is provided.
    """
    explicit_file = os.environ.get(ENV_LEDGER_CONFIG)
    if explicit_file:
        return _expand(explicit_file).parent

    explicit_dir = os.environ.get(ENV_LEDGER_CONFIG_DIR)
    if explicit_dir:
        return _expand(explicit_dir)

    return system_config_dir()


def config_search_paths() -> list[Path]:
    """Return config files in merge order."""
    explicit_file = os.environ.get(ENV_LEDGER_CONFIG)
    if explicit_file:
        return [_expand(explicit_file)]

    explicit_dir = os.environ.get(ENV_LEDGER_CONFIG_DIR)
    if explicit_dir:
        return [_expand(explicit_dir) / _CONFIG_FILENAME]

    return [system_config_dir() / _CONFIG_FILENAME]
