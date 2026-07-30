//! Deterministic backend for protocol and service tests.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, HealthStatus, ViolationEvent,
};
use uuid::Uuid;

use crate::{BackendError, EnforcementBackend, EventHub};

/// In-memory single-binding backend that performs no kernel operations.
pub struct MockBackend {
    bindings: Mutex<HashMap<Uuid, Binding>>,
    events: EventHub,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    /// Creates an empty backend.
    pub fn new() -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            events: EventHub::default(),
        }
    }

    /// Injects a violation for an active binding.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::MissingBinding`] when the event references an
    /// unknown binding.
    pub fn publish_violation(&self, event: ViolationEvent) -> Result<(), BackendError> {
        if !self.bindings().contains_key(&event.binding_id) {
            return Err(BackendError::MissingBinding(event.binding_id));
        }
        self.events.publish(event);
        Ok(())
    }

    fn bindings(&self) -> MutexGuard<'_, HashMap<Uuid, Binding>> {
        self.bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl EnforcementBackend for MockBackend {
    fn health(&self) -> Result<HealthStatus, BackendError> {
        Ok(HealthStatus {
            ready: true,
            backend: "mock".into(),
            message: Some("mock backend does not enforce kernel operations".into()),
        })
    }

    fn apply(&self, request: ApplyPolicy) -> Result<Binding, BackendError> {
        let mut bindings = self.bindings();
        if let Some(existing) = bindings.get(&request.binding_id) {
            return if existing.request == request {
                Ok(existing.clone())
            } else {
                Err(BackendError::BindingConflict(request.binding_id))
            };
        }
        if !bindings.is_empty() {
            return Err(BackendError::BindingConflict(request.binding_id));
        }

        let binding = Binding {
            request,
            state: BindingState::Enforced,
            message: Some("mock acknowledgement; no kernel policy attached".into()),
            domain_id: Some(1),
        };
        bindings.insert(binding.request.binding_id, binding.clone());
        Ok(binding)
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), BackendError> {
        if self.bindings().remove(&binding_id).is_some() {
            Ok(())
        } else {
            Err(BackendError::MissingBinding(binding_id))
        }
    }

    fn bindings(&self) -> Result<Vec<Binding>, BackendError> {
        let mut bindings: Vec<_> = self.bindings().values().cloned().collect();
        bindings.sort_by_key(|binding| binding.request.binding_id);
        Ok(bindings)
    }

    fn subscribe(&self) -> Receiver<ViolationEvent> {
        self.events.subscribe()
    }
}
