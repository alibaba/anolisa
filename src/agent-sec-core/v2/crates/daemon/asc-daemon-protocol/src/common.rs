use serde::{Deserialize, Serialize};

/// Generic identity plus revision query/delete params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionParams {
    /// Resource identity.
    pub id: String,
    /// Immutable revision.
    pub revision: u64,
}

/// Generic identity params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdParams {
    /// Resource identity.
    pub id: String,
}

/// Bounded pagination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListParams {
    /// Page size, 1..=1000.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Page offset.
    #[serde(default)]
    pub offset: u64,
}

const fn default_limit() -> u32 {
    100
}
