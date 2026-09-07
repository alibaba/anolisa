//! Target-independent Policy Scope contracts.

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::error::{Validate, ValidationError};
use crate::identifiers::{Digest, ResourceId, Revision};

/// Caller intent used to locate a future trusted execution-domain identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScopeSelector {
    /// Caller-observed process id. PID reuse is handled during target resolution.
    Pid { pid: u32 },
    /// Caller-observed cgroup id.
    CgroupId { cgroup_id: u64 },
    /// Compatibility selector for pre-selector stored Scopes.
    LegacyExecutionDomain { execution_domain_id: ResourceId },
}

impl Validate for ScopeSelector {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Pid { pid: 0 } => Err(ValidationError::new("pid", "must be positive")),
            Self::CgroupId { cgroup_id: 0 } => {
                Err(ValidationError::new("cgroupId", "must be positive"))
            }
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
    /// Whether constraints survive exec.
    pub preserve_across_exec: bool,
    /// Nested execution-domain inheritance.
    pub nested_execution_domains: NestedExecutionDomains,
    /// Namespace transition behavior.
    pub namespace_change: NamespaceChange,
    /// Binding lifetime.
    pub lifetime: ScopeLifetime,
}

/// Supported product scope kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// A logical execution domain resolved by a target Adapter.
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

/// Product-level lifetime intent without target-specific identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeLifetime {
    /// Activation boundary.
    pub activate_at: ScopeActivation,
    /// Optional RFC 3339 expiration timestamp.
    pub expires_at: Option<String>,
    /// Normal termination boundary.
    pub end_condition: ScopeEndCondition,
}

/// Activation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeActivation {
    /// Activation begins only after the Adapter reports readiness.
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
}

impl Validate for ScopeTemplate {
    fn validate(&self) -> Result<(), ValidationError> {
        if !self.preserve_across_exec {
            return Err(ValidationError::new(
                "preserveAcrossExec",
                "must be true for the phase-one Scope profile",
            ));
        }
        if self
            .lifetime
            .expires_at
            .as_ref()
            .is_some_and(|value| OffsetDateTime::parse(value, &Rfc3339).is_err())
        {
            return Err(ValidationError::new(
                "lifetime.expiresAt",
                "must be an RFC 3339 timestamp",
            ));
        }
        Ok(())
    }
}

/// Durable Scope revision with unresolved selector intent and validated behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedScope {
    /// Stable scope identity.
    pub scope_id: ResourceId,
    /// Immutable revision.
    pub revision: Revision,
    /// Unresolved caller selector intent.
    pub selector: ScopeSelector,
    /// Validated scope intent.
    pub template: ScopeTemplate,
    /// Digest over the authored scope JSON.
    pub template_digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedScopeWire {
    scope_id: ResourceId,
    revision: Revision,
    #[serde(default)]
    selector: Option<ScopeSelector>,
    template: ScopeTemplate,
    template_digest: String,
    #[serde(default, rename = "retired")]
    _legacy_retired: bool,
}

impl<'de> Deserialize<'de> for PreparedScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = PreparedScopeWire::deserialize(deserializer)?;
        let selector = wire
            .selector
            .unwrap_or_else(|| ScopeSelector::LegacyExecutionDomain {
                execution_domain_id: wire.scope_id.clone(),
            });
        Ok(Self {
            scope_id: wire.scope_id,
            revision: wire.revision,
            selector,
            template: wire.template,
            template_digest: wire.template_digest,
        })
    }
}

impl Validate for PreparedScope {
    fn validate(&self) -> Result<(), ValidationError> {
        self.selector.validate().map_err(|error| {
            ValidationError::new(format!("selector.{}", error.path), error.message)
        })?;
        self.template.validate().map_err(|error| {
            ValidationError::new(format!("template.{}", error.path), error.message)
        })?;
        Digest::new(&self.template_digest).map_err(|message| {
            ValidationError::new(
                "templateDigest",
                format!("invalid template digest: {message}"),
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_validation_rejects_unusable_selectors_and_expiration() {
        assert_eq!(
            ScopeSelector::Pid { pid: 0 }.validate().unwrap_err().path,
            "pid"
        );

        let mut template = ScopeTemplate::execution_domain_default();
        template.lifetime.expires_at = Some("tomorrow".to_owned());
        assert_eq!(template.validate().unwrap_err().path, "lifetime.expiresAt");
    }
}
