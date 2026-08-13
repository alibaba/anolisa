//! Typed intermediate representation for compiled policies (design doc §5).
//!
//! Both frontends (P0 structured rules, P1 trace DSL) compile into this IR;
//! the PDP only evaluates IR at runtime, never text. This crate is a pure
//! function library with no IO. The IR carries its own `schema_version`,
//! decoupled from the bundle format: new variants are append-only and unknown
//! variants cause the whole rule to fail loading (activation fails, previous
//! revision stays active) rather than being skipped.

#![forbid(unsafe_code)]

pub mod merge;
pub mod model;
pub mod tier;
