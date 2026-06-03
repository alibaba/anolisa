//! Interruption module — public API.

pub mod detector;
pub mod oom_recovery;
pub mod types;

pub use detector::{DetectorConfig, InterruptionDetector};
pub use oom_recovery::recover_oom_events;
pub use types::{InterruptionEvent, InterruptionType, Severity};
