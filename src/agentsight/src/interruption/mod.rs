//! Interruption module — public API.

pub mod types;
pub mod detector;
pub mod loop_detector;
pub mod oom_recovery;
pub mod security_scanner;

pub use types::{InterruptionEvent, InterruptionType, Severity};
pub use detector::{InterruptionDetector, DetectorConfig};
pub use loop_detector::{LoopDetector, LoopDetectorConfig, RecentCallSummary};
pub use security_scanner::SecurityScanner;
pub use oom_recovery::{recover_oom_events, was_pid_oom_killed};
