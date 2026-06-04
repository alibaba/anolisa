//! Subscription management (placeholder).

/// Placeholder for subscription registration/status.
pub struct SubscriptionManager;

impl SubscriptionManager {
    /// Register a host/org against a future subscription backend.
    pub fn register(&self, _org: &str, _key: &str) -> Result<(), String> {
        todo!("owner: subscription-core; when subscription API contract ships; register")
    }

    /// Remove the current subscription registration.
    pub fn unregister(&self) -> Result<(), String> {
        todo!("owner: subscription-core; when subscription API contract ships; unregister")
    }

    /// Return the locally known subscription state.
    pub fn status(&self) -> SubscriptionStatus {
        SubscriptionStatus::Unregistered
    }
}

/// Subscription state surfaced by subscription commands.
#[derive(Debug)]
pub enum SubscriptionStatus {
    /// Registration is active.
    Active {
        /// Organization identifier.
        org: String,
        /// Expiration timestamp or date string.
        expires: String,
    },
    /// Registration exists but no longer grants entitlement.
    Expired,
    /// No registration is present.
    Unregistered,
}
