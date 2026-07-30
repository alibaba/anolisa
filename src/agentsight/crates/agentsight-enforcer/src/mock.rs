//! Deterministic backend for protocol and service tests.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, HealthStatus, ViolationEvent,
};
use uuid::Uuid;

use crate::{BackendError, EnforcementBackend, EventHub, SubscriberClass};

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

    #[cfg(test)]
    fn with_event_capacity(capacity: usize) -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            events: EventHub::new(capacity),
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
        Ok(self.events.reflect_delivery_loss(HealthStatus {
            ready: true,
            backend: "mock".into(),
            message: Some("mock backend does not enforce kernel operations".into()),
        }))
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

    fn subscribe(&self, id: Uuid, class: SubscriberClass) -> Receiver<ViolationEvent> {
        self.events.subscribe(id, class)
    }

    fn unsubscribe(&self, id: Uuid) {
        self.events.unsubscribe(id);
    }

    fn record_required_delivery_loss(&self, count: u64) {
        self.events.record_required_delivery_loss(count);
    }
}

#[cfg(test)]
mod tests {
    use agentsight_enforcement_protocol::Effect;

    use super::*;

    fn request() -> ApplyPolicy {
        ApplyPolicy {
            binding_id: Uuid::new_v4(),
            agent_id: "mock-health-test".into(),
            session_id: None,
            root_pid: 42,
            process_start_time: 99,
            policy_id: "policy".into(),
            policy_revision: "revision".into(),
            policy_dsl: "label AGENT".into(),
        }
    }

    fn violation(binding_id: Uuid) -> ViolationEvent {
        ViolationEvent {
            event_id: Uuid::new_v4(),
            binding_id,
            agent_id: "mock-health-test".into(),
            session_id: None,
            policy_id: "policy".into(),
            policy_revision: "revision".into(),
            pid: 42,
            ppid: Some(1),
            process_start_time: 99,
            operation: "open".into(),
            target: "/tmp/secret".into(),
            effect: Effect::Block,
            blocked: true,
            killed: false,
            rule_id: None,
            reason: None,
            occurred_at_ns: 100,
            observed_at_ns: 101,
            actplane_revision: "mock".into(),
        }
    }

    #[test]
    fn health_is_not_ready_after_the_violation_queue_overflows() {
        let backend = MockBackend::with_event_capacity(1);
        let binding = backend.apply(request()).expect("binding should apply");
        let _subscriber = backend.subscribe(Uuid::new_v4(), SubscriberClass::Required);

        for _ in 0..2 {
            backend
                .publish_violation(violation(binding.request.binding_id))
                .expect("active binding should publish");
        }

        let health = backend.health().expect("mock health should load");
        assert!(!health.ready);
        assert_eq!(
            health.message.as_deref(),
            Some(
                "mock backend does not enforce kernel operations; violation event delivery loss: dropped_events=1"
            )
        );
    }
}
