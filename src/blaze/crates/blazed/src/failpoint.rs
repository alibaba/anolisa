// SPDX-License-Identifier: Apache-2.0
//! Feature-gated fault hooks for daemon-level integration verification.

#![allow(dead_code)] // Call sites land with their owning lifecycle commits.

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use tokio::sync::Notify;

const FAILPOINTS_ENV: &str = "BLAZE_TEST_FAILPOINTS";
const FAILPOINT_FILE_ENV: &str = "BLAZE_TEST_FAILPOINT_FILE";

#[cfg(test)]
thread_local! {
    static TEST_FAILPOINTS: RefCell<Option<Arc<TestFailpointState>>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
struct TestFailpointState {
    names: Vec<&'static str>,
    released: AtomicBool,
    paused: AtomicBool,
    paused_notify: Notify,
    release_notify: Notify,
}

/// Task-scoped failpoint driver used by feature-enabled unit tests.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestFailpoint {
    state: Arc<TestFailpointState>,
}

#[cfg(test)]
struct TestFailpointScope {
    previous: Option<Arc<TestFailpointState>>,
}

#[cfg(test)]
impl Drop for TestFailpointScope {
    fn drop(&mut self) {
        TEST_FAILPOINTS.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
impl TestFailpoint {
    /// Build a driver for one or more failpoints.
    pub(crate) fn new(names: &[&'static str]) -> Self {
        Self {
            state: Arc::new(TestFailpointState {
                names: names.to_vec(),
                released: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                paused_notify: Notify::new(),
                release_notify: Notify::new(),
            }),
        }
    }

    /// Run one future with this failpoint set in its test-thread context.
    pub(crate) async fn run<F: Future>(&self, future: F) -> F::Output {
        let previous = TEST_FAILPOINTS.with(|current| current.replace(Some(self.state.clone())));
        let _scope = TestFailpointScope { previous };
        future.await
    }

    /// Wait until the scoped future reaches a pause failpoint.
    pub(crate) async fn wait_until_paused(&self) {
        loop {
            let notified = self.state.paused_notify.notified();
            if self.state.paused.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Release a scoped pause failpoint.
    pub(crate) fn release(&self) {
        self.state.released.store(true, Ordering::Release);
        self.state.release_notify.notify_waiters();
    }
}

/// Log that a test-only binary is accepting failpoint configuration.
pub(crate) fn announce() {
    tracing::warn!(
        failpoints = %std::env::var(FAILPOINTS_ENV).unwrap_or_default(),
        failpoint_file = %std::env::var(FAILPOINT_FILE_ENV).unwrap_or_default(),
        "test-only failpoint feature enabled"
    );
}

/// Return a backend-domain error when `name` is currently armed.
pub(crate) fn backend(name: &str) -> blaze_core::Result<()> {
    if hit(name) {
        return Err(blaze_core::BlazeError::BackendError {
            msg: format!("test failpoint '{name}' triggered"),
        });
    }
    Ok(())
}

/// Return a storage-domain error when `name` is currently armed.
pub(crate) fn storage(name: &str) -> blaze_core::Result<()> {
    if hit(name) {
        return Err(blaze_core::BlazeError::StorageError {
            msg: format!("test failpoint '{name}' triggered"),
        });
    }
    Ok(())
}

/// Return a daemon state-commit error when `name` is currently armed.
pub(crate) fn state(name: &str) -> crate::error::Result<()> {
    if hit(name) {
        return Err(crate::error::BlazeDaemonError::Internal(format!(
            "test failpoint '{name}' triggered"
        )));
    }
    Ok(())
}

/// Hold an in-flight request at a durable transaction boundary.
///
/// This is compiled into test-only binaries so a verifier can terminate the
/// daemon after observing the persisted operation marker. Removing the name
/// from the failpoint file releases the request when the daemon is not killed.
pub(crate) async fn pause(name: &str) {
    #[cfg(test)]
    if let Some(state) = test_state(name) {
        state.paused.store(true, Ordering::Release);
        state.paused_notify.notify_waiters();
        tracing::warn!(failpoint = name, "test failpoint paused");
        while !state.released.load(Ordering::Acquire) {
            state.release_notify.notified().await;
        }
        tracing::warn!(failpoint = name, "test failpoint released");
        return;
    }

    if armed(name) {
        tracing::warn!(failpoint = name, "test failpoint paused");
        while armed(name) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tracing::warn!(failpoint = name, "test failpoint released");
    }
}

fn hit(name: &str) -> bool {
    if armed(name) {
        tracing::warn!(failpoint = name, "test failpoint triggered");
        return true;
    }
    false
}

fn armed(name: &str) -> bool {
    #[cfg(test)]
    if test_state(name).is_some() {
        return true;
    }
    let inline = std::env::var(FAILPOINTS_ENV).unwrap_or_default();
    let file = std::env::var(FAILPOINT_FILE_ENV)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    configured(name, &inline, &file)
}

#[cfg(test)]
fn test_state(name: &str) -> Option<Arc<TestFailpointState>> {
    TEST_FAILPOINTS
        .with(|current| current.borrow().clone())
        .filter(|state| !state.released.load(Ordering::Acquire) && state.names.contains(&name))
}

fn configured(name: &str, inline: &str, file: &str) -> bool {
    inline
        .split(|character: char| character == ',' || character.is_whitespace())
        .chain(file.split(|character: char| character == ',' || character.is_whitespace()))
        .filter(|token| !token.is_empty())
        .any(|token| token == name)
}

#[cfg(test)]
mod tests {
    use super::configured;

    #[test]
    fn configuration_matches_complete_tokens_from_both_sources() {
        assert!(configured("before-publish", "start, before-publish", ""));
        assert!(configured("after-publish", "", "start\nafter-publish"));
        assert!(!configured("publish", "before-publish", "after-publish"));
    }
}
