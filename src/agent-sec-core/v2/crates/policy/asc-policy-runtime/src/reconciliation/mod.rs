//! Dedicated Binding queue, bounded workers and compensation scanning.
mod queue;
pub use queue::WorkQueue;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use asc_foundation_types::ResourceId;
use asc_pcp::{BindingReconciler, Clock, Disposition};
use asc_policy_repository::{BindingReconcileCatalog, BindingStateRepository, StoreError};
use asc_policy_types::binding::BindingStatus;

/// One shared clock domain for core deadlines and queue timers.
pub struct MonotonicClock(Instant);
impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Attempts are serialized per Binding by the owning Runtime `WorkQueue`.
/// Callers must not concurrently invoke the same Binding through another queue
/// or directly; one Runtime owns scheduling for each Repository.
pub trait ReconcileAttempt: Send + Sync {
    /// Returns the shared clock used by this attempt's deadlines. Wrappers must
    /// delegate to the wrapped core, never construct an independent clock.
    fn clock(&self) -> Arc<dyn Clock>;

    /// # Errors
    /// Returns storage failure or exhausted CAS contention without claiming the
    /// remote operation failed.
    fn reconcile(&self, id: &ResourceId) -> Result<Disposition, StoreError>;
}
impl ReconcileAttempt for BindingReconciler {
    fn clock(&self) -> Arc<dyn Clock> {
        self.clock()
    }

    fn reconcile(&self, id: &ResourceId) -> Result<Disposition, StoreError> {
        self.reconcile(id)
    }
}

#[derive(Clone, Copy)]
pub struct RuntimeConfig {
    /// Concurrent synchronous calls across distinct Bindings; default 4, valid 1..=256.
    pub workers: usize,
    /// Maximum distinct queue entries in all states, including Exhausted; default 65,536.
    /// Must be nonzero; no additional application-level upper bound is enforced.
    pub capacity: usize,
    /// Maximum repository candidates per scan page; default 128, valid 1..=1000.
    pub scan_batch: usize,
    /// Interval for retry timer ticks and compensation scans; default 100 ms.
    pub tick_interval: Duration,
    /// Delay for Superseded and repository errors, in whole milliseconds.
    pub storage_retry: Duration,
    /// Automatic reschedules per notification/discovery, excluding the first call.
    /// Exhaustion remains in memory until a new notification or process restart.
    pub max_auto_retries: u32,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            capacity: 65_536,
            scan_batch: 128,
            tick_interval: Duration::from_millis(100),
            storage_retry: Duration::from_secs(1),
            max_auto_retries: 4,
        }
    }
}

/// Owns every worker and scanner handle. Stop admission/requests before shutdown.
/// Shutdown joins real synchronous calls; dropping an async waiter cannot release
/// their execution ownership. The process owner supplies an outer drain deadline.
pub struct ReconciliationRuntime {
    queue: Arc<WorkQueue>,
    handles: Vec<JoinHandle<()>>,
}
impl ReconciliationRuntime {
    /// # Errors
    /// Returns invalid configuration, storage readiness failure or thread creation failure.
    /// # Panics
    /// Panics if the Repository readiness probe panics or internal queue state is poisoned.
    pub fn start<R>(
        repository: Arc<R>,
        reconciler: Arc<dyn ReconcileAttempt>,
        config: RuntimeConfig,
    ) -> Result<Self, StoreError>
    where
        R: BindingStateRepository + BindingReconcileCatalog + 'static,
    {
        if config.workers == 0
            || config.workers > 256
            || config.capacity == 0
            || config.scan_batch == 0
            || config.scan_batch > 1000
            || config.tick_interval.is_zero()
            || config.storage_retry.as_millis() == 0
        {
            return Err(StoreError::Invalid);
        }
        // Establish repository readiness before exposing the notification port.
        repository.scan_reconciliation(None, 1)?;
        let clock = reconciler.clock();
        let queue = Arc::new(WorkQueue::new(config.capacity, config.max_auto_retries));
        let mut service = Self {
            queue: queue.clone(),
            handles: vec![],
        };
        for index in 0..config.workers {
            let repository = repository.clone();
            let reconciler = reconciler.clone();
            let clock = clock.clone();
            let queue = queue.clone();
            service.spawn(format!("binding-worker-{index}"), move || {
                while let Some(id) = queue.take() {
                    // The core finishes panic bookkeeping before unwinding here.
                    // Keep Running until that unwind and the state check finish.
                    let Ok(result) = catch_unwind(AssertUnwindSafe(|| reconciler.reconcile(&id)))
                    else {
                        finish_panicked(repository.as_ref(), &queue, id);
                        continue;
                    };
                    let (deadline, auto_retry) = scheduling_decision(
                        repository.as_ref(),
                        &id,
                        &result,
                        clock.now_ms(),
                        config.storage_retry,
                    );
                    if queue.finish(id.clone(), deadline, auto_retry) {
                        eprintln!("binding reconciliation {id}: automatic retry budget exhausted");
                    }
                }
            })?;
        }
        drop(reconciler);
        service.spawn("binding-timer".into(), move || {
            let mut cursor = None;
            loop {
                queue.tick(clock.now_ms());
                match repository.scan_reconciliation(cursor.as_ref(), config.scan_batch) {
                    Ok(batch) => {
                        queue.state.lock().unwrap().scan_failed = false;
                        let end = batch.len() < config.scan_batch;
                        cursor = if end {
                            None
                        } else {
                            batch.last().map(|c| c.id.clone())
                        };
                        let candidates = batch
                            .into_iter()
                            .filter(|candidate| {
                                candidate.status.is_reconciling()
                                    || matches!(
                                        candidate.status,
                                        BindingStatus::PendingApply | BindingStatus::PendingDelete
                                    )
                            })
                            .map(|candidate| (candidate.id, candidate.next_attempt_at));
                        queue.discover_many(candidates, clock.now_ms());
                    }
                    Err(_) => queue.state.lock().unwrap().scan_failed = true,
                }
                if !queue.wait_tick(config.tick_interval) {
                    break;
                }
            }
        })?;
        Ok(service)
    }

