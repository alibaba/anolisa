use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookFinding {
    pub hook_id: String,
    pub severity: FindingSeverity,
    pub title: String,
    pub description: String,
    pub suggestion: String,
    pub skill: Option<String>,
    pub cli_hint: Option<String>,
    #[serde(default)]
    pub context_refs: Vec<String>,
}
