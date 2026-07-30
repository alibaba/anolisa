//! Bounded fan-out for violation subscriptions.

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::ViolationEvent;

/// Default pending events retained for each subscriber.
const DEFAULT_SUBSCRIBER_CAPACITY: usize = 256;

/// Non-blocking bounded publisher for violation events.
pub struct EventHub {
    capacity: usize,
    subscribers: Mutex<Vec<SyncSender<ViolationEvent>>>,
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
        self.subscribers()
            .retain(|subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
    }

    fn subscribers(&self) -> MutexGuard<'_, Vec<SyncSender<ViolationEvent>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
