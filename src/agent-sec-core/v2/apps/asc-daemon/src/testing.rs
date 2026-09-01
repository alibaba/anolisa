//! Test-only transport entry points kept out of the production facade.

use asc_policy_runtime::PolicyAdapter;

use crate::{AppState, BoundSocket};

/// Serves a bounded number of connections for deterministic integration tests.
///
/// # Errors
/// Returns accept or response I/O errors.
pub fn serve_n<A>(
    listener: &BoundSocket,
    state: &AppState<A>,
    maximum: usize,
) -> Result<(), std::io::Error>
where
    A: PolicyAdapter + 'static,
{
    crate::transport::serve_n(listener, state, maximum)
}
