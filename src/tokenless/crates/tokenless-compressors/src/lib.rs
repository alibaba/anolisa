//! Compressor engines for the tokenless pipeline.
//!
//! This crate hosts executable compression engines. Routing metadata
//! (`CompressorSpec`) lives in `tokenless-pipeline`, and the adapters that
//! implement the pipeline's `Compressor` trait live in `tokenless-runtime`.
//! Engines here are trait-agnostic (`&str` in, outcome out) so they stay
//! independently testable and free of pipeline types.

mod build_log;
mod terminal_cleanup;

pub use build_log::{BuildLogMode, BuildLogOutcome, compress_log};
pub use terminal_cleanup::clean_terminal;
