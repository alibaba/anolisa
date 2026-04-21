"""Check command — the full state machine from design doc §2.

Implements ``agent-sec-cli skill-ledger check <skill_dir>``:

1. Read ``latest.json``
2. Missing → auto-create (sign) → ``{"status": "none"}``
3. Compute current fileHashes, compare
4. Mismatch → ``{"status": "drifted", "added": ..., "removed": ..., "modified": ...}``
5. Match → verify signature → invalid → ``{"status": "tampered", "reason": ...}``
6. Check scanStatus → ``deny`` / ``warn`` / ``none`` / ``pass``
"""

from pathlib import Path
from typing import Any

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
    save_manifest,
)
from agent_sec_cli.skill_ledger.errors import SignatureInvalidError
from agent_sec_cli.skill_ledger.models.manifest import (
    ManifestSignature,
    SignedManifest,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend


def _auto_create_manifest(
    skill_dir: str,
    file_hashes: dict[str, str],
    backend: SigningBackend,
) -> dict[str, Any]:
    """Create an initial manifest when none exists (scanStatus: "none").

    Requires the signing private key (first-time auto-creation).

    If prior versions exist (e.g. latest.json was deleted but versions/ has
    entries), the chain linkage fields are preserved so the audit trail stays
    intact.
    """
    skill_name = Path(skill_dir).name

    # Single traversal of .skill-meta/versions/ to derive all chain fields
    existing_ids = list_version_ids(skill_dir)
    if not existing_ids:
        vid = "v000001"
        prev_vid = None
        prev_sig = None
    else:
        last_num = int(existing_ids[-1][1:])
        if last_num >= 999999:
            from agent_sec_cli.skill_ledger.errors import SkillLedgerError

            raise SkillLedgerError(
                "Version ID overflow — maximum 999999 versions reached for "
                f"{skill_name}"
            )
        vid = f"v{last_num + 1:06d}"
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

    # Compute hash and sign
    manifest.manifestHash = manifest.compute_manifest_hash()
    sig_value, fingerprint = backend.sign(manifest.manifestHash.encode("utf-8"))
    manifest.signature = ManifestSignature(
        algorithm=backend.name,
        value=sig_value,
        keyFingerprint=fingerprint,
    )

    save_manifest(skill_dir, manifest)
    create_snapshot(skill_dir, vid)

    return {"status": "none", "versionId": vid}


def check(skill_dir: str, backend: SigningBackend) -> dict[str, Any]:
    """Execute the full check state machine.

    Returns a JSON-serialisable dict with at minimum ``{"status": "<status>"}``.
    """
    # Step 1: Load latest.json
    # If the file exists but is malformed/corrupted, treat as tampered.
    try:
        manifest = load_latest_manifest(skill_dir)
    except Exception as exc:
        # File exists but cannot be parsed — corrupted or tampered metadata
        if latest_json_path(skill_dir).is_file():
            return {
                "status": "tampered",
                "reason": f"manifest file is corrupted: {exc}",
            }
        # File doesn't exist and some other error — treat as missing
        manifest = None

    # Step 2: Compute current file hashes
    current_hashes = compute_file_hashes(skill_dir)

    # Step 2b: No manifest → auto-create
    if manifest is None:
        return _auto_create_manifest(skill_dir, current_hashes, backend)

    # Step 3: Compare fileHashes (takes priority over signature verification)
    diff = diff_file_hashes(manifest.fileHashes, current_hashes)

    # Step 4: Mismatch → drifted
    if not diff["match"]:
        return {
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
            "status": "tampered",
            "reason": "manifestHash does not match manifest content",
        }

    # 5b: Verify digital signature
    if manifest.signature is None:
        # Legacy manifest without signature — treat as "none" (backward compat)
        return {"status": "none", "reason": "manifest has no signature (legacy)"}

    try:
        backend.verify(
            manifest.manifestHash.encode("utf-8"),
            manifest.signature.value,
            manifest.signature.keyFingerprint,
        )
    except SignatureInvalidError as exc:
        return {"status": "tampered", "reason": str(exc)}

    # Step 6: Signature valid → dispatch on scanStatus
    scan_status = manifest.scanStatus

    if scan_status == "deny":
        findings = _collect_findings(manifest)
        return {"status": "deny", "findings": findings}

    if scan_status == "warn":
        findings = _collect_findings(manifest)
        return {"status": "warn", "findings": findings}

    if scan_status == "none":
        return {"status": "none"}

    # pass (or any other value)
    return {"status": "pass"}


def _collect_findings(manifest: SignedManifest) -> list[dict[str, Any]]:
    """Extract findings from all scans in the manifest."""
    return [f for scan in manifest.scans for f in scan.findings]
