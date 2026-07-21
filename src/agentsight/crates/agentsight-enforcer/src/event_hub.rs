//! Bounded non-blocking fan-out for raw violations and normalized security events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::{
    EnforcementStateEvent, HealthStatus, SecurityEvent, SecurityEventKind, ViolationEvent,
};
use uuid::Uuid;

/// Default pending events retained for each subscriber.
const DEFAULT_SUBSCRIBER_CAPACITY: usize = 256;

/// Non-blocking bounded publisher for violation events.
pub struct EventHub {
    capacity: usize,
    subscribers: Mutex<Vec<SyncSender<ViolationEvent>>>,
    dropped_events: AtomicU64,
}

/// Non-blocking bounded publisher for normalized security events.
pub(crate) struct SecurityEventHub {
    capacity: usize,
    subscribers: Mutex<Vec<SyncSender<SecurityEvent>>>,
    dropped_events: AtomicU64,
    reported_dropped_events: AtomicU64,
    last_event: Mutex<Option<SecurityEvent>>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new(DEFAULT_SUBSCRIBER_CAPACITY)
    }
}

impl EventHub {
    /// Creates a hub with a bounded queue for every subscriber.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            subscribers: Mutex::new(Vec::new()),
            dropped_events: AtomicU64::new(0),
        }
    }

    /// Registers an independent subscriber.
    pub fn subscribe(&self) -> Receiver<ViolationEvent> {
        let (sender, receiver) = mpsc::sync_channel(self.capacity);
        self.subscribers().push(sender);
        receiver
    }

    /// Publishes without allowing a slow subscriber to block policy lifecycle.
    pub fn publish(&self, event: ViolationEvent) {
        let mut delivered = false;
        self.subscribers()
            .retain(|subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => {
                    delivered = true;
                    true
                }
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
        if !delivered {
            // The count is diagnostic and sticky, so it does not order event data.
            let _ =
                self.dropped_events
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        Some(count.saturating_add(1))
                    });
        }
    }

    /// Returns events not delivered to any subscriber since this hub was created.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    /// Marks backend health degraded after any event has no successful delivery.
    pub(crate) fn reflect_delivery_loss(&self, mut health: HealthStatus) -> HealthStatus {
        let dropped_events = self.dropped_events();
        if dropped_events == 0 {
            return health;
        }
        let delivery_loss =
            format!("violation event delivery loss: dropped_events={dropped_events}");
        health.ready = false;
        health.message = Some(match health.message.take() {
            Some(message) if !message.is_empty() => format!("{message}; {delivery_loss}"),
            _ => delivery_loss,
        });
        health
    }

    fn subscribers(&self) -> MutexGuard<'_, Vec<SyncSender<ViolationEvent>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for SecurityEventHub {
    fn default() -> Self {
        Self::new(DEFAULT_SUBSCRIBER_CAPACITY)
    }
}

