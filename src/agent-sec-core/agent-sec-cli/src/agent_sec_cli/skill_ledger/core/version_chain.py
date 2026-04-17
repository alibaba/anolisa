""".skill-meta/ directory management, version numbering, and snapshot creation.

.. warning::

   This module does **not** provide file-level locking.  If multiple
   processes call :func:`save_manifest` concurrently on the same skill
   directory, the writes may conflict.  Callers in concurrent
   environments should serialise access externally (e.g. ``flock``).
"""

import re
import shutil
from pathlib import Path

from agent_sec_cli.skill_ledger.errors import SkillLedgerError
from agent_sec_cli.skill_ledger.models.manifest import SignedManifest

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SKILL_META_DIR = ".skill-meta"
VERSIONS_DIR = "versions"
LATEST_JSON = "latest.json"

_VERSION_RE = re.compile(r"^v(\d{6})\.json$")

# Directories excluded when creating a snapshot of the skill directory.
_SNAPSHOT_EXCLUDED = frozenset({".skill-meta", ".git"})


# ---------------------------------------------------------------------------
# Path helpers
# ---------------------------------------------------------------------------


def skill_meta_path(skill_dir: str | Path) -> Path:
    return Path(skill_dir) / SKILL_META_DIR


def latest_json_path(skill_dir: str | Path) -> Path:
    return skill_meta_path(skill_dir) / LATEST_JSON


def versions_dir_path(skill_dir: str | Path) -> Path:
    return skill_meta_path(skill_dir) / VERSIONS_DIR


def version_json_path(skill_dir: str | Path, version_id: str) -> Path:
    return versions_dir_path(skill_dir) / f"{version_id}.json"


def snapshot_dir_path(skill_dir: str | Path, version_id: str) -> Path:
    return versions_dir_path(skill_dir) / f"{version_id}.snapshot"


# ---------------------------------------------------------------------------
# Directory initialisation
# ---------------------------------------------------------------------------


def ensure_skill_meta(skill_dir: str | Path) -> Path:
    """Create ``.skill-meta/versions/`` if it does not exist.  Returns the meta path."""
    meta = skill_meta_path(skill_dir)
    meta.mkdir(parents=True, exist_ok=True)
    versions = versions_dir_path(skill_dir)
    versions.mkdir(parents=True, exist_ok=True)
    return meta


# ---------------------------------------------------------------------------
# Version ID management
# ---------------------------------------------------------------------------


def list_version_ids(skill_dir: str | Path) -> list[str]:
    """Return sorted list of existing version IDs (e.g. ``["v000001", "v000002"]``)."""
    vdir = versions_dir_path(skill_dir)
    if not vdir.is_dir():
        return []
    ids: list[str] = []
    for entry in vdir.iterdir():
        m = _VERSION_RE.match(entry.name)
        if m:
            ids.append(f"v{m.group(1)}")
    ids.sort()
    return ids


def next_version_id(skill_dir: str | Path) -> str:
    """Return the next sequential version ID (``v000001`` if none exist).

    Raises :class:`SkillLedgerError` if the maximum version (999999) is reached.
    """
    existing = list_version_ids(skill_dir)
    if not existing:
        return "v000001"
    last = existing[-1]
    num = int(last[1:])
    if num >= 999999:
        raise SkillLedgerError(
            "Version ID overflow — maximum 999999 versions reached for "
            f"{Path(skill_dir).name}"
        )
    return f"v{num + 1:06d}"


# ---------------------------------------------------------------------------
# Manifest I/O
# ---------------------------------------------------------------------------


def load_latest_manifest(skill_dir: str | Path) -> SignedManifest | None:
    """Load ``latest.json`` if it exists, else return ``None``."""
    path = latest_json_path(skill_dir)
    if not path.is_file():
        return None
    return SignedManifest.from_file(str(path))


def save_manifest(
    skill_dir: str | Path,
    manifest: SignedManifest,
    *,
    write_version: bool = True,
) -> None:
    """Write *manifest* to ``versions/<versionId>.json`` and ``latest.json``.

    Both writes are atomic (write-tmp + rename).
    """
    ensure_skill_meta(skill_dir)
    if write_version:
        vpath = version_json_path(skill_dir, manifest.versionId)
        manifest.write_to_file(str(vpath))
    # Always update latest.json
    manifest.write_to_file(str(latest_json_path(skill_dir)))


def load_version_manifest(
    skill_dir: str | Path, version_id: str
) -> SignedManifest | None:
    """Load a specific version manifest, or ``None`` if it does not exist."""
    path = version_json_path(skill_dir, version_id)
    if not path.is_file():
        return None
    return SignedManifest.from_file(str(path))


# ---------------------------------------------------------------------------
# Previous manifest signature extraction
# ---------------------------------------------------------------------------


def get_previous_signature(skill_dir: str | Path) -> str | None:
    """Return the ``signature.value`` of the most recent version, or ``None``."""
    ids = list_version_ids(skill_dir)
    if not ids:
        return None
    last = load_version_manifest(skill_dir, ids[-1])
    if last is None or last.signature is None:
        return None
    return last.signature.value


# ---------------------------------------------------------------------------
# Snapshot
# ---------------------------------------------------------------------------


def create_snapshot(skill_dir: str | Path, version_id: str) -> Path:
    """Copy the skill directory (excluding ``.skill-meta/`` and ``.git/``) into a snapshot.

    Symbolic links are skipped to stay consistent with :func:`compute_file_hashes`
    and to prevent directory-escape attacks.

    Returns the snapshot directory path.
    """
    src = Path(skill_dir).resolve()
    dst = snapshot_dir_path(skill_dir, version_id)
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)

    for entry in sorted(src.rglob("*")):
        if entry.is_symlink():
            continue
        rel = entry.relative_to(src)
        if any(part in _SNAPSHOT_EXCLUDED for part in rel.parts):
            continue
        target = dst / rel
        if entry.is_dir():
            target.mkdir(parents=True, exist_ok=True)
        elif entry.is_file():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(entry, target)

    return dst
