"""Asset verification module for skill integrity checking."""

from agent_sec_cli.asset_verify.errors import (
    ErrConfigMissing,
    ErrHashMismatch,
    ErrManifestMissing,
    ErrNoTrustedKeys,
    ErrSigInvalid,
    ErrSigMissing,
    ErrUnexpectedFile,
)
from agent_sec_cli.asset_verify.verifier import (
    compute_file_hash,
    load_config,
    load_trusted_keys,
    run_verification,
    verify_manifest_hashes,
    verify_skill,
    verify_skills_dir,
)

__all__ = [
    "ErrConfigMissing",
    "ErrHashMismatch",
    "ErrManifestMissing",
    "ErrNoTrustedKeys",
    "ErrSigInvalid",
    "ErrSigMissing",
    "ErrUnexpectedFile",
    "compute_file_hash",
    "load_config",
    "load_trusted_keys",
    "verify_manifest_hashes",
    "verify_skill",
    "verify_skills_dir",
    "run_verification",
]
