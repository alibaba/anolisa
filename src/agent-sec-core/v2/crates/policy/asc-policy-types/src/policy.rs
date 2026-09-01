//! Immutable Canonical Policy IR envelope.

use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};
use crate::identifiers::{Digest, PolicyId, ProfileId, Revision};
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
