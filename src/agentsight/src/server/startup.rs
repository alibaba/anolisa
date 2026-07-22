//! Transactional startup gate for ingestion-dependent background work.

use std::time::Duration;

use crate::IngestionReadinessError;
use crate::ingestion_readiness::GenerationReadiness;

pub(super) fn start_after_ingestion_ready<T, E>(
    enforcement: &GenerationReadiness,
    security: &GenerationReadiness,
    timeout: Duration,
    map_readiness_error: impl FnOnce(IngestionReadinessError) -> E,
    start: impl FnOnce() -> Result<T, E>,
    rollback: impl FnOnce(),
) -> Result<T, E> {
    let result = GenerationReadiness::wait_for_both_ready(enforcement, security, timeout)
        .map_err(map_readiness_error)
        .and_then(|()| start());
    if result.is_err() {
        rollback();
    }
    result
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