    fn spawn(
        &mut self,
        name: String,
        task: impl FnOnce() + Send + 'static,
    ) -> Result<(), StoreError> {
        let queue = self.queue.clone();
        if let Ok(handle) = thread::Builder::new().name(name).spawn(move || {
            if catch_unwind(AssertUnwindSafe(task)).is_err() {
                queue.fail();
            }
        }) {
            self.handles.push(handle);
            Ok(())
        } else {
            self.queue.fail();
            Err(StoreError::Unavailable)
        }
    }

    pub fn enqueuer(&self) -> Arc<WorkQueue> {
        self.queue.clone()
    }
    /// # Errors
    /// Reports a worker/scanner failure after joining all actual calls.
    /// # Panics
    /// Panics if internal queue state is poisoned.
    pub fn shutdown(mut self) -> Result<(), StoreError> {
        self.queue.stop();
        let mut failed = false;
        for handle in self.handles.drain(..) {
            failed |= handle.join().is_err();
        }
        if failed || self.queue.state.lock().unwrap().fatal {
            Err(StoreError::Unavailable)
        } else {
            Ok(())
        }
    }
}
impl Drop for ReconciliationRuntime {
    fn drop(&mut self) {
        self.queue.stop();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn finish_panicked(repository: &dyn BindingStateRepository, queue: &WorkQueue, id: ResourceId) {
    eprintln!("binding reconciliation {id}: attempt panicked");
    // A broken port can also panic during this read. Never turn an unconfirmed
    // outcome into completion or automatically replay the panicking attempt.
    let terminal = match catch_unwind(AssertUnwindSafe(|| repository.get_binding_state(&id))) {
        Ok(Ok(None)) => true,
        Ok(Ok(Some(record))) => matches!(
            record.binding.status,
            BindingStatus::Ready
                | BindingStatus::ApplyFailed
                | BindingStatus::DeleteFailed
                | BindingStatus::Deleted
        ),
        _ => false,
    };
    if !terminal {
        eprintln!("binding reconciliation {id}: panic outcome unconfirmed");
    }
    queue.finish_panicked(id, terminal);
}

fn scheduling_decision(
    repository: &dyn BindingStateRepository,
    id: &ResourceId,
    result: &Result<Disposition, StoreError>,
    now: u64,
    delay: Duration,
) -> (Option<u64>, bool) {
    let decision = match result {
        Ok(Disposition::RetryAt { at }) => {
            // A deadline can pass during bookkeeping. Still yield
            // to the timer rather than immediately retrying.
            Ok((Some((*at).max(now.saturating_add(1))), true))
        }
        Ok(Disposition::Superseded) => Ok((Some(retry_at(now, delay)), true)),
        Ok(Disposition::Skipped) => repository.get_binding_state(id).map(|record| {
            let deadline = record
                .filter(|r| {
                    matches!(
                        r.binding.status,
                        BindingStatus::PendingApply | BindingStatus::PendingDelete
                    )
                })
                .and_then(|r| r.runtime.next_attempt_at);
            (deadline, false)
        }),
        Ok(Disposition::Completed | Disposition::Failed { .. }) => Ok((None, false)),
        Err(error) => Err(*error),
    };
    match decision {
        Ok(decision) => decision,
        Err(error) => (Some(error_retry_at(id, error, now, delay)), true),
    }
}

fn error_retry_at(id: &ResourceId, error: StoreError, now: u64, delay: Duration) -> u64 {
    // Errors may prevent recording diagnostics in the Binding. Only emit the ID
    // and the closed, payload-free error enum; contention is routine scheduling.
    if error != StoreError::Contended {
        eprintln!("binding reconciliation {id}: {error}");
    }
    retry_at(now, delay)
}

fn retry_at(now: u64, delay: Duration) -> u64 {
    now.saturating_add(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
