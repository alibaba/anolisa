//! Generation-scoped readiness signaling for background ingestion workers.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Failures while waiting for an ingestion subscription to become usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestionReadinessError {
    /// The active generation did not acknowledge readiness before the deadline.
    Timeout {
        /// Configured wait bound in milliseconds.
        timeout_ms: u64,
    },
    /// No worker remained active for the generation being observed.
    WorkerStopped,
}

impl fmt::Display for IngestionReadinessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { timeout_ms } => {
                write!(formatter, "ingestion was not ready within {timeout_ms}ms")
            }
            Self::WorkerStopped => {
                formatter.write_str("ingestion worker stopped before becoming ready")
            }
        }
    }
}

impl std::error::Error for IngestionReadinessError {}

pub(crate) struct GenerationToken {
    _identity: (),
}

pub(crate) struct GenerationGuard {
    readiness: GenerationReadiness,
    worker: Arc<GenerationToken>,
}

#[derive(Clone)]
pub(crate) struct GenerationReadiness {
    inner: Arc<ReadinessInner>,
}

struct ReadinessInner {
    state: Mutex<ReadinessState>,
    changed: Condvar,
    unavailable_message: &'static str,
}

#[derive(Default)]
struct ReadinessState {
    current: Option<Arc<GenerationToken>>,
    ready: bool,
    message: Option<String>,
}

impl GenerationReadiness {
    pub(crate) fn new(unavailable_message: &'static str) -> Self {
        Self {
            inner: Arc::new(ReadinessInner {
                state: Mutex::new(ReadinessState::default()),
                changed: Condvar::new(),
                unavailable_message,
            }),
        }
    }

    pub(crate) fn candidate(&self) -> Arc<GenerationToken> {
        Arc::new(GenerationToken { _identity: () })
    }

    pub(crate) fn guard(&self, worker: Arc<GenerationToken>) -> GenerationGuard {
        GenerationGuard {
            readiness: self.clone(),
            worker,
        }
    }

    pub(crate) fn install(&self, worker: Arc<GenerationToken>) {
        let mut state = self.state();
        self.set_unready(&mut state, Some(worker));
        self.inner.changed.notify_all();
    }

    pub(crate) fn install_if_idle(&self, worker: Arc<GenerationToken>) -> bool {
        let mut state = self.state();
        if state.current.is_some() {
            return false;
        }
        self.set_unready(&mut state, Some(worker));
        self.inner.changed.notify_all();
        true
    }

    pub(crate) fn stop(&self) {
        let mut state = self.state();
        self.set_unready(&mut state, None);
        self.inner.changed.notify_all();
    }

    pub(crate) fn clear_if_current(&self, worker: &Arc<GenerationToken>) {
        let mut state = self.state();
        if is_worker(&state, worker) {
            self.set_unready(&mut state, None);
            self.inner.changed.notify_all();
        }
    }

    pub(crate) fn mark_ready(&self, worker: &Arc<GenerationToken>) -> bool {
        let mut state = self.state();
        if !is_worker(&state, worker) {
            return false;
        }
        state.ready = true;
        state.message = None;
        self.inner.changed.notify_all();
        true
    }

    pub(crate) fn mark_not_ready(&self, worker: &Arc<GenerationToken>) {
        let mut state = self.state();
        if is_worker(&state, worker) {
            state.ready = false;
            state.message = Some(self.inner.unavailable_message.into());
            self.inner.changed.notify_all();
        }
    }

    pub(crate) fn mark_unavailable(&self, worker: &Arc<GenerationToken>, message: String) {
        let mut state = self.state();
        if is_worker(&state, worker) {
            state.ready = false;
            state.message = Some(message);
            self.inner.changed.notify_all();
        }
    }

    pub(crate) fn wait_ready(&self, timeout: Duration) -> Result<(), IngestionReadinessError> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.state();
        let worker = state
            .current
            .clone()
            .ok_or(IngestionReadinessError::WorkerStopped)?;
        loop {
            if !is_worker(&state, &worker) {
                return Err(IngestionReadinessError::WorkerStopped);
            }
            if state.ready {
                return Ok(());
            }
            let remaining = deadline
                .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                return Err(IngestionReadinessError::Timeout {
                    timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                });
            }
            let (next, timed_out) = self
                .inner
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if timed_out.timed_out() && !state.ready {
                if !is_worker(&state, &worker) {
                    return Err(IngestionReadinessError::WorkerStopped);
                }
                return Err(IngestionReadinessError::Timeout {
                    timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                });
            }
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.state().ready
    }

    pub(crate) fn status(&self) -> (bool, Option<String>) {
        let state = self.state();
        (state.ready, state.message.clone())
    }

    pub(crate) fn is_current(&self, worker: &Arc<GenerationToken>) -> bool {
        is_worker(&self.state(), worker)
    }

    fn set_unready(&self, state: &mut ReadinessState, worker: Option<Arc<GenerationToken>>) {
        state.current = worker;
        state.ready = false;
        state.message = Some(self.inner.unavailable_message.into());
    }

    fn state(&self) -> MutexGuard<'_, ReadinessState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        self.readiness.clear_if_current(&self.worker);
    }
}

fn is_worker(state: &ReadinessState, worker: &Arc<GenerationToken>) -> bool {
    state
        .current
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, worker))
}

#[cfg(test)]
#[path = "ingestion_readiness_tests.rs"]
mod tests;
