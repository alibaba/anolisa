"""Log path configuration for security events."""

import os
import stat
from pathlib import Path

PRIMARY_LOG_PATH = "/var/log/agent-sec/security-events.jsonl"
FALLBACK_LOG_PATH = str(Path.home() / ".agent-sec-core" / "security-events.jsonl")


def _safe_tmp_log_path() -> str:
    """Return a log path inside a private per-user directory under ``/tmp``.

    Creates ``/tmp/agent-sec-<uid>/`` with mode ``0o700`` and validates
    ownership via :func:`os.lstat` (no symlink following) to prevent
    symlink and file-squatting attacks in the world-writable ``/tmp``.

    Raises :class:`OSError` if the directory cannot be created or passes
    the safety checks.
    """
    uid = os.getuid()
    safe_dir = Path("/tmp") / f"agent-sec-{uid}"

    safe_dir.mkdir(mode=0o700, exist_ok=True)

    # lstat() does NOT follow symlinks — catches attacker-planted links.
    st = safe_dir.lstat()
    if stat.S_ISLNK(st.st_mode):
        raise OSError(f"{safe_dir} is a symlink — refusing to use")
    if st.st_uid != uid:
        raise OSError(f"{safe_dir} not owned by uid {uid}")

    # Tighten permissions in case the directory pre-existed with looser mode.
    safe_dir.chmod(0o700)

    return str(safe_dir / "security-events.jsonl")


def get_log_path() -> str:
    """Return the best available log file path.

    Tries the primary path first (checks that its parent directory exists and
    is writeable).  Falls back to a per-user path under ``~/.agent-sec-core/``.
    Last resort is a private per-user directory under ``/tmp`` with symlink
    and ownership validation.
    Directories are created automatically when they do not exist.
    """
    primary_dir = Path(PRIMARY_LOG_PATH).parent
    try:
        primary_dir.mkdir(parents=True, exist_ok=True)
        if primary_dir.is_dir() and os.access(primary_dir, os.W_OK):
            return PRIMARY_LOG_PATH
    except OSError:
        pass

    fallback_dir = Path(FALLBACK_LOG_PATH).parent
    try:
        fallback_dir.mkdir(parents=True, exist_ok=True)
        return FALLBACK_LOG_PATH
    except OSError:
        pass

    # Last resort: private per-user directory under /tmp with symlink protection
    try:
        return _safe_tmp_log_path()
    except OSError:
        pass

    # Absolute last resort — return the safe path anyway; the writer's _open()
    # will handle the failure gracefully.
    return str(Path("/tmp") / f"agent-sec-{os.getuid()}" / "security-events.jsonl")
