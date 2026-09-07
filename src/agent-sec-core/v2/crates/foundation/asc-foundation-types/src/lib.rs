//! Small, stable primitives shared across protocol and application crates.

#![forbid(unsafe_code)]

mod identifier;
mod revision;

pub use identifier::{IdentifierError, ResourceId};
pub use revision::{Revision, RevisionError};
