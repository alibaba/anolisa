//! Content routing for the shared compression pipeline.
//!
//! This crate carries the pieces of roadmap §4.2 / §5.2 that sit between the
//! protocol boundary and the compressors themselves:
//!
//! - [`ContentType`]: the first content taxonomy;
//! - [`detect`]: deterministic, bounded-cost content detection;
//! - [`CompressorSpec`] and [`candidates`]: the compile-time registry and
//!   the seam/capability filter (roadmap principle 2: route by content,
//!   constrain by seam).
//!
//! The staged pipeline and end-to-end arbitration that consume these types
//! arrive separately; until existing compressors move behind the registry,
//! [`REGISTRY`] is empty and nothing routes through this crate. The roadmap
//! section numbers refer to the tokenless evolution roadmap, which has not
//! landed in this repository yet.

mod content;
mod registry;

pub use content::{ContentType, detect};
pub use registry::{CompressorSpec, CostClass, REGISTRY, Stage, candidates};
