//! Intent Firewall (design doc §6.2): intent classification, TrustScore
//! delta output and declared-vs-observed alignment checks, mounted as a
//! `StatefulAdjudicator` plugin. Depends only on the asc-pdp traits; model
//! inference runs in an isolated process routed through Tier C.
//!
//! Trust deltas are written back by the framework, never by this crate
//! directly, so every trust change passes through the audit trail; within a
//! session the score is monotonically non-increasing.

#![forbid(unsafe_code)]

pub mod adjudicator;
