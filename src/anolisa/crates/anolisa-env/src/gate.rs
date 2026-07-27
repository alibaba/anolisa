//! Environment requirement gating — evaluates component requirements against detected facts.

use crate::EnvFacts;

/// Result of checking a component's environment requirements.
#[derive(Debug)]
pub enum GateResult {
    /// Fully compatible.
    Compatible,
    /// Partially compatible with degraded functionality.
    Partial { reason: String, advice: String },
    /// Not compatible.
    Incompatible { reason: String, advice: String },
}

/// Evaluate a set of environment requirements against detected facts.
/// Requirements are expressed as key-value pairs from the component manifest.
pub fn evaluate(
    _requirements: &std::collections::HashMap<String, String>,
    _facts: &EnvFacts,
) -> GateResult {
    // TODO(owner: env-detection, when: manifest requirement syntax stabilizes):
    // evaluate requirement expressions instead of accepting every manifest.
    GateResult::Compatible
}
