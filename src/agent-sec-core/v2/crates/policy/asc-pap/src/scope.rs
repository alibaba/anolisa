use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use asc_foundation_types::ResourceId;

use crate::error::PapError;

/// Caller intent used to locate a future trusted execution-domain identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScopeSelector {
    /// Caller-observed process id. PID reuse must be handled by future resolution.
    Pid { pid: u32 },
    /// Caller-observed cgroup id.
    CgroupId { cgroup_id: u64 },
    /// Compatibility selector for pre-selector stored Scopes.
    LegacyExecutionDomain { execution_domain_id: ResourceId },
}

impl ScopeSelector {
    /// Rejects invalid zero-valued runtime selectors.
    ///
    /// # Errors
    /// Returns a stable Scope validation error.
    pub fn validate(&self) -> Result<(), PapError> {
        match self {
            Self::Pid { pid: 0 } => Err(PapError::InvalidScope("pid must be positive".to_owned())),
            Self::CgroupId { cgroup_id: 0 } => Err(PapError::InvalidScope(
                "cgroupId must be positive".to_owned(),
            )),
            Self::Pid { .. } | Self::CgroupId { .. } | Self::LegacyExecutionDomain { .. } => Ok(()),
        }
    }
}

/// Immutable product-level Scope revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeTemplate {
    /// Scope kind. Phase one accepts only `execution_domain`.
    pub kind: ScopeKind,
    /// Process coverage profile.
    pub process_membership: ProcessMembership,
    /// Constraints survive exec.
    pub preserve_across_exec: bool,
    /// Nested execution domains inherit parent constraints.
    pub nested_execution_domains: NestedExecutionDomains,
    /// Unapproved namespace changes are denied.
    pub namespace_change: NamespaceChange,
    /// Binding lifetime.
    pub lifetime: ScopeLifetime,
}

/// Supported product scope kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// A logical execution domain resolved by a future Adapter.
    ExecutionDomain,
}

/// Supported phase-one process membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessMembership {
    /// Root and members joining after binding, excluding existing children.
    RootAndFutureMembers,
}

/// Nested execution-domain behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NestedExecutionDomains {
    /// Child domains inherit and may only narrow constraints.
    Inherit,
}

/// Namespace transition behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceChange {
    /// Reject unapproved transitions.
    Deny,
}

/// Product-level lifetime intent without kernel identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeLifetime {
    /// Activation boundary.
    pub activate_at: ScopeActivation,
    /// Optional RFC 3339 timestamp.
    pub expires_at: Option<String>,
    /// Normal termination boundary.
    pub end_condition: ScopeEndCondition,
}

/// Activation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeActivation {
    /// Activation begins only after the Adapter reports its own readiness later.
    BindingReady,
}

/// Scope end condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeEndCondition {
    /// End after the logical execution domain drains.
    ExecutionDomainDrained,
}

impl ScopeTemplate {
    /// Returns the fixed first-phase execution-domain behavior.
    pub fn execution_domain_default() -> Self {
        Self {
            kind: ScopeKind::ExecutionDomain,
            process_membership: ProcessMembership::RootAndFutureMembers,
            preserve_across_exec: true,
            nested_execution_domains: NestedExecutionDomains::Inherit,
            namespace_change: NamespaceChange::Deny,
            lifetime: ScopeLifetime {
                activate_at: ScopeActivation::BindingReady,
                expires_at: None,
                end_condition: ScopeEndCondition::ExecutionDomainDrained,
            },
        }
    }

    /// Validates the supported phase-one Scope profile.
    ///
    /// # Errors
    /// Returns a stable validation error for unsupported values.
    pub fn validate(&self) -> Result<(), PapError> {
        if !self.preserve_across_exec {
            return Err(PapError::InvalidScope(
                "preserveAcrossExec must be true".to_owned(),
            ));
        }
        if self
            .lifetime
            .expires_at
            .as_ref()
            .is_some_and(|value| OffsetDateTime::parse(value, &Rfc3339).is_err())
        {
            return Err(PapError::InvalidScope(
                "lifetime.expiresAt must be an RFC 3339 timestamp".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_with_expiry(expires_at: &str) -> ScopeTemplate {
        ScopeTemplate {
            kind: ScopeKind::ExecutionDomain,
            process_membership: ProcessMembership::RootAndFutureMembers,
            preserve_across_exec: true,
            nested_execution_domains: NestedExecutionDomains::Inherit,
            namespace_change: NamespaceChange::Deny,
            lifetime: ScopeLifetime {
                activate_at: ScopeActivation::BindingReady,
                expires_at: Some(expires_at.to_owned()),
                end_condition: ScopeEndCondition::ExecutionDomainDrained,
            },
        }
    }

    #[test]
    fn scope_expiry_requires_a_real_rfc3339_timestamp() {
        assert!(
            scope_with_expiry("2026-08-27T12:34:56+08:00")
                .validate()
                .is_ok()
        );
        assert!(
            scope_with_expiry("2026-99-99T99:99:99Z")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn runtime_selectors_must_be_positive() {
        assert!(ScopeSelector::Pid { pid: 0 }.validate().is_err());
        assert!(ScopeSelector::CgroupId { cgroup_id: 0 }.validate().is_err());
        assert!(ScopeSelector::Pid { pid: 1 }.validate().is_ok());
    }
}
