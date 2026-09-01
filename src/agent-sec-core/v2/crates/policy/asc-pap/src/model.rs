use asc_foundation_types::{ResourceId, Revision};
use asc_policy_engine::PolicyTemplate;
use asc_policy_types::policy::PolicyEnvelope;
use serde::{Deserialize, Serialize};

use crate::scope::{ScopeSelector, ScopeTemplate};

/// Durable Policy revision with its deterministic lowered form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedPolicy {
    /// Stable product policy identity.
    pub policy_id: ResourceId,
    /// Human-readable policy name; it is not unique.
    pub policy_name: String,
    /// Immutable revision.
    pub revision: Revision,
    /// Product authoring input.
    pub template: PolicyTemplate,
    /// Backend-independent lowered policy.
    pub canonical_policy: PolicyEnvelope,
    /// Digest over the exact authored template JSON.
    pub template_digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedPolicyWire {
    policy_id: ResourceId,
    policy_name: String,
    revision: Revision,
    template: PolicyTemplate,
    canonical_policy: PolicyEnvelope,
    template_digest: String,
    #[serde(default, rename = "retired")]
    _legacy_retired: bool,
}

impl<'de> Deserialize<'de> for PreparedPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = PreparedPolicyWire::deserialize(deserializer)?;
        Ok(Self {
            policy_id: wire.policy_id,
            policy_name: wire.policy_name,
            revision: wire.revision,
            template: wire.template,
            canonical_policy: wire.canonical_policy,
            template_digest: wire.template_digest,
        })
    }
}

/// Durable allocation state for one Policy identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRevisionState {
    /// Highest revision ever allocated, including explicitly deleted revisions.
    pub last_allocated_revision: Revision,
    /// Highest Policy revision whose complete content still exists.
    pub latest: Option<PreparedPolicy>,
}

/// Durable Scope revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedScope {
    /// Stable scope identity.
    pub scope_id: ResourceId,
    /// Immutable revision.
    pub revision: Revision,
    /// Unresolved user selector intent.
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

/// Durable allocation state for one Scope identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRevisionState {
    /// Highest revision ever allocated, including explicitly deleted revisions.
    pub last_allocated_revision: Revision,
    /// Highest Scope revision whose complete content still exists.
    pub latest: Option<PreparedScope>,
}

/// Query result page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T> {
    /// Selected records.
    pub items: Vec<T>,
    /// Total records before pagination.
    pub total: u64,
}

#[cfg(test)]
mod tests {
    use asc_policy_engine::{PolicyTemplate, TemplateEnvelope, lower_template};
    use asc_policy_types::identifiers::{PolicyId, Revision as PolicyRevision};

    use super::*;

    #[test]
    fn legacy_policy_json_ignores_only_the_removed_retired_field() {
        let template = PolicyTemplate::HighSensitivityReadDeny {
            files: vec!["/secrets/**".to_owned()],
        };
        let policy_id = ResourceId::new("6efed5ea-47c9-4b14-8e86-888f2ad88fc7").unwrap();
        let revision = Revision::new(1).unwrap();
        let policy = PreparedPolicy {
            policy_id: policy_id.clone(),
            policy_name: "legacy-policy".to_owned(),
            revision,
            template: template.clone(),
            canonical_policy: lower_template(TemplateEnvelope {
                policy_id: PolicyId::new(policy_id.as_str()).unwrap(),
                revision: PolicyRevision::new(1).unwrap(),
                template,
            })
            .unwrap(),
            template_digest: "sha256:legacy".to_owned(),
        };
        let mut legacy = serde_json::to_value(&policy).unwrap();
        legacy["retired"] = serde_json::json!(false);

        assert_eq!(
            serde_json::from_value::<PreparedPolicy>(legacy).unwrap(),
            policy
        );
        assert!(
            serde_json::to_value(policy)
                .unwrap()
                .get("retired")
                .is_none()
        );
    }
}
