"""Check command — the full state machine from design doc §2.

Implements ``agent-sec-cli skill-ledger check <skill_dir>``:

1. Read ``latest.json``
2. Missing → auto-create (unsigned baseline) → ``{"status": "none"}``
3. Compute current fileHashes, compare
4. Mismatch → ``{"status": "drifted", "added": ..., "removed": ..., "modified": ...}``
5. Match → verify signature → invalid → ``{"status": "tampered", "reason": ...}``
6. Check scanStatus → ``deny`` / ``warn`` / ``none`` / ``pass``
"""

import json
import logging
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.config import remember_skill_dir
from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    create_snapshot,
    latest_json_path,
    list_version_ids,
    load_latest_manifest,
    load_version_manifest,
    next_version_id,
    save_manifest,
)
from agent_sec_cli.skill_ledger.errors import SignatureInvalidError
from agent_sec_cli.skill_ledger.models.manifest import (
    SignedManifest,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from agent_sec_cli.skill_ledger.utils import utc_now_iso, validate_skill_dir

logger = logging.getLogger(__name__)


def _manifest_metadata(manifest: SignedManifest, skill_dir: str) -> dict[str, Any]:
    """Return standard metadata fields extracted from a loaded manifest.

    These fields are included in every ``check`` / ``check --all`` return dict
    so that consumers (Agent, plugin, ``status`` command) never need to read
    ``.skill-meta/latest.json`` directly.
    """
    return {
        "skillName": Path(skill_dir).name,
        "versionId": manifest.versionId,
        "createdAt": manifest.createdAt,
        "updatedAt": manifest.updatedAt,
        "fileCount": len(manifest.fileHashes),
        "manifestHash": manifest.manifestHash,
    }


def _auto_create_manifest(
    skill_dir: str,
    file_hashes: dict[str, str],
) -> SignedManifest:
    """Create an unsigned baseline manifest when none exists.

    The manifest records file hashes for drift detection but is **not signed**.
    Signing is deferred to ``certify``, which is always run interactively.
    This avoids requiring the private key (and thus a passphrase) during
    ``check``, which may run in a non-interactive hook context.

    If prior versions exist (e.g. latest.json was deleted but versions/ has
    entries), the chain linkage fields are preserved so the audit trail stays
    intact.

    Returns the persisted :class:`SignedManifest` instance.  The caller is
    responsible for constructing the result dict.
    """
    skill_name = Path(skill_dir).name

    # Single traversal of .skill-meta/versions/ to derive all chain fields
    existing_ids = list_version_ids(skill_dir)
    if not existing_ids:
        vid = "v000001"
        prev_vid = None
        prev_sig = None
    else:
        vid = next_version_id(skill_dir)
        prev_vid = existing_ids[-1]
        last_manifest = load_version_manifest(skill_dir, prev_vid)
        prev_sig = (
            last_manifest.signature.value
            if last_manifest is not None and last_manifest.signature is not None
            else None
        )

    manifest = SignedManifest(
        versionId=vid,
        previousVersionId=prev_vid,
        skillName=skill_name,
        fileHashes=file_hashes,
        scanStatus="none",
        previousManifestSignature=prev_sig,
    )

    # Stamp the last-modified time and compute content hash (for integrity).
    # Signature is left as None — signing is deferred to ``certify``.
    manifest.updatedAt = utc_now_iso()
    manifest.manifestHash = manifest.compute_manifest_hash()

    save_manifest(skill_dir, manifest)
    create_snapshot(skill_dir, vid)

    return manifest


def check(skill_dir: str, backend: SigningBackend) -> dict[str, Any]:
    """Execute the full check state machine.

    Returns a JSON-serialisable dict with at minimum ``{"status": "<status>"}``.
    When a manifest is available the dict also includes standard metadata:
    ``skillName``, ``versionId``, ``createdAt``, ``updatedAt``, ``fileCount``,
    ``manifestHash``.
    """
    # Step 0: Validate skill directory
    validate_skill_dir(skill_dir)
    skill_name = Path(skill_dir).name

    # Auto-remember: append to skillDirs if not already covered (best-effort)
    try:
        remember_skill_dir(Path(skill_dir))
    except Exception:
        logger.debug(
            "auto-remember failed for %s, continuing", skill_dir, exc_info=True
        )

    # Step 1: Load latest.json
    # If the file exists but is malformed/corrupted, treat as tampered.
    try:
        manifest = load_latest_manifest(skill_dir)
    except (json.JSONDecodeError, ValueError) as exc:
        # File exists but cannot be parsed — corrupted or tampered metadata
        if latest_json_path(skill_dir).is_file():
            return {
                "status": "tampered",
                "skillName": skill_name,
                "versionId": None,
                "createdAt": None,
                "updatedAt": None,
                "fileCount": None,
                "manifestHash": None,
                "reason": f"manifest file is corrupted: {exc}",
            }
        # File doesn't exist and some other error — treat as missing
        manifest = None

    # Step 2: Compute current file hashes
    current_hashes = compute_file_hashes(skill_dir)

    # Step 2b: No manifest → auto-create unsigned baseline
    if manifest is None:
        manifest = _auto_create_manifest(skill_dir, current_hashes)
        return {"status": "none", **_manifest_metadata(manifest, skill_dir)}

    # Manifest loaded — compute standard metadata for all subsequent returns
    meta = _manifest_metadata(manifest, skill_dir)

    # Step 3: Compare fileHashes (takes priority over signature verification)
    diff = diff_file_hashes(manifest.fileHashes, current_hashes)

    # Step 4: Mismatch → drifted
    if not diff["match"]:
        return {
            **meta,
            "status": "drifted",
            "added": diff["added"],
            "removed": diff["removed"],
            "modified": diff["modified"],
        }

    # Step 5: fileHashes match → verify signature
    # 5a: Recompute manifestHash
    expected_hash = manifest.compute_manifest_hash()
    if manifest.manifestHash != expected_hash:
        return {
            **meta,
            "status": "tampered",
            "reason": "manifestHash does not match manifest content",
        }

    # 5b: Verify digital signature
    if manifest.signature is None:
        # Legacy manifest without signature — treat as "none" (backward compat)
        return {
            **meta,
            "status": "none",
            "reason": "manifest has no signature (legacy)",
        }

    try:
        backend.verify(
            manifest.manifestHash.encode("utf-8"),
            manifest.signature.value,
            manifest.signature.keyFingerprint,
        )
    except SignatureInvalidError as exc:
        return {**meta, "status": "tampered", "reason": str(exc)}

    # Step 6: Signature valid → dispatch on scanStatus
    scan_status = manifest.scanStatus

    if scan_status == "deny":
        findings = _collect_findings(manifest)
        return {**meta, "status": "deny", "findings": findings}

    if scan_status == "warn":
        findings = _collect_findings(manifest)
        return {**meta, "status": "warn", "findings": findings}

    if scan_status == "none":
        return {**meta, "status": "none"}

    # pass (or any other value)
    return {**meta, "status": "pass"}


def _collect_findings(manifest: SignedManifest) -> list[dict[str, Any]]:
    """Extract findings from all scans in the manifest."""
    return [f for scan in manifest.scans for f in scan.findings]


def check_batch(
    skill_dirs: list[Path],
    backend: SigningBackend,
) -> list[dict[str, Any]]:
    """Check multiple skill directories and return a list of per-skill results.

    Each entry is the enriched dict returned by :func:`check`.  On per-skill
    errors the entry contains ``{"skillName": ..., "status": "error", ...}``
    so that callers always receive one result per input directory.
    """
    results: list[dict[str, Any]] = []
    for skill_dir in skill_dirs:
        try:
            result = check(str(skill_dir), backend)
            results.append(result)
        except Exception as exc:
            results.append(
                {
                    "skillName": skill_dir.name,
                    "status": "error",
                    "error": str(exc),
                }
            )
    return results
