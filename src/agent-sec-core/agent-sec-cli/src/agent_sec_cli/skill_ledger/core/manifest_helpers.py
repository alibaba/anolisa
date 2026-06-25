"""Shared helpers for trusted Skill Ledger manifest handling."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_snapshot_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.version_chain import load_latest_manifest
from agent_sec_cli.skill_ledger.models.manifest import (
    SignedManifest,
    UserDecision,
)


def safe_load_latest_manifest(skill_dir: str | Path) -> SignedManifest | None:
    """Load latest manifest, treating malformed metadata as unavailable."""
    try:
        return load_latest_manifest(skill_dir)
    except (json.JSONDecodeError, ValueError):
        return None


def snapshot_matches_manifest(snapshot: str | Path, manifest: SignedManifest) -> bool:
    """Return whether a snapshot's strict file hashes match its manifest."""
    try:
        snapshot_hashes = compute_snapshot_file_hashes(snapshot)
    except ValueError:
        return False
    return bool(diff_file_hashes(manifest.fileHashes, snapshot_hashes)["match"])


def user_decision_to_dict(decision: UserDecision | None) -> dict[str, Any] | None:
    """Return a JSON payload for a user decision."""
    if decision is None:
        return None
    return decision.model_dump(exclude_none=True)
