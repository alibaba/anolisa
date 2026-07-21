//! Deterministic backend for protocol and service tests.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, DestinationClass, Effect, EventIdentity, FileAction,
    HealthStatus, NetworkAction, NetworkDirection, PolicyDecision, PolicyMode, SecurityEvent,
    SecurityEventKind, TaintTransition, TaintTransitionKind, ViolationEvent,
};
use uuid::Uuid;

use crate::event_hub::SecurityEventHub;
use crate::{BackendError, EnforcementBackend, EventHub};

/// In-memory single-binding backend that performs no kernel operations.
pub struct MockBackend {
    bindings: Mutex<HashMap<Uuid, Binding>>,
    events: EventHub,
    security_events: SecurityEventHub,
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
            security_events: SecurityEventHub::default(),
        }
    }

    #[cfg(test)]
    fn with_event_capacity(capacity: usize) -> Self {
        Self {
            bindings: Mutex::new(HashMap::new()),
            events: EventHub::new(capacity),
            security_events: SecurityEventHub::new(capacity),
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

    /// Emits a deterministic source, taint, sink, and decision chain.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::MissingBinding`] when `binding_id` is unknown.
    pub fn emit_credential_exfiltration(
        &self,
        binding_id: Uuid,
        source_path: &str,
        destination: &str,
    ) -> Result<(), BackendError> {
        let binding = self
            .bindings()
            .get(&binding_id)
            .cloned()
            .ok_or(BackendError::MissingBinding(binding_id))?;
        let revision = binding.request.policy_revision.parse().unwrap_or(1);
        let mode = mock_policy_mode(&binding.request.policy_dsl);
        let identity = event_identity(&binding);
        let base_time = unix_epoch_ns();
        let source_event_id = Uuid::new_v4();
        let sink_event_id = Uuid::new_v4();

        let events = [
            SecurityEvent {
                event_id: source_event_id,
                occurred_at_ns: base_time,
                observed_at_ns: base_time,
                identity: identity.clone(),
                kind: SecurityEventKind::FileAction(FileAction {
                    policy_id: binding.request.policy_id.clone(),
                    policy_revision: revision,
                    operation: "read".into(),
                    path: redact_home_path(source_path),
                    resource_class: "credential".into(),
                    succeeded: true,
                    errno: None,
                    rule_id: Some("credential-source".into()),
                }),
            },
            SecurityEvent {
                event_id: Uuid::new_v4(),
                occurred_at_ns: base_time.saturating_add(1),
                observed_at_ns: base_time.saturating_add(1),
                identity: identity.clone(),
                kind: SecurityEventKind::TaintTransition(TaintTransition {
                    policy_id: binding.request.policy_id.clone(),
                    policy_revision: revision,
                    label: "credential".into(),
                    transition: TaintTransitionKind::Add,
                    source_pid: binding.request.root_pid,
                    source_process_start_time: binding.request.process_start_time,
                    target_pid: binding.request.root_pid,
                    target_process_start_time: binding.request.process_start_time,
                    reason: "sensitive credential source read".into(),
                }),
            },
            SecurityEvent {
                event_id: sink_event_id,
                occurred_at_ns: base_time.saturating_add(2),
                observed_at_ns: base_time.saturating_add(2),
                identity: identity.clone(),
                kind: SecurityEventKind::NetworkAction(NetworkAction {
                    policy_id: binding.request.policy_id.clone(),
                    policy_revision: revision,
                    direction: NetworkDirection::Outbound,
                    destination: destination.into(),
                    destination_class: DestinationClass::Public,
                    protocol: "tcp".into(),
                    succeeded: mode != PolicyMode::Enforce,
                    errno: (mode == PolicyMode::Enforce).then_some(libc::EPERM),
                    rule_id: Some("credential-public-sink".into()),
                }),
            },
            SecurityEvent {
                event_id: Uuid::new_v4(),
                occurred_at_ns: base_time.saturating_add(3),
                observed_at_ns: base_time.saturating_add(3),
                identity,
                kind: SecurityEventKind::PolicyDecision(PolicyDecision {
                    policy_id: binding.request.policy_id,
                    policy_revision: revision,
                    source_event_id,
                    sink_event_id,
                    mode,
                    requested_effect: if mode == PolicyMode::Enforce {
                        Effect::Block
                    } else {
                        Effect::Notify
                    },
                    blocked: mode == PolicyMode::Enforce,
                    killed: false,
                    errno: (mode == PolicyMode::Enforce).then_some(libc::EPERM),
                    risk_score: 85,
                    reason: "credential taint reached unknown public endpoint".into(),
                }),
            },
        ];
        for event in events {
            self.security_events.publish(event);
        }
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
        let health = self.events.reflect_delivery_loss(HealthStatus {
            ready: true,
            backend: "mock".into(),
            message: Some("mock backend does not enforce kernel operations".into()),
        });
        Ok(self.security_events.reflect_delivery_loss(health))
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

    fn subscribe_security_events(&self) -> Receiver<SecurityEvent> {
        self.security_events.subscribe()
    }
}

fn event_identity(binding: &Binding) -> EventIdentity {
    EventIdentity {
        binding_id: binding.request.binding_id,
        agent_id: binding.request.agent_id.clone(),
        agent_name: Some("mock-agent".into()),
        session_id: binding.request.session_id.clone(),
        conversation_id: None,
        tool_call_id: None,
        pid: binding.request.root_pid,
        process_start_time: binding.request.process_start_time,
        ppid: None,
        cgroup_id: None,
        protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
        enforcer_version: env!("CARGO_PKG_VERSION").into(),
        actplane_revision: "mock".into(),
    }
}

fn mock_policy_mode(policy_dsl: &str) -> PolicyMode {
    if policy_dsl.split_whitespace().any(|word| word == "enforce") {
        PolicyMode::Enforce
    } else if policy_dsl.split_whitespace().any(|word| word == "observe") {
        PolicyMode::Observe
    } else {
        PolicyMode::Audit
    }
}

fn redact_home_path(path: &str) -> String {
    if let Some(relative) = path.strip_prefix("/root/") {
        return format!("~/{relative}");
    }
    let Some(home_relative) = path.strip_prefix("/home/") else {
        return path.into();
    };
    let Some((_, relative)) = home_relative.split_once('/') else {
        return path.into();
    };
    format!("~/{relative}")
}

fn unix_epoch_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.min(u128::from(u64::MAX)) as u64
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
        let _subscriber = backend.subscribe();

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
