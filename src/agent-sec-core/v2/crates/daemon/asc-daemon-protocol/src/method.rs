//! Stable daemon method inventory grouped by capability.

pub use crate::health::DAEMON_HEALTH;
pub use crate::policy::method::{
    POLICY_BINDINGS_DELETE, POLICY_BINDINGS_GET, POLICY_BINDINGS_LIST, POLICY_BINDINGS_PUT,
    POLICY_SCOPES_DELETE, POLICY_SCOPES_GET, POLICY_SCOPES_LIST, POLICY_SCOPES_PUT,
    POLICY_TEMPLATES_DELETE, POLICY_TEMPLATES_GET, POLICY_TEMPLATES_LIST, POLICY_TEMPLATES_PUT,
};

/// Daemon capability owning a registered method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Process lifecycle and readiness.
    Health,
    /// Policy administration and Binding intent.
    Policy,
}

/// Access policy declared by a registered method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPolicy {
    /// No management credential is required.
    Public,
    /// The local management credential is required.
    ManagementCredential,
}

/// Stable metadata attached to one registered method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    /// Owning daemon capability.
    pub capability: Capability,
    /// Required access policy.
    pub access: AccessPolicy,
}

/// Looks up one method in the closed daemon registry.
pub fn metadata(method: &str) -> Option<Metadata> {
    if method == DAEMON_HEALTH {
        return Some(Metadata {
            capability: Capability::Health,
            access: AccessPolicy::Public,
        });
    }
    if crate::policy::method::is_policy(method) {
        return Some(Metadata {
            capability: Capability::Policy,
            access: AccessPolicy::ManagementCredential,
        });
    }
    None
}

/// Returns whether a method belongs to the protected Policy surface.
pub fn is_policy(method: &str) -> bool {
    metadata(method).is_some_and(|value| value.capability == Capability::Policy)
}
