//! Product policy templates, lowering, and backend-independent composition.

#![forbid(unsafe_code)]

mod compose;
mod error;
mod lower;
mod template;

pub use compose::compose_policies;
pub use error::EngineError;
pub use lower::lower_template;
pub use template::{PolicyTemplate, TemplateEnvelope, TrustedDestination};
