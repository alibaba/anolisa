//! Interruption module — public API.

pub mod types;
pub mod detector;
pub mod oom_recovery;

pub use types::{InterruptionEvent, InterruptionType, Severity};
pub use detector::{InterruptionDetector, DetectorConfig};
pub use oom_recovery::recover_oom_events;
