"""File hashing and diff utilities for skill directories."""

import hashlib
from pathlib import Path
from typing import Any

# Directories to exclude when walking a skill directory.
_EXCLUDED_DIRS = frozenset({".skill-meta", ".git"})


def compute_file_hash(file_path: Path) -> str:
    """Return ``"sha256:<hex>"`` for a single file."""
    sha256 = hashlib.sha256()
    with open(file_path, "rb") as fh:
        for chunk in iter(lambda: fh.read(8192), b""):
            sha256.update(chunk)
    return f"sha256:{sha256.hexdigest()}"


def compute_file_hashes(skill_dir: str | Path) -> dict[str, str]:
    """Walk *skill_dir* and return ``{relative_path: "sha256:<hex>", ...}``.

    Excludes ``.skill-meta/`` and ``.git/`` directories.
    Symbolic links are skipped to avoid cycles and directory escapes.
    Files are sorted by relative path for deterministic ordering.
    """
    root = Path(skill_dir).resolve()
    hashes: dict[str, str] = {}

    for entry in sorted(root.rglob("*")):
        if entry.is_symlink():
            continue
        if not entry.is_file():
            continue
        # Skip excluded directories
        rel = entry.relative_to(root)
        if any(part in _EXCLUDED_DIRS for part in rel.parts):
            continue
        hashes[str(rel)] = compute_file_hash(entry)

    return hashes


def diff_file_hashes(
    stored: dict[str, str],
    current: dict[str, str],
) -> dict[str, Any]:
    """Compare two fileHashes maps and return a structured diff.

    Returns::

        {
            "match": bool,
            "added": ["new_file.py", ...],
            "removed": ["old_file.py", ...],
            "modified": ["changed_file.py", ...],
        }
    """
    stored_keys = set(stored.keys())
    current_keys = set(current.keys())

    added = sorted(current_keys - stored_keys)
    removed = sorted(stored_keys - current_keys)
    modified = sorted(k for k in stored_keys & current_keys if stored[k] != current[k])

    return {
        "match": not added and not removed and not modified,
        "added": added,
        "removed": removed,
        "modified": modified,
    }
