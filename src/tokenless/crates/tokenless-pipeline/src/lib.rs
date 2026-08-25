//! The shared compression pipeline: content routing, staged execution, and
//! end-to-end arbitration.
//!
//! This crate carries the pieces of roadmap §4.2–§4.3 / §5.2–§5.3 that sit
//! between the protocol boundary and the compressors themselves:
//!
//! - [`ContentType`]: the first content taxonomy;
//! - [`detect`]: deterministic, bounded-cost content detection;
//! - [`CompressorSpec`] and [`candidates`]: the compile-time registry and
//!   the seam/capability filter (roadmap principle 2: route by content,
//!   constrain by seam);
//! - [`Compressor`] and [`run`]: the executable compressor interface and
//!   the staged escalation engine with end-to-end arbitration.
//!
//! [`REGISTRY`] lists the production compressors; the first entry is the
//! existing JSON response cleanup ([`RESPONSE_CLEANUP`]), whose executable
//! implementation lives with the Runtime that owns its stash and per-call
//! configuration. The roadmap section numbers refer to the tokenless
//! evolution roadmap, which has not landed in this repository yet.

mod content;
mod pipeline;
mod registry;

pub use content::{ContentType, detect};
pub use pipeline::{CompressError, CompressOutcome, Compressor, PipelineConfig, run};
pub use registry::{CompressorSpec, CostClass, REGISTRY, RESPONSE_CLEANUP, Stage, candidates};
