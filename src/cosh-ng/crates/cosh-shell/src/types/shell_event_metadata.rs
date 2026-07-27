use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRoutingMetadata {
    pub generation: u64,
    pub top_level_missing: bool,
    pub proven: bool,
    pub sensitive: bool,
    pub unsafe_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCaptureMetadata {
    pub kind: Option<String>,
    pub target_id: Option<String>,
    pub generation: u64,
    pub lifecycle: ShellCaptureLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellCaptureLifecycle {
    Submitted,
    Drained,
    Expired,
    Overflow,
}
