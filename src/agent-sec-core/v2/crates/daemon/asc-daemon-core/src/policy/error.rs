use asc_pap::PapError;
use asc_policy_runtime::RuntimeError;

/// Stable Policy use-case errors projected by daemon adapters.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// PAP authoring failure.
    #[error(transparent)]
    Pap(#[from] PapError),
    /// Binding runtime failure.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
