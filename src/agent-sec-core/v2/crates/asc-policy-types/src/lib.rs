//! Domain model and stable schema contracts for the AgentSecCore V2 Policy
//! Engine Framework (design doc ASC-DD-2026-08-01 §3).
//!
//! This crate defines the cross-module wire contracts: subject/event/request
//! (`SubjectRef`, `EventEnvelope`, `DecisionRequest`), the verdict model
//! (`Verdict`, `Obligation`, five-value projection) and the `TokenAuthority`
//! trait for `ResumeToken` issuance/verification. Rust types are the canonical
//! form; serde JSON is the wire format.
//!
//! Schema evolution is append-only: breaking changes must bump the
//! `schema_version` carried by each envelope type.

#![forbid(unsafe_code)]

pub mod attribute;
pub mod event;
pub mod primitives;
pub mod request;
pub mod subject;
pub mod token;
pub mod verdict;
