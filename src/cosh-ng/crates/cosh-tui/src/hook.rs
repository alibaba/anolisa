use std::collections::{HashMap, HashSet};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{HookDefinition, HooksConfig};

// ─── Hook Event Names ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEventName {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    SessionStart,
    Stop,
}

impl HookEventName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::Stop => "Stop",
        }
    }
}

// ─── Hook IO Protocol ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: String,
    pub hook_event_name: String,
    pub timestamp: String,
    #[serde(flatten)]
    pub event_data: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HookOutput {
    pub decision: Option<String>,
    pub reason: Option<String>,
    pub system_message: Option<String>,
    pub hook_specific_output: Option<Value>,
}

// ─── Aggregated Results ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum HookDecision {
    Allow,
    Block(String),
    Ask,
    Passthrough,
}

/// Notification message produced by a hook for display to the user.
#[derive(Debug, Clone)]
pub struct HookNotification {
    pub hook_name: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct PreToolUseResult {
    pub decision: HookDecision,
    pub tool_input_patch: Option<Value>,
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct PostToolUseResult {
    pub deny: bool,
    pub deny_reason: Option<String>,
    pub additional_context: Option<String>,
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct UserPromptResult {
    pub decision: HookDecision,
    pub additional_context: Option<String>,
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct SessionStartResult {
    pub additional_context: Option<String>,
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct StopResult {
    pub reject: bool,
    pub reject_reason: Option<String>,
    pub notifications: Vec<HookNotification>,
}

// ─── HookSystem ──────────────────────────────────────────────────────

pub struct HookSystem {
    enabled: bool,
    disabled: HashSet<String>,
    hooks: HashMap<HookEventName, Vec<HookDefinition>>,
}

impl HookSystem {
    pub fn from_config(config: &HooksConfig) -> Self {
        let enabled = config.enabled;
        let disabled: HashSet<String> = config.disabled.iter().cloned().collect();

        let mut hooks: HashMap<HookEventName, Vec<HookDefinition>> = HashMap::new();
        hooks.insert(HookEventName::PreToolUse, config.pre_tool_use.clone());
        hooks.insert(HookEventName::PostToolUse, config.post_tool_use.clone());
        hooks.insert(HookEventName::UserPromptSubmit, config.user_prompt_submit.clone());
        hooks.insert(HookEventName::SessionStart, config.session_start.clone());
        hooks.insert(HookEventName::Stop, config.stop.clone());

        Self { enabled, disabled, hooks }
    }

    pub fn new_disabled() -> Self {
        Self {
            enabled: false,
            disabled: HashSet::new(),
            hooks: HashMap::new(),
        }
    }

    /// Dynamically append hook definitions from extensions.
    /// Extension hooks are appended to the end of each event's hook list.
    ///
    /// If extensions provide non-empty hooks, the hook system is automatically
    /// enabled (extensions are explicitly installed by the user, implying intent
    /// to use their hooks). The user can still force-disable via config if needed.
    ///
    /// The extension format uses nested `HookGroup` structures (matching
    /// copilot-shell's format). Groups are flattened into individual
    /// `HookDefinition` entries with group-level matcher/sequential inherited.
    pub fn register_extension_hooks(&mut self, hooks: &crate::extension::ExtensionHooks) {
        use crate::extension::config::flatten_hook_groups;

        if hooks.is_empty() {
            return;
        }

        // Auto-enable: extensions are user-installed, so their hooks should fire.
        self.enabled = true;

        self.hooks
            .entry(HookEventName::PreToolUse)
            .or_default()
            .extend(flatten_hook_groups(&hooks.pre_tool_use));
        self.hooks
            .entry(HookEventName::PostToolUse)
            .or_default()
            .extend(flatten_hook_groups(&hooks.post_tool_use));
        self.hooks
            .entry(HookEventName::UserPromptSubmit)
            .or_default()
            .extend(flatten_hook_groups(&hooks.user_prompt_submit));
        self.hooks
            .entry(HookEventName::SessionStart)
            .or_default()
            .extend(flatten_hook_groups(&hooks.session_start));
        self.hooks
            .entry(HookEventName::Stop)
            .or_default()
            .extend(flatten_hook_groups(&hooks.stop));

        let unsupported: &[(&str, &[_])] = &[
            ("PostToolUseFailure", &hooks.post_tool_use_failure),
            ("BeforeModel", &hooks.before_model),
            ("AfterModel", &hooks.after_model),
        ];
        for (name, groups) in unsupported {
            if !groups.is_empty() {
                eprintln!("[cosh-tui] Warning: extension hook event '{name}' is not yet supported and will be ignored");
            }
        }
    }

    fn active_hooks(&self, event: HookEventName) -> Vec<&HookDefinition> {
        self.hooks
            .get(&event)
            .map(|defs| {
                defs.iter()
                    .filter(|d| {
                        if let Some(ref name) = d.name {
                            !self.disabled.contains(name)
                        } else {
                            true
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn matches_tool(def: &HookDefinition, tool_name: &str) -> bool {
        match &def.matcher {
            None => true,
            Some(pattern) => {
                if let Ok(re) = Regex::new(pattern) {
                    re.is_match(tool_name)
                } else {
                    pattern == tool_name
                }
            }
        }
    }

    fn is_sequential(defs: &[&HookDefinition]) -> bool {
        defs.iter().any(|d| d.sequential.unwrap_or(false))
    }

    fn timeout_for(def: &HookDefinition) -> Duration {
        Duration::from_millis(def.timeout.unwrap_or(60_000))
    }

    fn hook_name(def: &HookDefinition, index: usize) -> String {
        def.name.clone().unwrap_or_else(|| format!("hook-{index}"))
    }

    // ─── Fire methods ────────────────────────────────────────────────

    pub async fn fire_pre_tool_use(
        &self,
        session_id: &str,
        cwd: &str,
        tool_name: &str,
        tool_input: &Value,
    ) -> PreToolUseResult {
        if !self.enabled {
            return PreToolUseResult {
                decision: HookDecision::Passthrough,
                tool_input_patch: None,
                notifications: vec![],
            };
        }

        let defs: Vec<&HookDefinition> = self
            .active_hooks(HookEventName::PreToolUse)
            .into_iter()
            .filter(|d| Self::matches_tool(d, tool_name))
            .collect();

        if defs.is_empty() {
            return PreToolUseResult {
                decision: HookDecision::Passthrough,
                tool_input_patch: None,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({
            "tool_name": tool_name,
            "tool_input": tool_input,
        });
        let input = self.build_input(session_id, cwd, HookEventName::PreToolUse, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_pre_tool_use(outputs, &defs)
    }

    pub async fn fire_post_tool_use(
        &self,
        session_id: &str,
        cwd: &str,
        tool_name: &str,
        tool_input: &Value,
        tool_response: &str,
    ) -> PostToolUseResult {
        if !self.enabled {
            return PostToolUseResult {
                deny: false,
                deny_reason: None,
                additional_context: None,
                notifications: vec![],
            };
        }

        let defs: Vec<&HookDefinition> = self
            .active_hooks(HookEventName::PostToolUse)
            .into_iter()
            .filter(|d| Self::matches_tool(d, tool_name))
            .collect();

        if defs.is_empty() {
            return PostToolUseResult {
                deny: false,
                deny_reason: None,
                additional_context: None,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({
            "tool_name": tool_name,
            "tool_input": tool_input,
            "tool_response": tool_response,
        });
        let input = self.build_input(session_id, cwd, HookEventName::PostToolUse, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_post_tool_use(outputs, &defs)
    }

    pub async fn fire_user_prompt_submit(
        &self,
        session_id: &str,
        cwd: &str,
        prompt: &str,
    ) -> UserPromptResult {
        if !self.enabled {
            return UserPromptResult {
                decision: HookDecision::Passthrough,
                additional_context: None,
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::UserPromptSubmit);
        if defs.is_empty() {
            return UserPromptResult {
                decision: HookDecision::Passthrough,
                additional_context: None,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({ "prompt": prompt });
        let input = self.build_input(session_id, cwd, HookEventName::UserPromptSubmit, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_user_prompt(outputs, &defs)
    }

    pub async fn fire_session_start(
        &self,
        session_id: &str,
        cwd: &str,
    ) -> SessionStartResult {
        if !self.enabled {
            return SessionStartResult {
                additional_context: None,
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::SessionStart);
        if defs.is_empty() {
            return SessionStartResult {
                additional_context: None,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({ "source": "startup" });
        let input = self.build_input(session_id, cwd, HookEventName::SessionStart, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_session_start(outputs, &defs)
    }

    pub async fn fire_stop(
        &self,
        session_id: &str,
        cwd: &str,
        last_message: &str,
    ) -> StopResult {
        if !self.enabled {
            return StopResult {
                reject: false,
                reject_reason: None,
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::Stop);
        if defs.is_empty() {
            return StopResult {
                reject: false,
                reject_reason: None,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({ "last_assistant_message": last_message });
        let input = self.build_input(session_id, cwd, HookEventName::Stop, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_stop(outputs, &defs)
    }

    // ─── Internal helpers ────────────────────────────────────────────

    fn build_input(
        &self,
        session_id: &str,
        cwd: &str,
        event: HookEventName,
        event_data: Value,
    ) -> HookInput {
        HookInput {
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            hook_event_name: event.as_str().to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            event_data,
        }
    }

    async fn run_hooks(
        &self,
        defs: &[&HookDefinition],
        input: &HookInput,
    ) -> Vec<(usize, HookOutput)> {
        let input_json = serde_json::to_string(input).unwrap_or_default();

        if Self::is_sequential(defs) {
            let mut results = Vec::new();
            for (i, def) in defs.iter().enumerate() {
                let output = Self::run_single_hook(def, &input_json).await;
                results.push((i, output));
            }
            results
        } else {
            let futs: Vec<_> = defs
                .iter()
                .enumerate()
                .map(|(i, def)| {
                    let json = input_json.clone();
                    let cmd = def.command.clone();
                    let timeout = Self::timeout_for(def);
                    async move {
                        let output = Self::run_hook_cmd(&cmd, &json, timeout).await;
                        (i, output)
                    }
                })
                .collect();
            futures::future::join_all(futs).await
        }
    }

    async fn run_single_hook(def: &HookDefinition, input_json: &str) -> HookOutput {
        Self::run_hook_cmd(&def.command, input_json, Self::timeout_for(def)).await
    }

    async fn run_hook_cmd(command: &str, input_json: &str, timeout: Duration) -> HookOutput {
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;

        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[cosh-hook] Failed to spawn hook '{command}': {e}");
                return HookOutput::default();
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input_json.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        let output = match result {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                eprintln!("[cosh-hook] Hook '{command}' execution failed: {e}");
                return HookOutput::default();
            }
            Err(_) => {
                eprintln!("[cosh-hook] Hook '{command}' timed out");
                return HookOutput::default();
            }
        };

        let exit_code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        match exit_code {
            0 => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let trimmed = stdout.trim();
                if trimmed.is_empty() {
                    return HookOutput::default();
                }
                serde_json::from_str::<HookOutput>(trimmed).unwrap_or_else(|e| {
                    eprintln!("[cosh-hook] Failed to parse output from '{command}': {e}");
                    HookOutput::default()
                })
            }
            2 => {
                // System block via exit code 2
                HookOutput {
                    decision: Some("block".to_string()),
                    reason: Some(if stderr.is_empty() {
                        "Blocked by hook".to_string()
                    } else {
                        stderr.trim().to_string()
                    }),
                    system_message: None,
                    hook_specific_output: None,
                }
            }
            _ => {
                // Non-zero (not 2) = warning, do not block
                if !stderr.is_empty() {
                    eprintln!("[cosh-hook] Hook '{command}' warning: {}", stderr.trim());
                }
                HookOutput::default()
            }
        }
    }

    // ─── Aggregation ─────────────────────────────────────────────────

    fn aggregate_pre_tool_use(
        &self,
        outputs: Vec<(usize, HookOutput)>,
        defs: &[&HookDefinition],
    ) -> PreToolUseResult {
        let mut decision = HookDecision::Passthrough;
        let mut tool_input_patch: Option<Value> = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            match out.decision.as_deref() {
                Some("block") | Some("deny") => {
                    let reason = out.reason.unwrap_or_else(|| "Blocked by hook".to_string());
                    decision = HookDecision::Block(reason);
                }
                Some("ask") => {
                    if decision == HookDecision::Passthrough || decision == HookDecision::Allow {
                        decision = HookDecision::Ask;
                    }
                }
                Some("approve") | Some("allow") => {
                    if decision == HookDecision::Passthrough {
                        decision = HookDecision::Allow;
                    }
                }
                _ => {}
            }

            if let Some(ref specific) = out.hook_specific_output {
                if let Some(patch) = specific.get("tool_input") {
                    tool_input_patch = Some(match tool_input_patch {
                        Some(existing) => merge_json(existing, patch.clone()),
                        None => patch.clone(),
                    });
                }
            }
        }

        PreToolUseResult {
            decision,
            tool_input_patch,
            notifications,
        }
    }

    fn aggregate_post_tool_use(
        &self,
        outputs: Vec<(usize, HookOutput)>,
        defs: &[&HookDefinition],
    ) -> PostToolUseResult {
        let mut deny = false;
        let mut deny_reason = None;
        let mut additional_context: Option<String> = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            if matches!(out.decision.as_deref(), Some("deny") | Some("block")) {
                deny = true;
                deny_reason = out.reason.or(deny_reason);
            }

            if let Some(ref specific) = out.hook_specific_output {
                if let Some(ctx) = specific.get("additional_context").and_then(|v| v.as_str()) {
                    additional_context = Some(match additional_context {
                        Some(existing) => format!("{existing}\n{ctx}"),
                        None => ctx.to_string(),
                    });
                }
            }
        }

        PostToolUseResult {
            deny,
            deny_reason,
            additional_context,
            notifications,
        }
    }

    fn aggregate_user_prompt(
        &self,
        outputs: Vec<(usize, HookOutput)>,
        defs: &[&HookDefinition],
    ) -> UserPromptResult {
        let mut decision = HookDecision::Passthrough;
        let mut additional_context: Option<String> = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            match out.decision.as_deref() {
                Some("block") | Some("deny") => {
                    let reason = out.reason.unwrap_or_else(|| "Blocked by hook".to_string());
                    decision = HookDecision::Block(reason);
                }
                _ => {}
            }

            if let Some(ref specific) = out.hook_specific_output {
                if let Some(ctx) = specific.get("additional_context").and_then(|v| v.as_str()) {
                    additional_context = Some(match additional_context {
                        Some(existing) => format!("{existing}\n{ctx}"),
                        None => ctx.to_string(),
                    });
                }
            }
        }

        UserPromptResult {
            decision,
            additional_context,
            notifications,
        }
    }

    fn aggregate_session_start(
        &self,
        outputs: Vec<(usize, HookOutput)>,
        defs: &[&HookDefinition],
    ) -> SessionStartResult {
        let mut additional_context: Option<String> = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            if let Some(ref specific) = out.hook_specific_output {
                if let Some(ctx) = specific.get("additional_context").and_then(|v| v.as_str()) {
                    additional_context = Some(match additional_context {
                        Some(existing) => format!("{existing}\n{ctx}"),
                        None => ctx.to_string(),
                    });
                }
            }
        }

        SessionStartResult {
            additional_context,
            notifications,
        }
    }

    fn aggregate_stop(
        &self,
        outputs: Vec<(usize, HookOutput)>,
        defs: &[&HookDefinition],
    ) -> StopResult {
        let mut reject = false;
        let mut reject_reason = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            if matches!(out.decision.as_deref(), Some("deny") | Some("block") | Some("reject")) {
                reject = true;
                reject_reason = out.reason.or(reject_reason);
            }
        }

        StopResult {
            reject,
            reject_reason,
            notifications,
        }
    }

    fn collect_notifications(
        &self,
        output: &HookOutput,
        hook_name: &str,
        notifications: &mut Vec<HookNotification>,
    ) {
        if let Some(ref msg) = output.system_message {
            notifications.push(HookNotification {
                hook_name: hook_name.to_string(),
                message: msg.clone(),
            });
        }
        // Also show reason as notification for block/deny
        if matches!(output.decision.as_deref(), Some("block") | Some("deny")) {
            if let Some(ref reason) = output.reason {
                notifications.push(HookNotification {
                    hook_name: hook_name.to_string(),
                    message: format!("Blocked: {reason}"),
                });
            }
        }
    }
}

/// Deep-merge two JSON values (b overwrites a for conflicting keys).
fn merge_json(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Object(mut map_a), Value::Object(map_b)) => {
            for (k, v) in map_b {
                let merged = if let Some(existing) = map_a.remove(&k) {
                    merge_json(existing, v)
                } else {
                    v
                };
                map_a.insert(k, merged);
            }
            Value::Object(map_a)
        }
        (_, b) => b,
    }
}

/// Public re-export of merge_json for use in core.rs.
pub fn merge_json_pub(a: Value, b: Value) -> Value {
    merge_json(a, b)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_str() {
        assert_eq!(HookEventName::PreToolUse.as_str(), "PreToolUse");
        assert_eq!(HookEventName::PostToolUse.as_str(), "PostToolUse");
        assert_eq!(HookEventName::UserPromptSubmit.as_str(), "UserPromptSubmit");
        assert_eq!(HookEventName::SessionStart.as_str(), "SessionStart");
        assert_eq!(HookEventName::Stop.as_str(), "Stop");
    }

    #[test]
    fn parse_hook_output_block() {
        let json = r#"{"decision":"block","reason":"unsafe command"}"#;
        let out: HookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.decision.as_deref(), Some("block"));
        assert_eq!(out.reason.as_deref(), Some("unsafe command"));
    }

    #[test]
    fn parse_hook_output_allow_with_patch() {
        let json = r#"{"decision":"allow","hook_specific_output":{"tool_input":{"safe_mode":true}}}"#;
        let out: HookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.decision.as_deref(), Some("allow"));
        let patch = out.hook_specific_output.unwrap();
        assert_eq!(patch["tool_input"]["safe_mode"], true);
    }

    #[test]
    fn merge_json_deep() {
        let a = serde_json::json!({"a": 1, "b": {"x": 10}});
        let b = serde_json::json!({"b": {"y": 20}, "c": 3});
        let merged = merge_json(a, b);
        assert_eq!(merged["a"], 1);
        assert_eq!(merged["b"]["x"], 10);
        assert_eq!(merged["b"]["y"], 20);
        assert_eq!(merged["c"], 3);
    }

    #[test]
    fn disabled_system_returns_passthrough() {
        let sys = HookSystem::new_disabled();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(sys.fire_pre_tool_use("s1", "/tmp", "shell", &Value::Null));
        assert_eq!(result.decision, HookDecision::Passthrough);
    }

    #[test]
    fn matcher_regex_works() {
        let def = HookDefinition {
            command: "echo".to_string(),
            name: None,
            matcher: Some("run_shell.*".to_string()),
            timeout: None,
            sequential: None,
        };
        assert!(HookSystem::matches_tool(&def, "run_shell_command"));
        assert!(!HookSystem::matches_tool(&def, "read_file"));
    }

    #[test]
    fn matcher_none_matches_all() {
        let def = HookDefinition {
            command: "echo".to_string(),
            name: None,
            matcher: None,
            timeout: None,
            sequential: None,
        };
        assert!(HookSystem::matches_tool(&def, "any_tool"));
    }

    #[tokio::test]
    async fn fire_pre_tool_use_with_blocking_hook() {
        let config = HooksConfig {
            enabled: true,
            disabled: vec![],
            pre_tool_use: vec![HookDefinition {
                command: "echo '{\"decision\":\"block\",\"reason\":\"no rm allowed\"}'".to_string(),
                name: Some("block-rm".to_string()),
                matcher: Some("run_shell_command".to_string()),
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use("s1", "/tmp", "run_shell_command", &serde_json::json!({"command": "rm -rf /"}))
            .await;
        assert_eq!(result.decision, HookDecision::Block("no rm allowed".to_string()));
        assert!(!result.notifications.is_empty());
    }

    #[tokio::test]
    async fn fire_pre_tool_use_no_match() {
        let config = HooksConfig {
            enabled: true,
            disabled: vec![],
            pre_tool_use: vec![HookDefinition {
                command: "echo '{\"decision\":\"block\",\"reason\":\"no\"}'".to_string(),
                name: None,
                matcher: Some("run_shell_command".to_string()),
                timeout: None,
                sequential: None,
            }],
            post_tool_use: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use("s1", "/tmp", "read_file", &serde_json::json!({}))
            .await;
        assert_eq!(result.decision, HookDecision::Passthrough);
    }

    #[tokio::test]
    async fn exit_code_2_means_block() {
        let config = HooksConfig {
            enabled: true,
            disabled: vec![],
            pre_tool_use: vec![HookDefinition {
                command: "sh -c 'echo blocked >&2; exit 2'".to_string(),
                name: Some("exit2-hook".to_string()),
                matcher: None,
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use("s1", "/tmp", "any", &serde_json::json!({}))
            .await;
        assert_eq!(result.decision, HookDecision::Block("blocked".to_string()));
    }
}
