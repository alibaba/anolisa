//! `SignedBundle` format and verification chain (§9.1): signature →
//! policyEpoch monotonicity → schema validation → compile → conformance.
//! Any step failing leaves the active revision untouched.

use asc_policy_types::primitives::Digest;
use serde::{Deserialize, Serialize};

/// One content-addressed file inside a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleEntry {
    /// Path relative to the bundle root (e.g. `rules/x.rule.yaml`).
    pub path: String,
    /// blake3 hash of the file content; a merkle leaf.
    pub hash: Digest,
}

/// A policy bundle as delivered: content-addressed entries under an Ed25519
/// detached signature over the merkle root (§9.1). The local trust anchor is
/// the release public key planted at install time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedBundle {
    /// Anti-rollback monotonic epoch from the manifest.
    pub policy_epoch: u64,
    /// Merkle root over all entries.
    pub merkle_root: Digest,
    /// Ed25519 detached signature over the merkle root.
    pub signature: Vec<u8>,
    /// Content-addressed file listing.
    pub entries: Vec<BundleEntry>,
}
