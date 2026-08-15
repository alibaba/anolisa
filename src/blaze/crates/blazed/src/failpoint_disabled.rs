// SPDX-License-Identifier: Apache-2.0
//! No-op hooks used when daemon verification support is disabled.

/// Keep daemon startup independent from verification-only configuration.
pub(crate) fn announce() {}

/// Leave backend operations unchanged in production builds.
pub(crate) fn backend(_name: &str) -> blaze_core::Result<()> {
    Ok(())
}

/// Leave storage operations unchanged in production builds.
pub(crate) fn storage(_name: &str) -> blaze_core::Result<()> {
    Ok(())
}

/// Empty test context used by the production no-op implementation.
#[cfg(test)]
pub(crate) struct TestFailpointContext;

/// Capture an empty failpoint context in default-feature tests.
#[cfg(test)]
pub(crate) fn capture_test_context() -> TestFailpointContext {
    TestFailpointContext
}

/// Run a blocking operation unchanged in default-feature tests.
#[cfg(test)]
pub(crate) fn with_test_context<T>(
    _context: TestFailpointContext,
    operation: impl FnOnce() -> T,
) -> T {
    operation()
}

/// Leave guest operations unchanged in production builds.
pub(crate) fn guest(_name: &str) -> crate::guest::Result<()> {
    Ok(())
}

/// Leave state commits unchanged in production builds.
pub(crate) fn state(_name: &str) -> crate::error::Result<()> {
    Ok(())
}

/// Never pause production requests.
pub(crate) async fn pause(_name: &str) {}

// Spawn detached supervision in production builds.
pub(crate) fn spawn<F, R>(future: F) -> tokio::task::JoinHandle<R>
where
    F: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    tokio::spawn(future)
}

/// Never pause production blocking operations.
pub(crate) fn pause_blocking(_name: &str) {}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn production_hooks_are_inert() {
        super::announce();
        super::backend("any").expect("backend hook");
        super::storage("any").expect("storage hook");
        super::guest("any").expect("guest hook");
        super::state("any").expect("state hook");
        super::pause("any").await;
    }
}
