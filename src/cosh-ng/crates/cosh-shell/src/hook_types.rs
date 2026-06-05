use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTrigger {
    OnComplete,
    OnSuccess,
    OnFail,
}

#[derive(Debug, Clone)]
pub struct HookMatcher {
    pub id: String,
    pub commands: Vec<String>,
    pub command_patterns: Vec<String>,
    pub exit_codes: Option<Vec<i32>>,
    pub trigger: HookTrigger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInput {
    pub command: String,
    pub cwd: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output_ref: Option<String>,
    pub output_bytes: u64,
    pub output_preview: String,
}

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
}
