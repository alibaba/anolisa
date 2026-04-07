"""Log path configuration for security events."""

import os

PRIMARY_LOG_PATH = "/var/log/agent-sec/security-events.jsonl"
FALLBACK_LOG_PATH = os.path.join(
    os.path.expanduser("~"), ".agent-sec-core", "security-events.jsonl"
)


def get_log_path() -> str:
    """Return the best available log file path.

    Tries the primary path first (checks that its parent directory exists and
    is writeable).  Falls back to a per-user path under ``~/.agent-sec-core/``.
    Directories are created automatically when they do not exist.
    """
    primary_dir = os.path.dirname(PRIMARY_LOG_PATH)
    try:
        os.makedirs(primary_dir, exist_ok=True)
        if os.path.isdir(primary_dir) and os.access(primary_dir, os.W_OK):
            return PRIMARY_LOG_PATH
    except OSError:
        pass

    fallback_dir = os.path.dirname(FALLBACK_LOG_PATH)
    try:
        os.makedirs(fallback_dir, exist_ok=True)
        return FALLBACK_LOG_PATH
    except OSError:
        pass

    # Last resort: write to /tmp
    return os.path.join("/tmp", "agent-sec-security-events.jsonl")
