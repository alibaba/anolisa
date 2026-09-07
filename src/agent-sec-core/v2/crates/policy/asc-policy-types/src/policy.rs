//! Immutable Canonical Policy IR envelope.

use serde::{Deserialize, Serialize};

use crate::authoring::PolicyTemplate;
use crate::error::{Validate, ValidationError};
use crate::identifiers::{Digest, PolicyId, ProfileId, ResourceId, Revision};
use crate::ir::CanonicalPolicyIr;
use crate::profile::{IR_SCHEMA_VERSION_V1, PROFILE_V1ALPHA1_DEMO1};

/// Immutable backend-independent Policy revision produced by PAP lowering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyEnvelope {
    /// Canonical IR envelope schema version.
    pub ir_schema_version: u16,
    /// Immutable semantic profile implemented by the payload.
    pub profile_id: ProfileId,
    /// Stable policy identity.
    pub policy_id: PolicyId,
    /// Immutable policy revision.
    pub revision: Revision,
    /// Optional digest over the canonical payload representation.
    ///
    /// Phase one deliberately permits this field to be absent until a shared
    /// JSON canonicalization algorithm is frozen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<Digest>,
    /// Backend-independent security semantics.
    pub payload: CanonicalPolicyIr,
}

impl Validate for PolicyEnvelope {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.ir_schema_version != IR_SCHEMA_VERSION_V1 {
            return Err(ValidationError::new(
                "irSchemaVersion",
                format!("unsupported IR schema version {}", self.ir_schema_version),
            ));
        }
        if self.profile_id.as_str() != PROFILE_V1ALPHA1_DEMO1 {
            return Err(ValidationError::new(
                "profileId",
                "unsupported Canonical Policy IR profile",
            ));
        }
        self.payload
            .validate()
            .map_err(|error| ValidationError::new(format!("payload.{}", error.path), error.message))
    }
}

/// Durable Policy revision with its authored and deterministic lowered forms.
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

impl Validate for PreparedPolicy {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.policy_name.trim().is_empty()
            || self.policy_name.len() > 256
            || self.policy_name.chars().any(char::is_control)
        {
            return Err(ValidationError::new(
                "policyName",
                "must contain a visible, control-free value of at most 256 bytes",
            ));
        }
        if self.policy_id.as_str() != self.canonical_policy.policy_id.as_str() {
            return Err(ValidationError::new(
                "canonicalPolicy.policyId",
                "must match the prepared Policy identity",
            ));
        }
        if self.revision != self.canonical_policy.revision {
            return Err(ValidationError::new(
                "canonicalPolicy.revision",
                "must match the prepared Policy revision",
            ));
        }
        Digest::new(&self.template_digest).map_err(|message| {
            ValidationError::new(
                "templateDigest",
                format!("invalid template digest: {message}"),
            )
        })?;
        self.canonical_policy.validate().map_err(|error| {
            ValidationError::new(format!("canonicalPolicy.{}", error.path), error.message)
        })
    }
}
