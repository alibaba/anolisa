use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Scope put params. Omit `scopeId` to create a new daemon-owned identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutScopeParams {
    /// Existing Scope identity to update. Omit it to create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<Uuid>,
    /// Unresolved process-selection intent. Trusted identity resolution is downstream.
    pub selector: ScopeSelectorDto,
}

/// Simple user-facing Scope selectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScopeSelectorDto {
    /// Select one current process. The PID is not a stable trusted identity.
    Pid {
        /// Positive process identifier observed by the caller.
        pid: u32,
    },
    /// Select one cgroup by caller-observed kernel cgroup id.
    CgroupId {
        /// Positive cgroup identifier; resolution remains daemon/downstream work.
        cgroup_id: u64,
    },
}
