//! Subscription management (placeholder).

/// Placeholder for subscription registration/status.
pub struct SubscriptionManager;

impl SubscriptionManager {
    pub fn register(&self, _org: &str, _key: &str) -> Result<(), String> {
        todo!("subscription register")
    }

    pub fn unregister(&self) -> Result<(), String> {
        todo!("subscription unregister")
    }

    pub fn status(&self) -> SubscriptionStatus {
        SubscriptionStatus::Unregistered
    }
}

#[derive(Debug)]
pub enum SubscriptionStatus {
    Active { org: String, expires: String },
    Expired,
    Unregistered,
}
