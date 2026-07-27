"""Shared manifest integrity helpers for skill-ledger core workflows."""

from agent_sec_cli.skill_ledger.errors import (
    KeyNotFoundError,
    SignatureInvalidError,
)
from agent_sec_cli.skill_ledger.models.manifest import SignedManifest
from agent_sec_cli.skill_ledger.signing.base import SigningBackend

MANIFEST_HASH_MISMATCH_ERROR = "manifestHash does not match manifest content"
MISSING_SIGNATURE_ERROR = "Missing signature"
SIGNATURE_FALSE_ERROR = "signature verification returned false"


def manifest_hash_error(manifest: SignedManifest) -> str | None:
    """Return a diagnostic string when ``manifestHash`` is not self-consistent."""
    if manifest.manifestHash != manifest.compute_manifest_hash():
        return MANIFEST_HASH_MISMATCH_ERROR
    return None


def verify_manifest_signature(
    manifest: SignedManifest,
    backend: SigningBackend,
) -> tuple[bool, str | None]:
    """Verify the manifest signature with strict bool/exception handling."""
    if manifest.signature is None:
        return False, MISSING_SIGNATURE_ERROR

    try:
        verified = backend.verify(
            manifest.manifestHash.encode("utf-8"),
            manifest.signature.value,
            manifest.signature.keyFingerprint,
        )
    except (SignatureInvalidError, KeyNotFoundError) as exc:
        return False, str(exc)

    if verified is not True:
        return False, SIGNATURE_FALSE_ERROR
    return True, None


def verify_manifest_integrity(
    manifest: SignedManifest,
    backend: SigningBackend,
) -> tuple[bool, str | None]:
    """Verify both the manifest self-hash and digital signature."""
    hash_error = manifest_hash_error(manifest)
    if hash_error is not None:
        return False, hash_error
    return verify_manifest_signature(manifest, backend)
