//! Transactional startup gate for ingestion-dependent background work.

pub(super) fn start_after_ingestion_ready<T, E>(
    wait_enforcement: impl FnOnce() -> Result<(), E>,
    wait_security: impl FnOnce() -> Result<(), E>,
    start: impl FnOnce() -> Result<T, E>,
    rollback: impl FnOnce(),
) -> Result<T, E> {
    let result = wait_enforcement()
        .and_then(|()| wait_security())
        .and_then(|()| start());
    if result.is_err() {
        rollback();
    }
    result
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
