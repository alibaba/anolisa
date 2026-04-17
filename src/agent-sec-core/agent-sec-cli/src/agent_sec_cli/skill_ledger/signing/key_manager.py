"""Key file I/O, XDG path resolution, and passphrase management."""

import getpass
import os
from pathlib import Path

from agent_sec_cli.skill_ledger.errors import (
    KeyAlreadyExistsError,
    KeyNotFoundError,
)
from agent_sec_cli.skill_ledger.paths import get_data_dir

# ---------------------------------------------------------------------------
# Key file paths (data dir resolved via paths.get_data_dir)
# ---------------------------------------------------------------------------


def key_enc_path() -> Path:
    """Path to the encrypted private key file."""
    return get_data_dir() / "key.enc"


def key_pub_path() -> Path:
    """Path to the public key file."""
    return get_data_dir() / "key.pub"


def keyring_dir() -> Path:
    """Path to the trusted public key ring directory."""
    return get_data_dir() / "keyring"


# ---------------------------------------------------------------------------
# Key existence checks
# ---------------------------------------------------------------------------


def keys_exist() -> bool:
    """Return ``True`` if both ``key.enc`` and ``key.pub`` exist."""
    return key_enc_path().is_file() and key_pub_path().is_file()


def ensure_keys_exist() -> None:
    """Raise :class:`KeyNotFoundError` if keys are missing."""
    if not key_enc_path().is_file():
        raise KeyNotFoundError(str(key_enc_path()))
    if not key_pub_path().is_file():
        raise KeyNotFoundError(str(key_pub_path()))


def ensure_keys_not_exist(force: bool = False) -> None:
    """Raise :class:`KeyAlreadyExistsError` unless *force* is ``True``."""
    if keys_exist() and not force:
        raise KeyAlreadyExistsError(str(key_enc_path()))


# ---------------------------------------------------------------------------
# Key file read/write
# ---------------------------------------------------------------------------


def write_key_enc(data: bytes) -> Path:
    """Write encrypted private key bytes.  Creates parent dirs, sets mode 0600."""
    path = key_enc_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(0o600)
    return path


def read_key_enc() -> bytes:
    """Read encrypted private key bytes."""
    path = key_enc_path()
    if not path.is_file():
        raise KeyNotFoundError(str(path))
    return path.read_bytes()


def write_key_pub(data: bytes) -> Path:
    """Write public key bytes.  Creates parent dirs, sets mode 0644."""
    path = key_pub_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(0o644)
    return path


def read_key_pub() -> bytes:
    """Read public key bytes."""
    path = key_pub_path()
    if not path.is_file():
        raise KeyNotFoundError(str(path))
    return path.read_bytes()


def load_keyring_public_keys() -> list[bytes]:
    """Load all ``*.pub`` files from the keyring directory.

    Returns a list of raw public key bytes.  Returns an empty list if the
    keyring directory does not exist.
    """
    kdir = keyring_dir()
    if not kdir.is_dir():
        return []
    keys: list[bytes] = []
    for pub_file in sorted(kdir.glob("*.pub")):
        keys.append(pub_file.read_bytes())
    return keys


# ---------------------------------------------------------------------------
# Passphrase management
# ---------------------------------------------------------------------------

_cached_passphrase: str | None = None


def get_passphrase(
    prompt: str = "Enter passphrase for skill-ledger signing key: ",
) -> str:
    """Obtain the passphrase, trying env var first, then interactive prompt.

    The passphrase is cached in-process for the session lifetime (like ssh-agent).
    """
    global _cached_passphrase  # noqa: PLW0603

    if _cached_passphrase is not None:
        return _cached_passphrase

    # Try environment variable (for CI / non-interactive)
    env_pass = os.environ.get("SKILL_LEDGER_PASSPHRASE")
    if env_pass:
        _cached_passphrase = env_pass
        return _cached_passphrase

    # Interactive prompt
    _cached_passphrase = getpass.getpass(prompt)
    return _cached_passphrase


def clear_passphrase_cache() -> None:
    """Clear the cached passphrase (e.g. after failed decryption)."""
    global _cached_passphrase  # noqa: PLW0603
    _cached_passphrase = None
