"""Asset verification module for skill integrity checking."""

from .errors import (
    ErrConfigMissing,
    ErrHashMismatch,
    ErrManifestMissing,
    ErrNoTrustedKeys,
    ErrSigInvalid,
    ErrSigMissing,
)
from .verifier import compute_file_hash, load_config, load_trusted_keys, run_verification, verify_manifest_hashes, verify_skill, verify_skills_dir

__all__ = [
    "ErrConfigMissing",
    "ErrHashMismatch",
    "ErrManifestMissing",
    "ErrNoTrustedKeys",
    "ErrSigInvalid",
    "ErrSigMissing",
    "compute_file_hash",
    "load_config",
    "load_trusted_keys",
    "verify_manifest_hashes",
    "verify_skill",
    "verify_skills_dir",
    "run_verification",
]
