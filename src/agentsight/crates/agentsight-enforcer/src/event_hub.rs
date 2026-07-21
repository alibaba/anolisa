//! Bounded fan-out for violation subscriptions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::{HealthStatus, ViolationEvent};

/// Default pending events retained for each subscriber.
const DEFAULT_SUBSCRIBER_CAPACITY: usize = 256;

/// Non-blocking bounded publisher for violation events.
pub struct EventHub {
    capacity: usize,
    subscribers: Mutex<Vec<SyncSender<ViolationEvent>>>,
    dropped_events: AtomicU64,
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

#[cfg(test)]
mod tests {
    use agentsight_enforcement_protocol::{Effect, HealthStatus, ViolationEvent};
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
}
