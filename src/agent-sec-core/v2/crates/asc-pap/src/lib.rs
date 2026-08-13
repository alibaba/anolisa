//! Policy administration point (design doc §9): AgentPolicy bundle signature
//! verification (Ed25519 over a blake3 merkle root, policyEpoch
//! anti-rollback), the prepare/commit/cancel activation transaction with
//! last-known-good rollback, and monitor → canary → enforce rollout control.
//!
//! Staging and active revisions live in separate directories; leftover
//! staging state is discarded on crash recovery. A failed verification or
//! compilation never affects the active revision.

#![forbid(unsafe_code)]

pub mod bundle;
pub mod rollout;
pub mod store;
