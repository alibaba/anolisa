//! Deterministic lowering from product templates to Canonical Policy IR.

use asc_policy_types::Validate;
use asc_policy_types::identifiers::{ProfileId, ResourceSetId, RuleId};
use asc_policy_types::ir::{
    ActivationRequirement, CanonicalPolicyIr, DecisionTiming, EvidenceRequirement, Expression,
    FailurePolicy, FlowPropagation, Obligation, ResourceOperation, ResourceTarget,
    RestrictiveDecision, RuleEnforcement, RuleIr, RuleOutcome, RuntimeFailurePolicy, SemanticAtom,
    SubjectRemediation, UpdateFailurePolicy,
};
use asc_policy_types::policy::PolicyEnvelope;
use asc_policy_types::profile::{IR_SCHEMA_VERSION_V1, PROFILE_V1ALPHA1_DEMO1};
use asc_policy_types::resource::{
    EndpointMatcher, FileMatcher, FileResolution, PathMatcher, ResourceSelector, ResourceSet,
};

use crate::{EngineError, PolicyTemplate, TemplateEnvelope, TrustedDestination};

type LoweredPayload = (Vec<ResourceSet>, Vec<RuleIr>);

/// Lowers one validated product template into the immutable phase-one profile.
///
/// # Errors
/// Returns an error when template selectors or the resulting IR are invalid.
pub fn lower_template(template: TemplateEnvelope) -> Result<PolicyEnvelope, EngineError> {
    let (resources, rules) = lower_payload(template.template)?;

    let policy = PolicyEnvelope {
        ir_schema_version: IR_SCHEMA_VERSION_V1,
        profile_id: ProfileId::new(PROFILE_V1ALPHA1_DEMO1).map_err(EngineError::Identifier)?,
        policy_id: template.policy_id,
        revision: template.revision,
        payload_digest: None,
        payload: CanonicalPolicyIr {
            resources,
            rules,
            activation: ActivationRequirement::PostAttachAllowed,
            failure_policy: FailurePolicy {
                runtime: RuntimeFailurePolicy::FailClosed,
                update: UpdateFailurePolicy::KeepLastKnownGood,
            },
        },
    };
    policy.validate()?;
    Ok(policy)
}

fn lower_payload(template: PolicyTemplate) -> Result<LoweredPayload, EngineError> {
    match template {
        PolicyTemplate::HighSensitivityReadDeny { files } => lower_file_operation(
            files,
            "high-sensitive-files",
            final_object_resolution(),
            "deny-high-sensitive-read",
            ResourceOperation::Read,
            &[EvidenceRequirement::OperationDenied],
        ),
        PolicyTemplate::PreventFileDeletion { files } => lower_file_operation(
            files,
            "protected-file-entries",
            final_object_resolution(),
            "deny-protected-file-namespace-mutation",
            ResourceOperation::NamespaceMutation,
            &[
                EvidenceRequirement::BindingReady,
                EvidenceRequirement::OperationDenied,
            ],
        ),
        PolicyTemplate::LowSensitivityEgress {
            files,
            trusted_destinations,
        } => lower_information_flow(files, trusted_destinations),
    }
}

fn lower_file_operation(
    files: Vec<String>,
    resource_name: &str,
    resolution: FileResolution,
    rule_name: &str,
    operation: ResourceOperation,
    required_evidence: &[EvidenceRequirement],
) -> Result<LoweredPayload, EngineError> {
    require_non_empty(&files, "files")?;
    let resource_id = resource_id(resource_name)?;
    Ok((
        vec![ResourceSet {
            id: resource_id.clone(),
            selector: ResourceSelector::File {
                matchers: lower_files(files, resolution),
            },
        }],
        vec![deny_rule(
            rule_name,
            SemanticAtom::ResourceOperation {
                operation,
                target: ResourceTarget::In {
                    resource_set: resource_id,
                },
            },
            required_evidence,
        )?],
    ))
}

fn lower_information_flow(
    files: Vec<String>,
    trusted_destinations: Vec<TrustedDestination>,
) -> Result<LoweredPayload, EngineError> {
    require_non_empty(&files, "files")?;
    require_non_empty(&trusted_destinations, "trustedDestinations")?;
    let source_id = resource_id("low-sensitive-sources")?;
    let destination_id = resource_id("trusted-egress-destinations")?;
    Ok((
        vec![
            ResourceSet {
                id: source_id.clone(),
                selector: ResourceSelector::File {
                    matchers: lower_files(files, final_object_resolution()),
                },
            },
            ResourceSet {
                id: destination_id.clone(),
                selector: ResourceSelector::Endpoint {
                    matchers: trusted_destinations
                        .into_iter()
                        .map(lower_destination)
                        .collect(),
                },
            },
        ],
        vec![deny_rule(
            "deny-direct-low-sensitive-egress",
            SemanticAtom::InformationFlow {
                source: ResourceTarget::In {
                    resource_set: source_id,
                },
                destination: ResourceTarget::Except {
                    resource_set: destination_id,
                },
                propagation: FlowPropagation::Direct,
            },
            &[
                EvidenceRequirement::OperationDenied,
                EvidenceRequirement::EffectReceipt,
            ],
        )?],
    ))
}

fn final_object_resolution() -> FileResolution {
    FileResolution::FinalObject {
        follow_final_symlink: true,
        match_hardlink_identity: true,
    }
}

fn lower_files(files: Vec<String>, resolution: FileResolution) -> Vec<FileMatcher> {
    files
        .into_iter()
        .map(|path| FileMatcher {
            path: if path.contains(['*', '?']) {
                PathMatcher::Glob { pattern: path }
            } else {
                PathMatcher::Exact { path }
            },
            resolution,
        })
        .collect()
}

fn lower_destination(destination: TrustedDestination) -> EndpointMatcher {
    match destination {
        TrustedDestination::Host { pattern, ports } => EndpointMatcher::Host { pattern, ports },
        TrustedDestination::Cidr { cidr, ports } => EndpointMatcher::Cidr { cidr, ports },
    }
}

fn require_non_empty<T>(values: &[T], path: &str) -> Result<(), EngineError> {
    if values.is_empty() {
        return Err(EngineError::InvalidTemplate {
            path: path.to_owned(),
            message: "must not be empty".to_owned(),
        });
    }
    Ok(())
}

fn resource_id(value: &str) -> Result<ResourceSetId, EngineError> {
    ResourceSetId::new(value).map_err(EngineError::Identifier)
}

fn deny_rule(
    id: &str,
    atom: SemanticAtom,
    required_evidence: &[EvidenceRequirement],
) -> Result<RuleIr, EngineError> {
    Ok(RuleIr {
        id: RuleId::new(id).map_err(EngineError::Identifier)?,
        when: Expression::Atom { atom },
        outcome: RuleOutcome {
            decision: RestrictiveDecision::Deny,
            obligations: vec![Obligation::Audit, Obligation::EmitReceipt],
            remediation: SubjectRemediation::None,
        },
        enforcement: RuleEnforcement {
            decision_timing: DecisionTiming::PreEffect,
            required_evidence: required_evidence.to_vec(),
        },
    })
}
