"""Audit command — deep verification of the version chain integrity.

Implements ``agent-sec-cli skill-ledger audit <skill_dir> [--verify-snapshots]``:

1. Load all public keys (key.pub + keyring/)
2. Walk versions/ chronologically
3. Verify each manifest's hash, signature, and chain linkage
4. Optionally verify snapshot file hashes
"""

from typing import Any

from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    list_version_ids,
    load_latest_manifest,
    load_version_manifest,
    snapshot_dir_path,
)
from agent_sec_cli.skill_ledger.errors import SignatureInvalidError
from agent_sec_cli.skill_ledger.signing.base import SigningBackend


def audit(
    skill_dir: str,
    backend: SigningBackend,
    verify_snapshots: bool = False,
) -> dict[str, Any]:
    """Perform a deep integrity audit of the version chain.

    Returns ``{"valid": bool, "versions_checked": int, "errors": [...]}``.
    """
    errors: list[dict[str, Any]] = []
    version_ids = list_version_ids(skill_dir)

    if not version_ids:
        return {
            "valid": True,
            "versions_checked": 0,
            "errors": [],
            "message": "No versions found — nothing to audit",
        }

    prev_signature: str | None = None

    for vid in version_ids:
        manifest = load_version_manifest(skill_dir, vid)

        if manifest is None:
            errors.append(
                {"versionId": vid, "error": f"Version file {vid}.json is missing"}
            )
            prev_signature = None
            continue

        # 3a: Verify manifestHash
        expected_hash = manifest.compute_manifest_hash()
        if manifest.manifestHash != expected_hash:
            errors.append(
                {
                    "versionId": vid,
                    "error": "manifestHash does not match manifest content",
                }
            )

        # 3b: Verify signature
        if manifest.signature is not None:
            try:
                backend.verify(
                    manifest.manifestHash.encode("utf-8"),
                    manifest.signature.value,
                    manifest.signature.keyFingerprint,
                )
            except SignatureInvalidError as exc:
                errors.append({"versionId": vid, "error": f"Signature invalid: {exc}"})
        else:
            errors.append({"versionId": vid, "error": "Missing signature"})

        # 3c: Verify previousManifestSignature chain
        if prev_signature is not None:
            if manifest.previousManifestSignature != prev_signature:
                errors.append(
                    {
                        "versionId": vid,
                        "error": (
                            "previousManifestSignature does not match "
                            "the prior version's signature — chain broken"
                        ),
                    }
                )
        else:
            if vid == version_ids[0]:
                # First version: previousManifestSignature should be None
                if manifest.previousManifestSignature is not None:
                    errors.append(
                        {
                            "versionId": vid,
                            "error": "First version should have null previousManifestSignature",
                        }
                    )
            else:
                # Previous version was missing — cannot verify chain linkage
                errors.append(
                    {
                        "versionId": vid,
                        "error": (
                            "Cannot verify previousManifestSignature — "
                            "prior version manifest is missing"
                        ),
                    }
                )

        # 3d: Optional snapshot verification
        if verify_snapshots:
            snap_path = snapshot_dir_path(skill_dir, vid)
            if snap_path.is_dir():
                snap_hashes = compute_file_hashes(str(snap_path))
                diff = diff_file_hashes(manifest.fileHashes, snap_hashes)
                if not diff["match"]:
                    errors.append(
                        {
                            "versionId": vid,
                            "error": (
                                f"Snapshot mismatch — added: {diff['added']}, "
                                f"removed: {diff['removed']}, modified: {diff['modified']}"
                            ),
                        }
                    )
            else:
                errors.append(
                    {
                        "versionId": vid,
                        "error": f"Snapshot directory {vid}.snapshot/ is missing",
                    }
                )

        # Track signature for chain verification
        if manifest.signature is not None:
            prev_signature = manifest.signature.value
        else:
            prev_signature = None

    # Verify latest.json consistency
    latest = load_latest_manifest(skill_dir)
    if latest is not None and version_ids:
        expected_latest_vid = version_ids[-1]
        if latest.versionId != expected_latest_vid:
            errors.append(
                {
                    "versionId": "latest.json",
                    "error": (
                        f"latest.json points to {latest.versionId} "
                        f"but latest version is {expected_latest_vid}"
                    ),
                }
            )

    return {
        "valid": len(errors) == 0,
        "versions_checked": len(version_ids),
        "errors": errors,
    }