impl SecurityEventHub {
    /// Creates a security hub with a bounded queue for every subscriber.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            subscribers: Mutex::new(Vec::new()),
            dropped_events: AtomicU64::new(0),
            reported_dropped_events: AtomicU64::new(0),
            last_event: Mutex::new(None),
        }
    }

    /// Registers an independent normalized security-event subscriber.
    pub(crate) fn subscribe(&self) -> Receiver<SecurityEvent> {
        let (sender, receiver) = mpsc::sync_channel(self.capacity);
        self.subscribers().push(sender);
        self.publish_pending_loss();
        receiver
    }

    /// Publishes without allowing evidence delivery to block policy decisions.
    // ActPlane publishes only after its raw provenance has been normalized.
    #[cfg_attr(not(feature = "mock-backend"), allow(dead_code))]
    pub(crate) fn publish(&self, event: SecurityEvent) {
        self.publish_pending_loss();
        *self.last_event() = Some(event.clone());
        if !self.try_publish(&event) {
            let _ =
                self.dropped_events
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        Some(count.saturating_add(1))
                    });
        }
    }

    fn publish_pending_loss(&self) {
        let dropped_events = self.dropped_events.load(Ordering::Relaxed);
        if dropped_events == self.reported_dropped_events.load(Ordering::Relaxed) {
            return;
        }
        let Some(last_event) = self.last_event().clone() else {
            return;
        };
        let (policy_id, policy_revision) = event_policy(&last_event.kind);
        let recovery = SecurityEvent {
            event_id: Uuid::new_v4(),
            occurred_at_ns: last_event.occurred_at_ns.saturating_add(1),
            observed_at_ns: last_event.observed_at_ns.saturating_add(1),
            identity: last_event.identity,
            kind: SecurityEventKind::EnforcementState(EnforcementStateEvent {
                policy_id,
                policy_revision,
                code: "evidence_loss".into(),
                ready: false,
                message: format!("security event delivery loss: dropped_events={dropped_events}"),
                dropped_events: Some(dropped_events),
            }),
        };
        if self.try_publish(&recovery) {
            self.reported_dropped_events
                .store(dropped_events, Ordering::Relaxed);
        }
    }

    fn try_publish(&self, event: &SecurityEvent) -> bool {
        let mut delivered = false;
        self.subscribers()
            .retain(|subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => {
                    delivered = true;
                    true
                }
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
        delivered
    }

    /// Marks backend health degraded after normalized evidence delivery loss.
    pub(crate) fn reflect_delivery_loss(&self, mut health: HealthStatus) -> HealthStatus {
        let dropped_events = self.dropped_events.load(Ordering::Relaxed);
        if dropped_events == 0 {
            return health;
        }
        let delivery_loss =
            format!("security event delivery loss: dropped_events={dropped_events}");
        health.ready = false;
        health.message = Some(match health.message.take() {
            Some(message) if !message.is_empty() => format!("{message}; {delivery_loss}"),
            _ => delivery_loss,
        });
        health
    }

    fn subscribers(&self) -> MutexGuard<'_, Vec<SyncSender<SecurityEvent>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn last_event(&self) -> MutexGuard<'_, Option<SecurityEvent>> {
        self.last_event
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn event_policy(kind: &SecurityEventKind) -> (Option<String>, Option<u64>) {
    match kind {
        SecurityEventKind::FileAction(event) => {
            (Some(event.policy_id.clone()), Some(event.policy_revision))
        }
        SecurityEventKind::TaintTransition(event) => {
            (Some(event.policy_id.clone()), Some(event.policy_revision))
        }
        SecurityEventKind::NetworkAction(event) => {
            (Some(event.policy_id.clone()), Some(event.policy_revision))
        }
        SecurityEventKind::PolicyDecision(event) => {
            (Some(event.policy_id.clone()), Some(event.policy_revision))
        }
        SecurityEventKind::EnforcementState(event) => {
            (event.policy_id.clone(), event.policy_revision)
        }
    }
}

#[cfg(test)]
mod tests {
    use agentsight_enforcement_protocol::{
        Effect, EnforcementStateEvent, EventIdentity, HealthStatus, SecurityEvent,
        SecurityEventKind, ViolationEvent,
    };
    use uuid::Uuid;

    use super::*;

    fn violation() -> ViolationEvent {
        ViolationEvent {
            event_id: Uuid::new_v4(),
            binding_id: Uuid::new_v4(),
            agent_id: "event-hub-test".into(),
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
            actplane_revision: "test".into(),
        }
    }

    fn security_event() -> SecurityEvent {
        SecurityEvent {
            event_id: Uuid::new_v4(),
            occurred_at_ns: 100,
            observed_at_ns: 101,
            identity: EventIdentity {
                binding_id: Uuid::new_v4(),
                agent_id: "event-hub-test".into(),
                agent_name: None,
                session_id: None,
                conversation_id: None,
                tool_call_id: None,
                pid: 42,
                process_start_time: 99,
                ppid: Some(1),
                cgroup_id: None,
                protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
                enforcer_version: "test".into(),
                actplane_revision: "test".into(),
            },
            kind: SecurityEventKind::EnforcementState(EnforcementStateEvent {
                policy_id: Some("policy".into()),
                policy_revision: Some(1),
                code: "fixture".into(),
                ready: true,
                message: "fixture".into(),
                dropped_events: None,
            }),
        }
    }

    #[test]
    fn full_subscriber_records_a_sticky_dropped_event_count() {
        let hub = EventHub::new(1);
        let subscriber = hub.subscribe();

        hub.publish(violation());
        hub.publish(violation());
        hub.publish(violation());
        assert_eq!(hub.dropped_events(), 2);

        subscriber
            .try_recv()
            .expect("the first event should remain queued");
        hub.publish(violation());
        assert_eq!(hub.dropped_events(), 2);
    }

    #[test]
    fn disconnected_subscriber_is_pruned_and_the_undelivered_event_is_recorded() {
        let hub = EventHub::new(1);
        let subscriber = hub.subscribe();
        drop(subscriber);

        hub.publish(violation());

        assert_eq!(hub.dropped_events(), 1);
        assert!(hub.subscribers().is_empty());
        assert!(
            !hub.reflect_delivery_loss(HealthStatus {
                ready: true,
                backend: "test".into(),
                message: None,
            })
            .ready
        );
    }

    #[test]
    fn event_without_subscribers_is_recorded_as_dropped() {
        let hub = EventHub::new(1);

        hub.publish(violation());

        assert_eq!(hub.dropped_events(), 1);
        assert!(
            !hub.reflect_delivery_loss(HealthStatus {
                ready: true,
                backend: "test".into(),
                message: None,
            })
            .ready
        );
    }

    #[test]
    fn one_successful_delivery_keeps_health_ready_when_another_queue_is_full() {
        let hub = EventHub::new(1);
        let fast_subscriber = hub.subscribe();
        let _slow_subscriber = hub.subscribe();

        hub.publish(violation());
        fast_subscriber
            .try_recv()
            .expect("the fast subscriber should drain its queue");
        hub.publish(violation());

        assert_eq!(hub.dropped_events(), 0);
        assert!(
            hub.reflect_delivery_loss(HealthStatus {
                ready: true,
                backend: "test".into(),
                message: None,
            })
            .ready
        );
    }

    #[test]
    fn overflow_degrades_health_with_the_sticky_cumulative_count() {
        let hub = EventHub::new(1);
        let _subscriber = hub.subscribe();
        hub.publish(violation());
        hub.publish(violation());
        hub.publish(violation());

        let health = hub.reflect_delivery_loss(HealthStatus {
            ready: true,
            backend: "test".into(),
            message: Some("runtime healthy".into()),
        });

        assert!(!health.ready);
        assert_eq!(
            health.message.as_deref(),
            Some("runtime healthy; violation event delivery loss: dropped_events=2")
        );
    }

    #[test]
    fn recovering_security_subscriber_receives_one_evidence_loss_event() {
        let hub = SecurityEventHub::new(4);
        hub.publish(security_event());

        let subscriber = hub.subscribe();
        let recovered = subscriber
            .try_recv()
            .expect("recovery must disclose lost evidence");
        let SecurityEventKind::EnforcementState(state) = recovered.kind else {
            panic!("recovery frame must be enforcement state");
        };
        assert_eq!(state.code, "evidence_loss");
        assert_eq!(state.dropped_events, Some(1));
        assert!(subscriber.try_recv().is_err());
    }
}
