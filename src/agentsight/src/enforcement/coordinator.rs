//! Desired-state coordinator between AgentSight, SQLite, and the enforcer.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState, ViolationEvent};
use thiserror::Error;
use uuid::Uuid;

use super::{EnforcementClient, EnforcementError, EnforcementStore, EnforcementStoreError};

const INGESTION_UNAVAILABLE_MESSAGE: &str = "violation ingestion is not subscribed";

type WorkerTask = Box<dyn FnOnce() + Send + 'static>;

struct WorkerToken {
    _identity: (),
}

#[derive(Default)]
struct IngestionState {
    current: Option<Arc<WorkerToken>>,
    ready: bool,
}

#[derive(Clone)]
struct IngestionReadiness {
    state: Arc<Mutex<IngestionState>>,
}

impl IngestionReadiness {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(IngestionState::default())),
        }
    }

    fn candidate(&self) -> Arc<WorkerToken> {
        Arc::new(WorkerToken { _identity: () })
    }

    fn install(&self, worker: Arc<WorkerToken>) {
        let mut state = self.state();
        state.current = Some(worker);
        state.ready = false;
    }

    fn stop(&self) {
        let mut state = self.state();
        state.current = None;
        state.ready = false;
    }

    fn clear_if_current(&self, worker: &Arc<WorkerToken>) {
        let mut state = self.state();
        if state
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, worker))
        {
            state.current = None;
            state.ready = false;
        }
    }

    fn mark_ready(&self, worker: &Arc<WorkerToken>) -> bool {
        let mut state = self.state();
        if state
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, worker))
        {
            state.ready = true;
            true
        } else {
            false
        }
    }

    fn mark_not_ready(&self, worker: &Arc<WorkerToken>) {
        let mut state = self.state();
        if state
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, worker))
        {
            state.ready = false;
        }
    }

    fn is_ready(&self) -> bool {
        self.state().ready
    }

    fn is_current(&self, worker: &Arc<WorkerToken>) -> bool {
        self.state()
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, worker))
    }

    fn state(&self) -> MutexGuard<'_, IngestionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Coordination failures across the UDS and persistence boundaries.
