//! Composition of independently lowered restrictive policies.

use std::collections::HashMap;

use asc_policy_types::Validate;
use asc_policy_types::identifiers::{PolicyId, ResourceSetId, Revision, RuleId};
use asc_policy_types::ir::CanonicalPolicyIr;
use asc_policy_types::policy::PolicyEnvelope;

use crate::EngineError;

/// Composes policies into one Canonical IR payload using collision-free local IDs.
///
/// Identical duplicate revisions are ignored. Reusing the same policy identity
/// and revision for different content is rejected.
///
/// # Errors
/// Returns an error for invalid input, incompatible profiles or guarantees, or
/// an invalid composed IR.
pub fn compose_policies(
    output_policy_id: PolicyId,
    output_revision: Revision,
    policies: &[PolicyEnvelope],
) -> Result<PolicyEnvelope, EngineError> {
    let Some(first) = policies.first() else {
        return Err(EngineError::EmptyPolicySet);
    };

    let mut unique: HashMap<(&PolicyId, Revision), &PolicyEnvelope> = HashMap::new();
    for policy in policies {
        policy.validate()?;
        if policy.ir_schema_version != first.ir_schema_version
            || policy.profile_id != first.profile_id
        {
            return Err(EngineError::ProfileMismatch);
        }
        let key = (&policy.policy_id, policy.revision);
        if let Some(previous) = unique.insert(key, policy)
            && previous != policy
        {
            return Err(EngineError::ConflictingRevision {
                policy_id: policy.policy_id.to_string(),
                revision: policy.revision.get(),
            });
        }
    }

    let mut resources = Vec::new();
    let mut rules = Vec::new();
    let mut activation = first.payload.activation;
    let failure_policy = first.payload.failure_policy;

    for policy in unique.values() {
        if policy.payload.failure_policy != failure_policy {
            return Err(EngineError::IncompatibleFailurePolicy);
        }
        activation = activation.stricter(policy.payload.activation);

        let mut remapping = HashMap::new();
        for resource in &policy.payload.resources {
            let new_id = namespaced_resource_id(policy, &resource.id)?;
            remapping.insert(resource.id.clone(), new_id.clone());
            let mut resource = resource.clone();
            resource.id = new_id;
            resources.push(resource);
        }
        for rule in &policy.payload.rules {
            let mut rule = rule.clone();
            rule.id = namespaced_rule_id(policy, &rule.id)?;
            rule.when
                .remap_resources(|old| remapping.get(old).cloned().unwrap_or_else(|| old.clone()));
            rules.push(rule);
        }
    }

    let composed = PolicyEnvelope {
        ir_schema_version: first.ir_schema_version,
        profile_id: first.profile_id.clone(),
        policy_id: output_policy_id,
        revision: output_revision,
        payload_digest: None,
        payload: CanonicalPolicyIr {
            resources,
            rules,
            activation,
            failure_policy,
        },
    };
    composed.validate()?;
    Ok(composed)
}

fn namespaced_resource_id(
    policy: &PolicyEnvelope,
    local: &ResourceSetId,
) -> Result<ResourceSetId, EngineError> {
    ResourceSetId::new(format!(
        "{}:{}:{}",
        policy.policy_id,
        policy.revision.get(),
        local
    ))
    .map_err(EngineError::Identifier)
}

fn namespaced_rule_id(policy: &PolicyEnvelope, local: &RuleId) -> Result<RuleId, EngineError> {
    RuleId::new(format!(
        "{}:{}:{}",
        policy.policy_id,
        policy.revision.get(),
        local
    ))
    .map_err(EngineError::Identifier)
}
