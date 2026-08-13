//! Audit and decision logging (design doc §9.5): `DecisionRecord` and
//! `EnforcementReceipt` written to separate append-only NDJSON files linked
//! by decision_id, with a per-record `prev_record_hash` chain (P0) and
//! periodic Ed25519 checkpoints (P1).
//!
//! The append-only writer is single-threaded; under backpressure advisory
//! records may be dropped but hard-path records never are. High-frequency
//! Tier A kernel hits are aggregated with sampled details.

#![forbid(unsafe_code)]

pub mod hash_chain;
pub mod record;
