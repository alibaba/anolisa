"""Check command — the full state machine from design doc §2.

Implements ``agent-sec-cli skill-ledger check <skill_dir>``:

1. Read ``latest.json``
2. Missing → ``{"status": "none"}``
3. Manifest present → compute current fileHashes, compare
4. Mismatch → ``{"status": "drifted", "added": ..., "removed": ..., "modified": ...}``
5. Match → verify signature → invalid → ``{"status": "tampered", "reason": ...}``
6. Check scanStatus → ``deny`` / ``warn`` / ``none`` / ``pass``
"""

import json
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.live_root import (
    ResolvedSkillRoot,
    SkillRootInput,
    canonical_skill_operation,
    resolve_skill_root,
    validate_resolved_skill_root,
)
from agent_sec_cli.skill_ledger.core.manifest_helpers import (
    snapshot_matches_manifest,
)
from agent_sec_cli.skill_ledger.core.manifest_integrity import (
    MISSING_SIGNATURE_ERROR,
    manifest_hash_error,
    verify_manifest_signature,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    latest_json_path,
    load_latest_manifest,
    snapshot_dir_path,
)
from agent_sec_cli.skill_ledger.models.manifest import (
    SignedManifest,
)
from agent_sec_cli.skill_ledger.path_identity import (
    normalize_canonical_skill_dir,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend


def _manifest_metadata(
    manifest: SignedManifest,
    root: ResolvedSkillRoot,
) -> dict[str, Any]:
    """Return standard metadata fields extracted from a loaded manifest.

    These fields are included in every ``check`` / ``check --all`` return dict
    so that consumers (Agent, plugin, ``status`` command) never need to read
    ``.skill-meta/latest.json`` directly.
    """
    return {
        "canonicalSkillDir": str(root.canonical_dir),
        "skillName": root.skill_name,
        "versionId": manifest.versionId,
        "createdAt": manifest.createdAt,
        "updatedAt": manifest.updatedAt,
        "fileCount": len(manifest.fileHashes),
        "manifestHash": manifest.manifestHash,
        "userDecision": (
            manifest.userDecision.model_dump(exclude_none=True)
            if manifest.userDecision is not None
            else None
        ),
    }


@canonical_skill_operation
def check(skill_dir: SkillRootInput, backend: SigningBackend) -> dict[str, Any]:
    """Execute the full check state machine.

    Returns a JSON-serialisable dict with at minimum ``{"status": "<status>"}``.
    When a manifest is available the dict also includes standard metadata:
    ``skillName``, ``versionId``, ``createdAt``, ``updatedAt``, ``fileCount``,
    ``manifestHash``.
    """
    # Step 0: Resolve once, then keep identity separate from filesystem I/O.
    root = resolve_skill_root(skill_dir)
    validate_resolved_skill_root(root)
    io_skill_dir = str(root.io_dir)

    # Step 1: Load latest.json
    # If the file exists but is malformed/corrupted, treat as tampered.
    try:
        manifest = load_latest_manifest(io_skill_dir)
    except (json.JSONDecodeError, ValueError) as exc:
        # File exists but cannot be parsed — corrupted or tampered metadata
        if latest_json_path(io_skill_dir).is_file():
            return {
                "status": "tampered",
                "canonicalSkillDir": str(root.canonical_dir),
                "skillName": root.skill_name,
                "versionId": None,
                "createdAt": None,
                "updatedAt": None,
                "fileCount": None,
                "manifestHash": None,
                "reason": f"manifest file is corrupted: {exc}",
            }
        # File doesn't exist and some other error — treat as missing
        manifest = None

    # Step 2: No manifest → read-only none.  scan/certify are the only
    # commands that create signed versions and snapshots.
    if manifest is None:
        return {
            "status": "none",
            "canonicalSkillDir": str(root.canonical_dir),
            "skillName": root.skill_name,
            "versionId": None,
            "createdAt": None,
            "updatedAt": None,
            "fileCount": None,
            "manifestHash": None,
        }

    # Step 3: Compute current file hashes
    current_hashes = compute_file_hashes(io_skill_dir)

    # Manifest loaded — compute standard metadata for all subsequent returns
    meta = _manifest_metadata(manifest, root)

    # Step 4: Compare fileHashes (takes priority over signature verification)
    diff = diff_file_hashes(manifest.fileHashes, current_hashes)

    # Step 5: Mismatch → drifted
    if not diff["match"]:
        return {
            **meta,
            "status": "drifted",
            "added": diff["added"],
            "removed": diff["removed"],
            "modified": diff["modified"],
        }

    # Step 6: fileHashes match → verify signature
    # 6a: Recompute manifestHash
    hash_error = manifest_hash_error(manifest)
    if hash_error is not None:
        return {
            **meta,
            "status": "tampered",
            "reason": hash_error,
        }

    # 6b: Verify digital signature
    signature_valid, signature_error = verify_manifest_signature(manifest, backend)
    if not signature_valid and signature_error == MISSING_SIGNATURE_ERROR:
        # Legacy manifest without signature — treat as "none" (backward compat)
        return {
            **meta,
            "status": "none",
            "reason": "manifest has no signature (legacy)",
        }
    if not signature_valid:
        return {**meta, "status": "tampered", "reason": signature_error}

    # Step 7: Signature valid → dispatch on scanStatus
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


@canonical_skill_operation
def manifest_only_status(
    skill_dir: SkillRootInput,
    backend: SigningBackend,
) -> dict[str, Any]:
    """Return latest trusted manifest status without hashing root files."""
    root = resolve_skill_root(skill_dir)
    validate_resolved_skill_root(root)
    io_skill_dir = str(root.io_dir)
    try:
        manifest = load_latest_manifest(io_skill_dir)
    except (json.JSONDecodeError, ValueError) as exc:
        if latest_json_path(io_skill_dir).is_file():
            return {
                "status": "tampered",
                "canonicalSkillDir": str(root.canonical_dir),
                "skillName": root.skill_name,
                "versionId": None,
                "createdAt": None,
                "updatedAt": None,
                "fileCount": None,
                "manifestHash": None,
                "reason": f"manifest file is corrupted: {exc}",
            }
        manifest = None
    if manifest is None:
        return {
            "status": "none",
            "canonicalSkillDir": str(root.canonical_dir),
            "skillName": root.skill_name,
            "versionId": None,
            "createdAt": None,
            "updatedAt": None,
            "fileCount": None,
            "manifestHash": None,
        }

    meta = _manifest_metadata(manifest, root)
    hash_error = manifest_hash_error(manifest)
    if hash_error is not None:
        return {**meta, "status": "tampered", "reason": hash_error}

    signature_valid, signature_error = verify_manifest_signature(manifest, backend)
    if not signature_valid and signature_error == MISSING_SIGNATURE_ERROR:
        return {
            **meta,
            "status": "none",
            "reason": "manifest has no signature (legacy)",
        }
    if not signature_valid:
        return {**meta, "status": "tampered", "reason": signature_error}

    if not snapshot_matches_manifest(
        snapshot_dir_path(io_skill_dir, manifest.versionId),
        manifest,
    ):
        return {
            **meta,
            "status": "tampered",
            "reason": "snapshot does not match manifest",
        }

    if manifest.scanStatus in {"deny", "warn"}:
        return {
            **meta,
            "status": manifest.scanStatus,
            "findings": _collect_findings(manifest),
        }
    if manifest.scanStatus == "none":
        return {**meta, "status": "none"}
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
            canonical_dir = normalize_canonical_skill_dir(skill_dir)
            results.append(
                {
                    "canonicalSkillDir": str(canonical_dir),
                    "skillName": canonical_dir.name,
                    "status": "error",
                    "error": str(exc),
                }
            )
    return results