#[derive(Debug, Error)]
pub enum EnforcementCoordinatorError {
    /// A violation subscriber has not completed its acknowledgement handshake.
    #[error("{INGESTION_UNAVAILABLE_MESSAGE}")]
    IngestionUnavailable,
    /// The privileged service call failed.
    #[error(transparent)]
    Client(#[from] EnforcementError),
    /// Desired state or evidence persistence failed.
    #[error(transparent)]
    Store(#[from] EnforcementStoreError),
    /// The ingestion worker could not be created.
    #[error("start enforcement ingestion: {0}")]
    Thread(#[from] std::io::Error),
}

/// AgentSight owner of desired policy state and violation ingestion.
pub struct EnforcementCoordinator {
    client: EnforcementClient,
    store: EnforcementStore,
    ingestion_readiness: IngestionReadiness,
}

impl EnforcementCoordinator {
    /// Creates a coordinator without starting background ingestion.
    pub fn new(client: EnforcementClient, store: EnforcementStore) -> Self {
        Self {
            client,
            store,
            ingestion_readiness: IngestionReadiness::new(),
        }
    }

    /// Persists pending desired state, then applies and persists acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns a persistence error or the enforcer rejection after recording a
    /// sanitized failed state. Returns [`EnforcementCoordinatorError::IngestionUnavailable`]
    /// before persisting when no violation subscription is acknowledged.
    pub fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementCoordinatorError> {
        // A disconnect can still race this check; durable replay is required to close that gap.
        if !self.ingestion_readiness.is_ready() {
            return Err(EnforcementCoordinatorError::IngestionUnavailable);
        }
        self.store.upsert_binding(&Binding {
            request: request.clone(),
            state: BindingState::Pending,
            message: None,
            domain_id: None,
        })?;
        match self.client.apply(request.clone()) {
            Ok(binding) => {
                self.store.upsert_binding(&binding)?;
                Ok(binding)
            }
            Err(error) => {
                self.store.upsert_binding(&Binding {
                    request,
                    state: BindingState::Failed,
                    message: Some(error.to_string()),
                    domain_id: None,
                })?;
                Err(error.into())
            }
        }
    }

    /// Persists detaching state and waits for acknowledgement before detached.
    ///
    /// # Errors
    ///
    /// Returns a missing-binding, persistence, or enforcer error.
    pub fn detach(&self, binding_id: Uuid) -> Result<(), EnforcementCoordinatorError> {
        let mut binding = self
            .store
            .binding(binding_id)?
            .ok_or(EnforcementStoreError::MissingBinding(binding_id))?;
        binding.state = BindingState::Detaching;
        self.store.upsert_binding(&binding)?;
        match self.client.detach(binding_id) {
            Ok(()) => {
                binding.state = BindingState::Detached;
                binding.message = None;
                self.store.upsert_binding(&binding)?;
                Ok(())
            }
            Err(error) => {
                binding.state = BindingState::Degraded;
                binding.message = Some(error.to_string());
                self.store.upsert_binding(&binding)?;
                Err(error.into())
            }
        }
    }

    /// Lists persisted binding state.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub fn bindings(&self) -> Result<Vec<Binding>, EnforcementCoordinatorError> {
        Ok(self.store.bindings()?)
    }

    /// Lists newest persisted violations.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub fn violations(
        &self,
        limit: usize,
    ) -> Result<Vec<ViolationEvent>, EnforcementCoordinatorError> {
        Ok(self.store.violations(limit)?)
    }

    /// Starts bounded reconnecting violation ingestion.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the worker thread cannot be spawned or activated.
    pub fn start_ingestion(&self) -> Result<JoinHandle<()>, EnforcementCoordinatorError> {
        self.start_ingestion_with(|worker| {
            thread::Builder::new()
                .name("agentsight-enforcement-ingestion".into())
                .spawn(worker)
        })
    }

    fn start_ingestion_with<F>(
        &self,
        spawn: F,
    ) -> Result<JoinHandle<()>, EnforcementCoordinatorError>
    where
        F: FnOnce(WorkerTask) -> Result<JoinHandle<()>, std::io::Error>,
    {
        let worker = self.ingestion_readiness.candidate();
        let client = self.client.clone();
        let store = self.store.clone();
        let ingestion_readiness = self.ingestion_readiness.clone();
        let worker_token = Arc::clone(&worker);
        let (activate, activation) = mpsc::sync_channel(0);
        let task = Box::new(move || {
            if activation.recv().is_ok() {
                ingest_loop(client, store, ingestion_readiness, worker_token);
            }
        });
        let handle = spawn(task)?;
        self.ingestion_readiness.install(Arc::clone(&worker));
        if activate.send(()).is_err() {
            self.ingestion_readiness.clear_if_current(&worker);
            return Err(EnforcementCoordinatorError::Thread(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "activate enforcement ingestion worker",
            )));
        }
        Ok(handle)
    }

    /// Requests the ingestion worker to stop at its next bounded read interval.
    pub fn stop_ingestion(&self) {
        self.ingestion_readiness.stop();
    }

    /// Queries backend readiness and requires an acknowledged violation subscriber.
    ///
    /// # Errors
    ///
    /// Returns a client error when the enforcer cannot be reached.
    pub fn health(
        &self,
    ) -> Result<agentsight_enforcement_protocol::HealthStatus, EnforcementCoordinatorError> {
        let mut health = self.client.health()?;
        let ingestion_ready = self.ingestion_readiness.is_ready();
        if health.ready && !ingestion_ready {
            health.ready = false;
            health.message = Some(INGESTION_UNAVAILABLE_MESSAGE.into());
        } else {
            health.ready &= ingestion_ready;
        }
        Ok(health)
    }
}

