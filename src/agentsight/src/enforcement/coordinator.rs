//! Desired-state coordinator between AgentSight, SQLite, and the enforcer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState, ViolationEvent};
use thiserror::Error;
use uuid::Uuid;

use super::{EnforcementClient, EnforcementError, EnforcementStore, EnforcementStoreError};

/// Coordination failures across the UDS and persistence boundaries.
#[derive(Debug, Error)]
pub enum EnforcementCoordinatorError {
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
    stop: Arc<AtomicBool>,
}

impl EnforcementCoordinator {
    /// Creates a coordinator without starting background ingestion.
    pub fn new(client: EnforcementClient, store: EnforcementStore) -> Self {
        Self {
            client,
            store,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Persists pending desired state, then applies and persists acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns a persistence error or the enforcer rejection after recording a
    /// sanitized failed state.
    pub fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementCoordinatorError> {
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
    /// Returns an I/O error when the worker thread cannot be spawned.
    pub fn start_ingestion(&self) -> Result<JoinHandle<()>, EnforcementCoordinatorError> {
        self.stop.store(false, Ordering::Release);
        let client = self.client.clone();
        let store = self.store.clone();
        let stop = Arc::clone(&self.stop);
        thread::Builder::new()
            .name("agentsight-enforcement-ingestion".into())
            .spawn(move || ingest_loop(client, store, stop))
            .map_err(Into::into)
    }

    /// Requests the ingestion worker to stop at its next bounded read interval.
    pub fn stop_ingestion(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Queries current enforcer readiness directly.
    ///
    /// # Errors
    ///
    /// Returns a client error when the enforcer cannot be reached.
    pub fn health(
        &self,
    ) -> Result<agentsight_enforcement_protocol::HealthStatus, EnforcementCoordinatorError> {
        Ok(self.client.health()?)
    }
}

fn ingest_loop(client: EnforcementClient, store: EnforcementStore, stop: Arc<AtomicBool>) {
    let mut backoff = Duration::from_millis(100);
    while !stop.load(Ordering::Acquire) {
        match client.subscribe() {
            Ok(mut subscription) => {
                backoff = Duration::from_millis(100);
                while !stop.load(Ordering::Acquire) {
                    match subscription.next_event() {
                        Ok(Some(event)) => {
                            if let Err(error) = store.insert_violation(&event) {
                                eprintln!(
                                    "AgentSight could not persist enforcement event: {error}"
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
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
                let message = format!("enforcement unavailable: {error}");
                if let Err(store_error) = store.mark_active_degraded(&message) {
                    eprintln!(
                        "AgentSight could not persist enforcement unavailability: {store_error}"
                    );
                }
            }
        }
        sleep_until_stop(&stop, backoff);
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
    }
}

fn sleep_until_stop(stop: &AtomicBool, duration: Duration) {
    let step = Duration::from_millis(50);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration && !stop.load(Ordering::Acquire) {
        let remaining = duration.saturating_sub(elapsed);
        let sleep = remaining.min(step);
        thread::sleep(sleep);
        elapsed += sleep;
    }
}
