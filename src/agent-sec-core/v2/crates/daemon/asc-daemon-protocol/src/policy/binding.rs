use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Resource identity plus immutable revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionRefDto {
    /// Stable identity.
    pub id: Uuid,
    /// Immutable revision.
    pub revision: u64,
}

/// Binding put params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutBindingParams {
    /// Existing Binding identity to update. Omit it to create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<Uuid>,
    /// Exactly one Policy revision.
    pub policy_ref: RevisionRefDto,
    /// Exactly one Scope revision.
    pub scope_ref: RevisionRefDto,
}

/// Binding removal params. Removal is another immutable desired revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteBindingParams {
    /// Stable Binding identity.
    pub binding_id: Uuid,
}