fn ingest_loop(
    client: EnforcementClient,
    store: EnforcementStore,
    ingestion_readiness: IngestionReadiness,
    worker: Arc<WorkerToken>,
) {
    let mut backoff = Duration::from_millis(100);
    while ingestion_readiness.is_current(&worker) {
        ingestion_readiness.mark_not_ready(&worker);
        match client.subscribe() {
            Ok(mut subscription) => {
                if !ingestion_readiness.mark_ready(&worker) {
                    break;
                }
                backoff = Duration::from_millis(100);
                while ingestion_readiness.is_current(&worker) {
                    match subscription.next_event() {
                        Ok(Some(event)) => {
                            if !ingestion_readiness.is_current(&worker) {
                                break;
                            }
                            if let Err(error) = store.insert_violation(&event) {
                                eprintln!(
                                    "AgentSight could not persist enforcement event: {error}"
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            ingestion_readiness.mark_not_ready(&worker);
                            if !ingestion_readiness.is_current(&worker) {
                                break;
                            }
                            let message = format!("enforcement subscription lost: {error}");
                            if let Err(store_error) = store.mark_active_degraded(&message) {
                                eprintln!(
                                    "AgentSight could not persist enforcement degradation: {store_error}"
                                );
                            }
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                ingestion_readiness.mark_not_ready(&worker);
                if ingestion_readiness.is_current(&worker) {
                    let message = format!("enforcement unavailable: {error}");
                    if let Err(store_error) = store.mark_active_degraded(&message) {
                        eprintln!(
                            "AgentSight could not persist enforcement unavailability: {store_error}"
                        );
                    }
                }
            }
        }
        ingestion_readiness.mark_not_ready(&worker);
        sleep_until_superseded(&ingestion_readiness, &worker, backoff);
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
    }
    ingestion_readiness.mark_not_ready(&worker);
}

fn sleep_until_superseded(
    ingestion_readiness: &IngestionReadiness,
    worker: &Arc<WorkerToken>,
    duration: Duration,
) {
    let step = Duration::from_millis(50);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration && ingestion_readiness.is_current(worker) {
        let remaining = duration.saturating_sub(elapsed);
        let sleep = remaining.min(step);
        thread::sleep(sleep);
        elapsed += sleep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_start_does_not_supersede_ready_worker() {
        let coordinator = EnforcementCoordinator::new(
            EnforcementClient::new("/tmp/unused-enforcement.sock"),
            EnforcementStore::open(":memory:").expect("test store should open"),
        );
        let active = coordinator.ingestion_readiness.candidate();
        coordinator.ingestion_readiness.install(Arc::clone(&active));
        assert!(coordinator.ingestion_readiness.mark_ready(&active));

        let result = coordinator
            .start_ingestion_with(|_| Err(std::io::Error::other("fixture thread spawn failure")));

        assert!(matches!(
            result,
            Err(EnforcementCoordinatorError::Thread(_))
        ));
        assert!(coordinator.ingestion_readiness.is_current(&active));
        assert!(coordinator.ingestion_readiness.is_ready());
    }

    #[test]
    fn ancient_worker_never_becomes_current_after_replacements() {
        let readiness = IngestionReadiness::new();
        let ancient = readiness.candidate();
        readiness.install(Arc::clone(&ancient));
        assert!(readiness.mark_ready(&ancient));

        for _ in 0..10_000 {
            let current = readiness.candidate();
            readiness.install(Arc::clone(&current));
            assert!(!readiness.is_current(&ancient));
            assert!(!readiness.mark_ready(&ancient));
            assert!(readiness.mark_ready(&current));
        }
    }

    #[test]
    fn superseded_worker_cannot_publish_or_revoke_current_readiness() {
        let readiness = IngestionReadiness::new();
        let first = readiness.candidate();
        readiness.install(Arc::clone(&first));
        assert!(readiness.mark_ready(&first));

        let second = readiness.candidate();
        readiness.install(Arc::clone(&second));
        assert!(!readiness.is_ready());
        assert!(!readiness.mark_ready(&first));
        assert!(readiness.mark_ready(&second));
        readiness.mark_not_ready(&first);
        assert!(readiness.is_ready());

        readiness.stop();
        assert!(!readiness.mark_ready(&second));
        assert!(!readiness.is_ready());
    }
}
