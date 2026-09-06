//! Minimal target Adapter result and dispatchable plan contracts.

use serde::{Deserialize, Serialize};

/// Result of translating one complete immutable Binding snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslationOutcome {
    /// The Adapter produced a target plan that passed its static translation checks.
    Translated(TargetBindingPlan),
    /// The target deterministically cannot express the Binding safely.
    Rejected(TranslationRejection),
}

/// Opaque target-specific Binding payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetBindingPlan {
    /// Versioned target format understood by the matching target Client.
    pub format: String,
    /// Exact bytes that are persisted before Client request preparation.
    pub content: Vec<u8>,
}

/// Deterministic semantic rejection produced by a functioning Adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRejection {
    /// Stable, specific code suitable for status projection and logs.
    pub code: String,
}

/// Internal Adapter failure distinct from a deterministic translation rejection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("target Adapter failed with code {code}")]
pub struct AdapterFault {
    /// Stable internal failure code.
    pub code: String,
}
