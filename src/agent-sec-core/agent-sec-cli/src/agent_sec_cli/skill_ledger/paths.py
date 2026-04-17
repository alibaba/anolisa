"""XDG Base Directory resolution for skill-ledger.

Provides ``get_data_dir()`` and ``get_config_dir()`` so that every module can
resolve paths without pulling in unrelated dependencies.

Testing strategy: set ``XDG_DATA_HOME`` / ``XDG_CONFIG_HOME`` env vars to a
``tempfile.mkdtemp()`` directory — all key and config I/O is automatically
redirected.

TODO: Allow agent-sec-core to override these paths for system-level deployment.
Approach TBD after agent-sec-core path conventions are finalized.
"""

import os
from pathlib import Path

_APP_NAME = "skill-ledger"


def get_data_dir() -> Path:
    """Return the skill-ledger data directory (XDG_DATA_HOME).

    Default: ``~/.local/share/skill-ledger/``
    """
    base = os.environ.get("XDG_DATA_HOME", "")
    if not base:
        base = str(Path.home() / ".local" / "share")
    return Path(base) / _APP_NAME


def get_config_dir() -> Path:
    """Return the skill-ledger config directory (XDG_CONFIG_HOME).

    Default: ``~/.config/skill-ledger/``
    """
    base = os.environ.get("XDG_CONFIG_HOME", "")
    if not base:
        base = str(Path.home() / ".config")
    return Path(base) / _APP_NAME
