//! Interruption module — public API.

pub mod detector;
pub mod loop_detector;
pub mod oom_recovery;
pub mod types;

pub use detector::{DetectorConfig, InterruptionDetector};
pub use loop_detector::{LoopDetector, LoopDetectorConfig, RecentCallSummary};
pub use oom_recovery::{recover_oom_events, was_pid_oom_killed};
pub use types::{InterruptionEvent, InterruptionType, Severity};
