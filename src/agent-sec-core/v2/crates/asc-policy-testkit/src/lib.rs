//! Policy test framework (design doc §10.2): conformance corpus runner
//! (NDJSON event sequences + expected verdicts), shadow replay against
//! staged revisions with decision diffing, and the fail-closed fault
//! injection suite used as a release gate (§10.3).
//!
//! Consumed as a dev-dependency by other crates and by the PAP activation
//! conformance stage; never part of the shipped daemon.

#![forbid(unsafe_code)]

pub mod conformance;
pub mod fault;
pub mod shadow;
