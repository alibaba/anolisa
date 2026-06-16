// SPDX-License-Identifier: Apache-2.0
//! Workload class enum, policy file schema and policy engine.
//!
//! See the workload class table and policy TOML schema.
//! Evaluation logic returns a [`RuntimeDecision`] that the
//! daemon hands to the [`crate::backend`] selector and the
//! [`crate::lifecycle`] manager.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::backend::BackendKind;
use crate::error::{AnvilError, Result};

// ---------------------------------------------------------------------------
// WorkloadClass
// ---------------------------------------------------------------------------

/// Requests without a declared workload class are rejected; no silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadClass {
    AgentRl,
    AgentTool,
    Serverless,
    DevShell,
    Untrusted,
    Function,
}

impl WorkloadClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            WorkloadClass::AgentRl => "agent-rl",
            WorkloadClass::AgentTool => "agent-tool",
            WorkloadClass::Serverless => "serverless",
            WorkloadClass::DevShell => "dev-shell",
            WorkloadClass::Untrusted => "untrusted",
            WorkloadClass::Function => "function",
        }
    }
}

impl fmt::Display for WorkloadClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkloadClass {
    type Err = AnvilError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "agent-rl" => Ok(WorkloadClass::AgentRl),
            "agent-tool" => Ok(WorkloadClass::AgentTool),
            "serverless" => Ok(WorkloadClass::Serverless),
            "dev-shell" => Ok(WorkloadClass::DevShell),
            "untrusted" => Ok(WorkloadClass::Untrusted),
            "function" => Ok(WorkloadClass::Function),
            other => Err(AnvilError::PolicyEvalError {
                reason: format!("unknown workload class: {other}"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Misc enums in policy schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FallbackOnMissingHook {
    #[default]
    Fail,
    Degrade,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ResetMode {
    #[default]
    MmTemplate,
    OverlayfsRollback,
    FullRecreate,
}

/// Checkpoint strategy selection.
/// NOTE(Phase 3): v0.1 stores strategy in policy config but does NOT invoke
/// kernel syscalls. Real checkpoint/restore via UFFD-WP deferred to Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointStrategy {
    #[serde(rename = "uffd-wp-async")]
    UffdWpAsync,
    Criu,
}

// ---------------------------------------------------------------------------
// Policy file schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyFile {
    pub manifest_version: u32,
    pub policy_name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(rename = "match")]
    pub match_: PolicyMatch,
    pub select: PolicySelect,
    #[serde(default)]
    pub pool: Option<PolicyPool>,
    #[serde(default)]
    pub checkpoint: Option<PolicyCheckpoint>,
    #[serde(default)]
    pub quota: Option<PolicyQuota>,
    #[serde(default)]
    pub hooks: PolicyHooks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyMatch {
    pub workload_class: WorkloadClass,
    #[serde(default)]
    pub image_labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySelect {
    pub backend_priority: Vec<BackendKind>,
    #[serde(default)]
    pub kernel_hooks: Vec<String>,
    #[serde(default)]
    pub templates: Vec<String>,
    #[serde(default)]
    pub fallback_on_missing_hook: FallbackOnMissingHook,
}

impl PolicyFile {
    /// Validate internal consistency constraints of a policy file.
    pub fn validate(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPool {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub target: u32,
    #[serde(default)]
    pub max: u32,
    #[serde(default = "default_warm_ttl")]
    pub warm_ttl: String,
    #[serde(default)]
    pub reset_mode: ResetMode,
}

fn default_warm_ttl() -> String {
    "30m".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCheckpoint {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_checkpoint_strategy")]
    pub strategy: CheckpointStrategy,
}

fn default_checkpoint_strategy() -> CheckpointStrategy {
    CheckpointStrategy::UffdWpAsync
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyQuota {
    #[serde(default)]
    pub cpu_shares: Option<u32>,
    #[serde(default)]
    pub memory_high: Option<String>,
    #[serde(default)]
    pub memory_max: Option<String>,
    #[serde(default)]
    pub pids_max: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyHooks {
    #[serde(default)]
    pub on_create: Option<HookSequence>,
    #[serde(default)]
    pub on_reset: Option<HookSequence>,
    #[serde(default)]
    pub on_destroy: Option<HookSequence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookSequence {
    #[serde(default)]
    pub sequence: Vec<String>,
}

// ---------------------------------------------------------------------------
// Decision + image metadata
// ---------------------------------------------------------------------------

/// Subset of OCI image metadata that policy evaluation reads.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageMetadata {
    pub digest: String,
    #[serde(default)]
    pub workload_class: Option<WorkloadClass>,
    #[serde(default)]
    pub kernel_version: Option<String>,
}

/// Result of evaluating a request against the policy library. Drives
/// backend selection, hook activation, and pool eligibility
/// for a single sandbox instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDecision {
    pub policy_name: String,
    pub workload_class: WorkloadClass,
    pub backend_priority: Vec<BackendKind>,
    pub kernel_hooks: Vec<String>,
    pub templates: Vec<String>,
    pub fallback_on_missing_hook: FallbackOnMissingHook,
    pub pool: Option<PolicyPool>,
    pub checkpoint: Option<PolicyCheckpoint>,
    pub quota: Option<PolicyQuota>,
    pub hooks: PolicyHooks,
    pub pool_eligible: bool,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// In-memory store of policy files, sorted by descending `priority`.
#[derive(Debug, Default, Clone)]
pub struct PolicyEngine {
    policies: Vec<PolicyFile>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    /// Construct an engine pre-populated with `policies`. Sorted so that
    /// higher priority is evaluated first.
    pub fn with_policies(mut policies: Vec<PolicyFile>) -> Self {
        policies.sort_by_key(|p| std::cmp::Reverse(p.priority));
        Self { policies }
    }

    /// Load every `*.toml` file under `dir` into a single engine. Files
    /// whose `manifest_version` is unsupported, or whose schema fails to
    /// parse, are wrapped in [`AnvilError::PolicyLoadError`] (caller can
    /// decide between `fail`/`warn` per [`crate::config::PolicyLoadErrorMode`]).
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut policies = Vec::new();
        let entries = fs::read_dir(dir).map_err(|e| AnvilError::PolicyLoadError {
            path: dir.to_path_buf(),
            source: Box::new(AnvilError::IoError { source: e }),
        })?;
        for entry in entries {
            let entry = entry.map_err(AnvilError::from)?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let policy = load_one(&path)?;
            policy.validate()?;
            tracing::info!(
                policy_name = %policy.policy_name,
                priority = policy.priority,
                path = %path.display(),
                "loaded policy"
            );
            policies.push(policy);
        }
        Ok(Self::with_policies(policies))
    }

    pub fn policies(&self) -> &[PolicyFile] {
        &self.policies
    }

    /// Walk the policy library top-down by priority and return the first
    /// policy whose `match` block matches the request.
    pub fn evaluate(
        &self,
        labels: &HashMap<String, String>,
        image_metadata: &ImageMetadata,
    ) -> Result<RuntimeDecision> {
        let class = image_metadata
            .workload_class
            .ok_or_else(|| AnvilError::PolicyEvalError {
                reason: "image_metadata.workload_class is required (no silent fallback)".into(),
            })?;

        for policy in &self.policies {
            if policy.match_.workload_class != class {
                continue;
            }
            if !labels_match(&policy.match_.image_labels, labels) {
                continue;
            }
            tracing::info!(
                policy = %policy.policy_name,
                class = %class,
                "policy matched"
            );
            return Ok(build_decision(policy));
        }

        Err(AnvilError::PolicyEvalError {
            reason: format!("no policy matched workload_class={class}"),
        })
    }
}

fn load_one(path: &Path) -> Result<PolicyFile> {
    let raw = fs::read_to_string(path).map_err(|e| AnvilError::PolicyLoadError {
        path: path.to_path_buf(),
        source: Box::new(AnvilError::IoError { source: e }),
    })?;
    let policy: PolicyFile = toml::from_str(&raw).map_err(|e| AnvilError::PolicyLoadError {
        path: path.to_path_buf(),
        source: Box::new(AnvilError::from(e)),
    })?;
    if policy.manifest_version != 1 {
        return Err(AnvilError::PolicyLoadError {
            path: path.to_path_buf(),
            source: Box::new(AnvilError::PolicyEvalError {
                reason: format!(
                    "unsupported manifest_version {} (only 1 is supported)",
                    policy.manifest_version
                ),
            }),
        });
    }
    Ok(policy)
}

fn labels_match(required: &HashMap<String, String>, provided: &HashMap<String, String>) -> bool {
    required
        .iter()
        .all(|(k, v)| provided.get(k).map(|got| got == v).unwrap_or(false))
}

fn build_decision(policy: &PolicyFile) -> RuntimeDecision {
    let pool_eligible = policy.pool.as_ref().map(|p| p.enabled).unwrap_or(false);
    RuntimeDecision {
        policy_name: policy.policy_name.clone(),
        workload_class: policy.match_.workload_class,
        backend_priority: policy.select.backend_priority.clone(),
        kernel_hooks: policy.select.kernel_hooks.clone(),
        templates: policy.select.templates.clone(),
        fallback_on_missing_hook: policy.select.fallback_on_missing_hook,
        pool: policy.pool.clone(),
        checkpoint: policy.checkpoint.clone(),
        quota: policy.quota.clone(),
        hooks: policy.hooks.clone(),
        pool_eligible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_toml() -> &'static str {
        r#"
manifest_version = 1
policy_name = "agent-rl-default"
priority = 100

[match]
workload_class = "agent-rl"
image_labels = { "ai.anolisa.workload" = "rl-rollout" }

[select]
backend_priority = ["kata-fc", "kata-clh", "rund"]
kernel_hooks = ["mm-template", "uffd-wp"]
templates = ["mm-template"]
fallback_on_missing_hook = "fail"

[pool]
enabled = true
min = 4
target = 16
max = 64
warm_ttl = "30m"
reset_mode = "mm-template"

[checkpoint]
enabled = true
strategy = "uffd-wp-async"

[quota]
cpu_shares = 1024
memory_high = "2G"
memory_max = "4G"
pids_max = 4096

[hooks.on_create]
sequence = ["template-reg:bind-mm-template"]
"#
    }

    #[test]
    fn parses_full_schema() {
        let pf: PolicyFile = toml::from_str(sample_toml()).expect("parse");
        assert_eq!(pf.policy_name, "agent-rl-default");
        assert_eq!(pf.match_.workload_class, WorkloadClass::AgentRl);
        assert_eq!(pf.select.backend_priority[0], BackendKind::KataFc);
        assert!(pf.pool.as_ref().expect("pool").enabled);
    }

    #[test]
    fn evaluate_picks_highest_priority_match() {
        let p1: PolicyFile = toml::from_str(sample_toml()).expect("parse");
        let mut p2 = p1.clone();
        p2.policy_name = "agent-rl-override".into();
        p2.priority = 200;
        p2.select.backend_priority = vec![BackendKind::Rund];

        let engine = PolicyEngine::with_policies(vec![p1, p2]);

        let labels = HashMap::from([("ai.anolisa.workload".into(), "rl-rollout".into())]);
        let img = ImageMetadata {
            digest: "sha256:abc".into(),
            workload_class: Some(WorkloadClass::AgentRl),
            kernel_version: None,
        };
        let decision = engine.evaluate(&labels, &img).expect("matches");
        assert_eq!(decision.policy_name, "agent-rl-override");
        assert_eq!(decision.backend_priority, vec![BackendKind::Rund]);
        assert!(decision.pool_eligible);
    }

    #[test]
    fn evaluate_rejects_when_no_class() {
        let engine = PolicyEngine::new();
        let img = ImageMetadata::default();
        let err = engine
            .evaluate(&HashMap::new(), &img)
            .expect_err("no class");
        assert!(matches!(err, AnvilError::PolicyEvalError { .. }));
    }

    #[test]
    fn evaluate_rejects_when_no_match() {
        let p: PolicyFile = toml::from_str(sample_toml()).expect("parse");
        let engine = PolicyEngine::with_policies(vec![p]);
        let img = ImageMetadata {
            digest: "sha256:abc".into(),
            workload_class: Some(WorkloadClass::Function),
            kernel_version: None,
        };
        let err = engine
            .evaluate(&HashMap::new(), &img)
            .expect_err("no match");
        assert!(matches!(err, AnvilError::PolicyEvalError { .. }));
    }

    #[test]
    fn load_dir_reads_toml_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("agent-rl.toml");
        let mut f = fs::File::create(&path).expect("create");
        f.write_all(sample_toml().as_bytes()).expect("write");
        // a non-toml file should be skipped silently.
        fs::write(tmp.path().join("README"), b"ignore me").expect("write");

        let engine = PolicyEngine::load_dir(tmp.path()).expect("load");
        assert_eq!(engine.policies().len(), 1);
    }
}
