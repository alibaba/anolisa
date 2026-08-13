//! Policy information point (design doc §7): `ContextProvider` abstraction,
//! the ContextBroker for batched concurrent attribute fetch, and the failure
//! semantics matrix (Table 8).
//!
//! Missing context is not an error but part of the decision: it flows into
//! `Verdict.missing_context` and triggers STEP_UP/DEFER. Every attribute
//! carries a provenance assurance level (P0-P3) for `minAssurance` gating.
//! Provider timeouts run on the runtime injected by asc-policyd; the PDP
//! sees a synchronous snapshot.

#![forbid(unsafe_code)]

pub mod broker;
pub mod provider;
