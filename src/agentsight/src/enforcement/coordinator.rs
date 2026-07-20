//! Desired-state coordinator between AgentSight, SQLite, and the enforcer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState, ViolationEvent};
use thiserror::Error;
use uuid::Uuid;

use super::{EnforcementClient, EnforcementError, EnforcementStore, EnforcementStoreError};

const INGESTION_UNAVAILABLE_MESSAGE: &str = "violation ingestion is not subscribed";
const READY_BIT: u64 = 1;

#[derive(Clone)]
struct IngestionReadiness {
    state: Arc<AtomicU64>,
}

impl IngestionReadiness {
    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU64::new(0)),
        }
    }

    fn begin_worker(&self) -> u64 {
        self.advance_generation()
    }

    fn stop(&self) {
        self.advance_generation();
    }

    fn mark_ready(&self, generation: u64) -> bool {
        self.state
            .compare_exchange(
                generation,
                generation | READY_BIT,
                Ordering::Release,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn mark_not_ready(&self, generation: u64) {
        let _ = self.state.compare_exchange(
            generation | READY_BIT,
            generation,
            Ordering::Release,
            Ordering::Acquire,
        );
    }

    fn is_ready(&self) -> bool {
        self.state.load(Ordering::Acquire) & READY_BIT != 0
    }

    fn is_current(&self, generation: u64) -> bool {
        self.state.load(Ordering::Acquire) & !READY_BIT == generation
    }

    fn advance_generation(&self) -> u64 {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            let next = (current & !READY_BIT).wrapping_add(2);
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
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
    /// Returns an I/O error when the worker thread cannot be spawned.
    pub fn start_ingestion(&self) -> Result<JoinHandle<()>, EnforcementCoordinatorError> {
        let generation = self.ingestion_readiness.begin_worker();
        let client = self.client.clone();
        let store = self.store.clone();
        let ingestion_readiness = self.ingestion_readiness.clone();
        thread::Builder::new()
            .name("agentsight-enforcement-ingestion".into())
            .spawn(move || ingest_loop(client, store, ingestion_readiness, generation))
            .map_err(Into::into)
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
    generation: u64,
) {
    let mut backoff = Duration::from_millis(100);
    while ingestion_readiness.is_current(generation) {
        ingestion_readiness.mark_not_ready(generation);
        match client.subscribe() {
            Ok(mut subscription) => {
                if !ingestion_readiness.mark_ready(generation) {
                    break;
                }
                backoff = Duration::from_millis(100);
                while ingestion_readiness.is_current(generation) {
                    match subscription.next_event() {
                        Ok(Some(event)) => {
                            if !ingestion_readiness.is_current(generation) {
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
                            ingestion_readiness.mark_not_ready(generation);
                            if !ingestion_readiness.is_current(generation) {
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
                ingestion_readiness.mark_not_ready(generation);
                if ingestion_readiness.is_current(generation) {
                    let message = format!("enforcement unavailable: {error}");
                    if let Err(store_error) = store.mark_active_degraded(&message) {
                        eprintln!(
                            "AgentSight could not persist enforcement unavailability: {store_error}"
                        );
                    }
                }
            }
        }
        ingestion_readiness.mark_not_ready(generation);
        sleep_until_superseded(&ingestion_readiness, generation, backoff);
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
    }
    ingestion_readiness.mark_not_ready(generation);
}

fn sleep_until_superseded(
    ingestion_readiness: &IngestionReadiness,
    generation: u64,
    duration: Duration,
) {
    let step = Duration::from_millis(50);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration && ingestion_readiness.is_current(generation) {
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
    fn superseded_worker_cannot_publish_or_revoke_current_readiness() {
        let readiness = IngestionReadiness::new();
        let first = readiness.begin_worker();
        assert!(readiness.mark_ready(first));

        let second = readiness.begin_worker();
        assert!(!readiness.is_ready());
        assert!(!readiness.mark_ready(first));
        assert!(readiness.mark_ready(second));
        readiness.mark_not_ready(first);
        assert!(readiness.is_ready());

        readiness.stop();
        assert!(!readiness.mark_ready(second));
        assert!(!readiness.is_ready());
    }
}
