//! Environment probes for detecting platform, kernel, distro, etc.

pub mod distro;
pub mod frameworks;
pub mod kernel;
pub mod platform;

use crate::EnvFacts;

/// Trait for pluggable environment detectors.
pub trait EnvDetector {
    /// Human-readable name of this detector.
    fn name(&self) -> &str;

    /// Priority for execution ordering (lower = earlier).
    fn priority(&self) -> u8;

    /// Run detection and mutate the facts in place.
    fn detect(&self, facts: &mut EnvFacts) -> Result<(), DetectError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("probe '{probe}' failed: {reason}")]
    ProbeFailed { probe: String, reason: String },

    #[error("I/O error during detection: {0}")]
    Io(#[from] std::io::Error),
}
