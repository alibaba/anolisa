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
    PostToolUseFailure,
    UserPromptSubmit,
    SessionStart,
    Stop,
    BeforeModel,
    AfterModel,
}

impl HookEventName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::Stop => "Stop",
            Self::BeforeModel => "BeforeModel",
            Self::AfterModel => "AfterModel",
        }
    }
}

// ─── Hook IO Protocol ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct HookInput {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub cwd: String,
    pub hook_event_name: String,
    pub timestamp: String,
    /// copilot-shell 协议必填字段。cosh-ng 不维护会话 transcript
    /// 文件，用 cwd 派生路径占位，仅保证协议合规。
    pub transcript_path: String,
    #[serde(flatten)]
    pub event_data: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HookOutput {
    pub decision: Option<String>,
    pub reason: Option<String>,
    #[serde(alias = "systemMessage")]
    pub system_message: Option<String>,
    #[serde(alias = "hookSpecificOutput")]
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
    /// The individual hook's decision (e.g. "allow", "ask", "block", "deny").
    /// Carried through the protocol so cosh-shell can color-code per-hook notices.
    pub decision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreToolUseResult {
    pub decision: HookDecision,
    pub tool_input_patch: Option<Value>,
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct PostToolUseResult {
    pub decision: HookDecision,
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
    pub decision: HookDecision,
    pub notifications: Vec<HookNotification>,
}

/// Sandbox bypass request extracted from hook output.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxBypassRequest {
    pub original_command: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PostToolUseFailureResult {
    pub notifications: Vec<HookNotification>,
    /// If present, a hook is requesting sandbox bypass approval.
    pub sandbox_bypass_request: Option<SandboxBypassRequest>,
}

#[derive(Debug, Clone)]
pub struct BeforeModelResult {
    pub notifications: Vec<HookNotification>,
}

#[derive(Debug, Clone)]
pub struct AfterModelResult {
    pub notifications: Vec<HookNotification>,
}

// ─── HookSystem ──────────────────────────────────────────────────────

pub struct HookSystem {
    enabled: bool,
    disabled: HashSet<String>,
    hooks: HashMap<HookEventName, Vec<HookDefinition>>,
    /// Current run_id, set at the start of each agent run.
    run_id: Option<String>,
}

impl HookSystem {
    pub fn from_config(config: &HooksConfig) -> Self {
        let enabled = config.enabled;

        // Read disabled hooks from states/hooks.json
        let disabled = crate::state::load_disabled(crate::state::HOOKS_STATE);

        // Filter out hooks without name (enforced for all sources).
        let filter_named = |defs: &[HookDefinition]| -> Vec<HookDefinition> {
            defs.iter()
                .filter(|d| {
                    if d.name.is_none() {
                        let command = crate::redaction::redact_text(&d.command);
                        tracing::warn!(target: "cosh_hook", "Skipping config hook without name: {command}");
                        false
                    } else {
                        true
                    }
                })
                .cloned()
                .collect()
        };

        let mut hooks: HashMap<HookEventName, Vec<HookDefinition>> = HashMap::new();
        hooks.insert(
            HookEventName::PreToolUse,
            filter_named(&config.pre_tool_use),
        );
        hooks.insert(
            HookEventName::PostToolUse,
            filter_named(&config.post_tool_use),
        );
        hooks.insert(
            HookEventName::PostToolUseFailure,
            filter_named(&config.post_tool_use_failure),
        );
        hooks.insert(
            HookEventName::UserPromptSubmit,
            filter_named(&config.user_prompt_submit),
        );
        hooks.insert(
            HookEventName::SessionStart,
            filter_named(&config.session_start),
        );
        hooks.insert(HookEventName::Stop, filter_named(&config.stop));
        hooks.insert(
            HookEventName::BeforeModel,
            filter_named(&config.before_model),
        );
        hooks.insert(HookEventName::AfterModel, filter_named(&config.after_model));

        Self {
            enabled,
            disabled,
            hooks,
            run_id: None,
        }
    }

    pub fn new_disabled() -> Self {
        Self {
            enabled: false,
            disabled: HashSet::new(),
            hooks: HashMap::new(),
            run_id: None,
        }
    }

    /// Set the current run_id for this agent run (used in hook inputs).
    pub(crate) fn set_run_id(&mut self, run_id: String) {
        self.run_id = Some(run_id);
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

        // Helper: flatten hook groups and filter out hooks without a name field.
        let filter_named = |groups: &[crate::extension::config::HookGroup]| -> Vec<HookDefinition> {
            flatten_hook_groups(groups)
                .into_iter()
                .filter(|d| {
                    if d.name.is_none() {
                        let command = crate::redaction::redact_text(&d.command);
                        tracing::warn!(target: "cosh_hook", "Skipping hook without name: {command}");
                        false
                    } else {
                        true
                    }
                })
                .collect()
        };

        self.hooks
            .entry(HookEventName::PreToolUse)
            .or_default()
            .extend(filter_named(&hooks.pre_tool_use));
        self.hooks
            .entry(HookEventName::PostToolUse)
            .or_default()
            .extend(filter_named(&hooks.post_tool_use));
        self.hooks
            .entry(HookEventName::UserPromptSubmit)
            .or_default()
            .extend(filter_named(&hooks.user_prompt_submit));
        self.hooks
            .entry(HookEventName::SessionStart)
            .or_default()
            .extend(filter_named(&hooks.session_start));
        self.hooks
            .entry(HookEventName::Stop)
            .or_default()
            .extend(filter_named(&hooks.stop));
        self.hooks
            .entry(HookEventName::PostToolUseFailure)
            .or_default()
            .extend(filter_named(&hooks.post_tool_use_failure));
        self.hooks
            .entry(HookEventName::BeforeModel)
            .or_default()
            .extend(filter_named(&hooks.before_model));
        self.hooks
            .entry(HookEventName::AfterModel)
            .or_default()
            .extend(filter_named(&hooks.after_model));
    }

    fn active_hooks(&self, event: HookEventName) -> Vec<&HookDefinition> {
        self.hooks
            .get(&event)
            .map(|defs| {
                defs.iter()
                    .filter(|d| {
                        match &d.name {
                            Some(name) => !self.disabled.contains(name),
                            // Defensive: hooks without name from config.toml still execute
                            None => true,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// cosh-ng 内部工具名 ↔ copilot-shell 标准名 双向别名映射。
    /// 仅用于 matcher 匹配阶段，不影响发给 hook 的 tool_name 字段。
    fn tool_name_alias(tool_name: &str) -> Option<&'static str> {
        match tool_name {
            "shell" => Some("run_shell_command"),
            "run_shell_command" => Some("shell"),
            "grep" => Some("grep_search"),
            "grep_search" => Some("grep"),
            "todo" => Some("todo_write"),
            "todo_write" => Some("todo"),
            _ => None,
        }
    }

    /// 从 hook_specific_output 读取 additionalContext，兼容两种字段名。
    /// 优先 snake_case（cosh-ng 原协议），次选 camelCase（copilot-shell 协议）。
    fn pick_additional_context(specific: &Value) -> Option<&str> {
        specific
            .get("additional_context")
            .and_then(|v| v.as_str())
            .or_else(|| specific.get("additionalContext").and_then(|v| v.as_str()))
    }

    /// 把 tool 输出文本包装为 copilot-shell 协议要求的 JSON 对象。
    /// 对齐 copilot-shell 行为：始终将原始文本作为 llmContent/returnDisplay
    /// 传递，即使文本本身是合法 JSON。copilot-shell 的 coreToolScheduler
    /// 会先提取文本再包装，hook 脚本始终看到统一结构。
    fn wrap_tool_response(tool_response: &str) -> Value {
        serde_json::json!({
            "llmContent": tool_response,
            "returnDisplay": tool_response,
        })
    }

    fn matches_tool(def: &HookDefinition, tool_name: &str) -> bool {
        match &def.matcher {
            None => true,
            Some(pattern) => {
                let matches_one = |name: &str| {
                    if let Ok(re) = Regex::new(pattern) {
                        re.is_match(name)
                    } else {
                        pattern == name
                    }
                };
                matches_one(tool_name) || Self::tool_name_alias(tool_name).is_some_and(matches_one)
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
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &Value,
        skill_context: Option<&Value>,
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

        let mut event_data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "tool_name": tool_name,
            "tool_input": tool_input,
        });
        if let Some(ctx) = skill_context {
            event_data["skill_context"] = ctx.clone();
        }
        let input = self.build_input(session_id, cwd, HookEventName::PreToolUse, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_pre_tool_use(outputs, &defs)
    }

    pub async fn fire_post_tool_use(
        &self,
        session_id: &str,
        cwd: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &Value,
        tool_response: &str,
        skill_context: Option<&Value>,
    ) -> PostToolUseResult {
        if !self.enabled {
            return PostToolUseResult {
                decision: HookDecision::Passthrough,
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
                decision: HookDecision::Passthrough,
                additional_context: None,
                notifications: vec![],
            };
        }

        let mut event_data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "tool_name": tool_name,
            "tool_input": tool_input,
            "tool_response": Self::wrap_tool_response(tool_response),
        });
        if let Some(ctx) = skill_context {
            event_data["skill_context"] = ctx.clone();
        }
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

    pub async fn fire_session_start(&self, session_id: &str, cwd: &str) -> SessionStartResult {
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

    pub async fn fire_stop(&self, session_id: &str, cwd: &str, last_message: &str) -> StopResult {
        if !self.enabled {
            return StopResult {
                decision: HookDecision::Passthrough,
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::Stop);
        if defs.is_empty() {
            return StopResult {
                decision: HookDecision::Passthrough,
                notifications: vec![],
            };
        }

        let event_data = serde_json::json!({ "last_assistant_message": last_message });
        let input = self.build_input(session_id, cwd, HookEventName::Stop, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        self.aggregate_stop(outputs, &defs)
    }

    pub async fn fire_post_tool_use_failure(
        &self,
        session_id: &str,
        cwd: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &Value,
        error: &str,
        skill_context: Option<&Value>,
    ) -> PostToolUseFailureResult {
        if !self.enabled {
            return PostToolUseFailureResult {
                notifications: vec![],
                sandbox_bypass_request: None,
            };
        }

        let defs: Vec<&HookDefinition> = self
            .active_hooks(HookEventName::PostToolUseFailure)
            .into_iter()
            .filter(|d| Self::matches_tool(d, tool_name))
            .collect();

        if defs.is_empty() {
            return PostToolUseFailureResult {
                notifications: vec![],
                sandbox_bypass_request: None,
            };
        }

        let mut event_data = serde_json::json!({
            "tool_use_id": tool_use_id,
            "tool_name": tool_name,
            "tool_input": tool_input,
            "error": error,
        });
        if let Some(ctx) = skill_context {
            event_data["skill_context"] = ctx.clone();
        }
        let input = self.build_input(
            session_id,
            cwd,
            HookEventName::PostToolUseFailure,
            event_data,
        );
        let outputs = self.run_hooks(&defs, &input).await;

        let mut notifications = Vec::new();
        let mut sandbox_bypass_request = None;
        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);
            // Extract sandbox_bypass_request from hookSpecificOutput (last valid wins).
            if let Some(ref specific) = out.hook_specific_output {
                if let Some(req) = specific.get("sandbox_bypass_request") {
                    if let Ok(parsed) = serde_json::from_value::<SandboxBypassRequest>(req.clone())
                    {
                        sandbox_bypass_request = Some(parsed);
                    }
                }
            }
        }
        PostToolUseFailureResult {
            notifications,
            sandbox_bypass_request,
        }
    }

    /// Temporarily disable/enable a hook by name (used for sandbox bypass).
    /// Not persisted to states/hooks.json — only affects the current session.
    pub(crate) fn set_hook_disabled(&mut self, hook_name: &str, disabled: bool) {
        if disabled {
            self.disabled.insert(hook_name.to_string());
        } else {
            self.disabled.remove(hook_name);
        }
    }

    pub async fn fire_before_model(
        &self,
        session_id: &str,
        cwd: &str,
        model: &str,
        messages: &[crate::provider::Message],
    ) -> BeforeModelResult {
        if !self.enabled {
            return BeforeModelResult {
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::BeforeModel);
        if defs.is_empty() {
            return BeforeModelResult {
                notifications: vec![],
            };
        }

        // 对齐 copilot-shell hookTranslator.toHookLLMRequest 格式：
        // llm_request 包含 model 和 messages 数组。
        let hook_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": &m.role,
                    "content": m.content.as_text(),
                })
            })
            .collect();

        let event_data = serde_json::json!({
            "llm_request": {
                "model": model,
                "messages": hook_messages,
            },
        });
        let input = self.build_input(session_id, cwd, HookEventName::BeforeModel, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        let mut notifications = Vec::new();
        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);
        }
        BeforeModelResult { notifications }
    }

    pub async fn fire_after_model(
        &self,
        session_id: &str,
        cwd: &str,
        has_tool_calls: bool,
        response_text: &str,
        model: &str,
        messages: &[crate::provider::Message],
        usage: Option<(u32, u32, u32)>,
    ) -> AfterModelResult {
        if !self.enabled {
            return AfterModelResult {
                notifications: vec![],
            };
        }

        let defs = self.active_hooks(HookEventName::AfterModel);
        if defs.is_empty() {
            return AfterModelResult {
                notifications: vec![],
            };
        }

        // 对齐 copilot-shell hookTranslator 格式：
        // llm_request: {model, messages}
        // llm_response: {text, candidates, usageMetadata}
        let hook_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": &m.role,
                    "content": m.content.as_text(),
                })
            })
            .collect();

        let mut llm_response = serde_json::json!({
            "text": response_text,
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [response_text],
                },
                "finishReason": "STOP",
            }],
        });
        if let Some((prompt_tokens, completion_tokens, total_tokens)) = usage {
            llm_response["usageMetadata"] = serde_json::json!({
                "promptTokenCount": prompt_tokens,
                "candidatesTokenCount": completion_tokens,
                "totalTokenCount": total_tokens,
            });
        }

        let event_data = serde_json::json!({
            "has_tool_calls": has_tool_calls,
            "llm_request": {
                "model": model,
                "messages": hook_messages,
            },
            "llm_response": llm_response,
        });
        let input = self.build_input(session_id, cwd, HookEventName::AfterModel, event_data);
        let outputs = self.run_hooks(&defs, &input).await;

        let mut notifications = Vec::new();
        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);
        }
        AfterModelResult { notifications }
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
            run_id: self.run_id.clone(),
            cwd: cwd.to_string(),
            hook_event_name: event.as_str().to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            transcript_path: format!("{cwd}/.cosh-transcript.jsonl"),
            event_data,
        }
    }

    async fn run_hooks(
        &self,
        defs: &[&HookDefinition],
        input: &HookInput,
    ) -> Vec<(usize, HookOutput)> {
        let input_json = crate::redaction::to_redacted_json(input);

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
        use tokio::process::Command;

        use crate::process::{output_with_timeout, OutputError};

        let safe_command = crate::redaction::redact_text(command);
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);

        // The deadline covers the stdin write as well: a hook that never
        // reads stdin must not stall the session, and on timeout the whole
        // process group is killed instead of leaking grandchildren.
        let result = output_with_timeout(cmd, Some(input_json.as_bytes().to_vec()), timeout).await;

        let output = match result {
            Ok(o) => o,
            Err(OutputError::Spawn(e)) => {
                tracing::error!(target: "cosh_hook", "Failed to spawn hook '{safe_command}': {e}");
                return HookOutput::default();
            }
            Err(OutputError::Io(e)) => {
                tracing::error!(target: "cosh_hook", "Hook '{safe_command}' execution failed: {e}");
                return HookOutput::default();
            }
            Err(OutputError::Timeout) => {
                tracing::warn!(target: "cosh_hook", "Hook '{safe_command}' timed out");
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
                let output = serde_json::from_str::<HookOutput>(trimmed).unwrap_or_else(|e| {
                    tracing::warn!(target: "cosh_hook", "Failed to parse output from '{safe_command}': {e}");
                    HookOutput::default()
                });
                Self::redact_hook_output(output)
            }
            2 => {
                // System block via exit code 2
                Self::redact_hook_output(HookOutput {
                    decision: Some("block".to_string()),
                    reason: Some(if stderr.is_empty() {
                        "Blocked by hook".to_string()
                    } else {
                        stderr.trim().to_string()
                    }),
                    system_message: None,
                    hook_specific_output: None,
                })
            }
            _ => {
                // Non-zero (not 2) = warning, do not block
                if !stderr.is_empty() {
                    let stderr = crate::redaction::redact_text(stderr.trim());
                    tracing::warn!(target: "cosh_hook", "Hook '{safe_command}' warning: {stderr}");
                }
                HookOutput::default()
            }
        }
    }

    fn redact_hook_output(mut output: HookOutput) -> HookOutput {
        output.reason = output
            .reason
            .map(|value| crate::redaction::redact_text(&value));
        output.system_message = output
            .system_message
            .map(|value| crate::redaction::redact_text(&value));
        if let Some(value) = &mut output.hook_specific_output {
            crate::redaction::redact_value(value);
        }
        output
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

            decision = fold_decision(decision, out.decision.as_deref(), out.reason.clone());

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
        let mut decision = HookDecision::Passthrough;
        let mut additional_context: Option<String> = None;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            decision = fold_decision(decision, out.decision.as_deref(), out.reason.clone());
            fold_additional_context(&mut additional_context, &out.hook_specific_output);
        }

        PostToolUseResult {
            decision,
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

            decision = fold_decision(decision, out.decision.as_deref(), out.reason.clone());
            fold_additional_context(&mut additional_context, &out.hook_specific_output);
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

            fold_additional_context(&mut additional_context, &out.hook_specific_output);
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
        let mut decision = HookDecision::Passthrough;
        let mut notifications = Vec::new();

        for (i, out) in outputs {
            let name = Self::hook_name(defs[i], i);
            self.collect_notifications(&out, &name, &mut notifications);

            decision = fold_decision(decision, out.decision.as_deref(), out.reason.clone());
        }

        StopResult {
            decision,
            notifications,
        }
    }

    fn collect_notifications(
        &self,
        output: &HookOutput,
        hook_name: &str,
        notifications: &mut Vec<HookNotification>,
    ) {
        // Use systemMessage if present, otherwise fall back to reason.
        // This avoids duplicate notifications when both fields exist on block/deny.
        let msg = output.system_message.as_ref().or(output.reason.as_ref());
        if let Some(msg) = msg {
            notifications.push(HookNotification {
                hook_name: hook_name.to_string(),
                message: msg.clone(),
                decision: output.decision.clone(),
            });
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

// ─── Decision Aggregation Primitives ─────────────────────────────────

/// Fold a raw hook output decision string into the running `HookDecision`.
///
/// Priority (highest wins): Block > Ask > Allow > Passthrough.
/// "reject" is treated as equivalent to "block"/"deny" (used by Stop hooks).
fn fold_decision(current: HookDecision, raw: Option<&str>, reason: Option<String>) -> HookDecision {
    match raw {
        Some("block") | Some("deny") | Some("reject") => {
            // Preserve the first non-empty block reason; don't let a later
            // hook without a reason overwrite an existing detailed message.
            match (&current, &reason) {
                (HookDecision::Block(_), None) => current,
                _ => HookDecision::Block(reason.unwrap_or_else(|| "Blocked by hook".to_string())),
            }
        }
        Some("ask") => match current {
            HookDecision::Block(_) => current,
            _ => HookDecision::Ask,
        },
        Some("approve") | Some("allow") => match current {
            HookDecision::Passthrough => HookDecision::Allow,
            _ => current,
        },
        _ => current,
    }
}

/// Extract `additionalContext` from `hook_specific_output` and append-merge
/// into the running accumulator.
fn fold_additional_context(current: &mut Option<String>, specific: &Option<Value>) {
    if let Some(ref specific) = specific {
        if let Some(ctx) = HookSystem::pick_additional_context(specific) {
            *current = Some(match current.take() {
                Some(existing) => format!("{existing}\n{ctx}"),
                None => ctx.to_string(),
            });
        }
    }
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
        let json =
            r#"{"decision":"allow","hook_specific_output":{"tool_input":{"safe_mode":true}}}"#;
        let out: HookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.decision.as_deref(), Some("allow"));
        let patch = out.hook_specific_output.unwrap();
        assert_eq!(patch["tool_input"]["safe_mode"], true);
    }

    #[test]
    fn hook_output_is_redacted_before_aggregation() {
        let secret = "short-hook-secret";
        let output = HookOutput {
            decision: Some("block".to_string()),
            reason: Some(format!("password={secret}")),
            system_message: Some(format!("Bearer {secret}")),
            hook_specific_output: Some(serde_json::json!({
                "additionalContext": format!("api_key={secret}"),
                "tool_input": {"token": secret}
            })),
        };

        let output = HookSystem::redact_hook_output(output);
        let serialized = format!(
            "{} {} {}",
            output.reason.as_deref().unwrap_or_default(),
            output.system_message.as_deref().unwrap_or_default(),
            output
                .hook_specific_output
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default()
        );

        assert!(!serialized.contains(secret), "{serialized}");
        assert!(serialized.contains("<redacted>"), "{serialized}");
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
        let result =
            rt.block_on(sys.fire_pre_tool_use("s1", "/tmp", "tool-1", "shell", &Value::Null, None));
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

            pre_tool_use: vec![HookDefinition {
                command: "echo '{\"decision\":\"block\",\"reason\":\"no rm allowed\"}'".to_string(),
                name: Some("block-rm".to_string()),
                matcher: Some("run_shell_command".to_string()),
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use: vec![],
            post_tool_use_failure: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
            before_model: vec![],
            after_model: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use(
                "s1",
                "/tmp",
                "tool-1",
                "run_shell_command",
                &serde_json::json!({"command": "rm -rf /"}),
                None,
            )
            .await;
        assert_eq!(
            result.decision,
            HookDecision::Block("no rm allowed".to_string())
        );
        assert!(!result.notifications.is_empty());
    }

    #[tokio::test]
    async fn fire_pre_tool_use_no_match() {
        let config = HooksConfig {
            enabled: true,

            pre_tool_use: vec![HookDefinition {
                command: "echo '{\"decision\":\"block\",\"reason\":\"no\"}'".to_string(),
                name: None,
                matcher: Some("run_shell_command".to_string()),
                timeout: None,
                sequential: None,
            }],
            post_tool_use: vec![],
            post_tool_use_failure: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
            before_model: vec![],
            after_model: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use(
                "s1",
                "/tmp",
                "tool-1",
                "read_file",
                &serde_json::json!({}),
                None,
            )
            .await;
        assert_eq!(result.decision, HookDecision::Passthrough);
    }

    #[tokio::test]
    async fn exit_code_2_means_block() {
        let config = HooksConfig {
            enabled: true,

            pre_tool_use: vec![HookDefinition {
                command: "sh -c 'echo blocked >&2; exit 2'".to_string(),
                name: Some("exit2-hook".to_string()),
                matcher: None,
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use: vec![],
            post_tool_use_failure: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
            before_model: vec![],
            after_model: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_pre_tool_use("s1", "/tmp", "tool-1", "any", &serde_json::json!({}), None)
            .await;
        assert_eq!(result.decision, HookDecision::Block("blocked".to_string()));
    }

    // ===== Task 1: matcher 双向工具名兼容 =====

    fn def_with_matcher(matcher: &str) -> HookDefinition {
        HookDefinition {
            command: "echo".to_string(),
            name: None,
            matcher: Some(matcher.to_string()),
            timeout: None,
            sequential: None,
        }
    }

    #[test]
    fn matcher_matches_alias_run_shell_command() {
        // matcher 写 copilot-shell 名字，cosh-ng 内部名也能命中
        let def = def_with_matcher("^run_shell_command$");
        assert!(HookSystem::matches_tool(&def, "shell"));
        assert!(HookSystem::matches_tool(&def, "run_shell_command"));
    }

    #[test]
    fn matcher_matches_alias_shell() {
        // matcher 写 cosh-ng 名字，copilot-shell 名字也能命中
        let def = def_with_matcher("^shell$");
        assert!(HookSystem::matches_tool(&def, "shell"));
        assert!(HookSystem::matches_tool(&def, "run_shell_command"));
    }

    #[test]
    fn matcher_alias_grep_and_todo() {
        let def_grep = def_with_matcher("^grep_search$");
        assert!(HookSystem::matches_tool(&def_grep, "grep"));
        let def_todo = def_with_matcher("^todo_write$");
        assert!(HookSystem::matches_tool(&def_todo, "todo"));
    }

    #[test]
    fn matcher_unknown_tool_no_alias() {
        // 不在别名表的工具名走原路径
        let def = def_with_matcher("^read_file$");
        assert!(HookSystem::matches_tool(&def, "read_file"));
        assert!(!HookSystem::matches_tool(&def, "shell"));
    }

    // ===== Task 2: additionalContext 双向兼容 =====

    #[test]
    fn pick_additional_context_prefers_snake_case() {
        let v = serde_json::json!({
            "additional_context": "snake",
            "additionalContext": "camel"
        });
        assert_eq!(HookSystem::pick_additional_context(&v), Some("snake"));
    }

    #[test]
    fn pick_additional_context_falls_back_to_camel_case() {
        let v = serde_json::json!({"additionalContext": "only-camel"});
        assert_eq!(HookSystem::pick_additional_context(&v), Some("only-camel"));
    }

    #[test]
    fn pick_additional_context_returns_none_when_absent() {
        let v = serde_json::json!({"other": "x"});
        assert_eq!(HookSystem::pick_additional_context(&v), None);
    }

    // ===== Task 3: tool_response 包装 =====

    #[test]
    fn wrap_tool_response_plain_text() {
        let v = HookSystem::wrap_tool_response("hello world");
        assert_eq!(v["llmContent"], "hello world");
        assert_eq!(v["returnDisplay"], "hello world");
    }

    #[test]
    fn wrap_tool_response_passes_through_object() {
        // 对齐 copilot-shell 行为：即使原始文本是 JSON，仍作为文本包装进 llmContent。
        let raw = r#"{"llmContent":"x","returnDisplay":"y"}"#;
        let v = HookSystem::wrap_tool_response(raw);
        assert_eq!(v["llmContent"], raw);
        assert_eq!(v["returnDisplay"], raw);
    }

    #[test]
    fn wrap_tool_response_wraps_array_as_text() {
        // 对齐 copilot-shell：数组也作为文本包装，而非透传。
        let raw = r#"[1,2,3]"#;
        let v = HookSystem::wrap_tool_response(raw);
        assert_eq!(v["llmContent"], raw);
        assert_eq!(v["returnDisplay"], raw);
    }

    #[test]
    fn wrap_tool_response_bare_number_is_wrapped() {
        // 裸数字虽是合法 JSON 但不是 object/array，仍需要包装。
        let v = HookSystem::wrap_tool_response("42");
        assert_eq!(v["llmContent"], "42");
    }

    // ===== Task 4: HookInput / event_data 新字段 =====

    #[test]
    fn hook_input_contains_transcript_path() {
        let sys = HookSystem::new_disabled();
        let input = sys.build_input(
            "sess-1",
            "/work",
            HookEventName::PreToolUse,
            serde_json::json!({"tool_name": "shell"}),
        );
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["transcript_path"], "/work/.cosh-transcript.jsonl");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["hook_event_name"], "PreToolUse");
    }

    #[tokio::test]
    async fn event_data_contains_tool_use_id_and_keeps_native_tool_name() {
        // 调用方传 cosh-ng 原名 shell + matcher 写 run_shell_command。
        // hook 脚本输出使用输入中的 tool_name 与 tool_use_id 作为上下文验证。
        let config = HooksConfig {
            enabled: true,

            pre_tool_use: vec![],
            post_tool_use: vec![HookDefinition {
                command: r#"python3 -c 'import sys,json; d=json.load(sys.stdin); print(json.dumps({"hook_specific_output": {"additionalContext": d["tool_name"]+"|"+d["tool_use_id"]}}))'"#.to_string(),
                name: Some("echo".to_string()),
                matcher: Some("^run_shell_command$".to_string()),
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use_failure: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
            before_model: vec![],
            after_model: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let result = sys
            .fire_post_tool_use(
                "s1",
                "/tmp",
                "call-42",
                "shell",
                &serde_json::json!({"command": "ls"}),
                "hello",
                None,
            )
            .await;
        // additional_context 里会包含传入的 cosh-ng 原名 shell 与 tool_use_id call-42
        let ctx = result.additional_context.unwrap();
        assert!(ctx.contains("shell"), "ctx={ctx}");
        assert!(ctx.contains("call-42"), "ctx={ctx}");
    }

    #[tokio::test]
    async fn event_data_includes_skill_context_when_provided() {
        // hook 脚本反射 skill_context.file_path 到 additionalContext 验证透传。
        // PreToolUse 不会在 additional_context 里体现 hook 输出，改用
        // PostToolUse 路径验证（不同处理器但同样读 skill_context）。
        let config = HooksConfig {
            enabled: true,

            pre_tool_use: vec![],
            post_tool_use: vec![HookDefinition {
                command: r#"python3 -c 'import sys,json; d=json.load(sys.stdin); ctx=d.get("skill_context",{}); print(json.dumps({"hook_specific_output":{"additionalContext": ctx.get("file_path","none")}}))'"#.to_string(),
                name: Some("skill-probe".to_string()),
                matcher: Some("skill".to_string()),
                timeout: Some(5000),
                sequential: None,
            }],
            post_tool_use_failure: vec![],
            user_prompt_submit: vec![],
            session_start: vec![],
            stop: vec![],
            before_model: vec![],
            after_model: vec![],
        };
        let sys = HookSystem::from_config(&config);
        let skill_ctx = serde_json::json!({
            "skill_name": "demo",
            "file_path": "/skills/demo/SKILL.md",
        });
        let result = sys
            .fire_post_tool_use(
                "s1",
                "/tmp",
                "call-1",
                "skill",
                &serde_json::json!({"action": "invoke", "name": "demo"}),
                "",
                Some(&skill_ctx),
            )
            .await;
        assert_eq!(
            result.additional_context.as_deref(),
            Some("/skills/demo/SKILL.md"),
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_timeout_kills_process_group() {
        use crate::process::test_support::*;

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");
        let script = leak_script(&marker, &pid_file);

        let started = std::time::Instant::now();
        let out = HookSystem::run_hook_cmd(&script, "{}", Duration::from_millis(300)).await;
        assert!(
            out.decision.is_none(),
            "timed-out hook must fall back to the default output"
        );

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());
        for pid in &pids {
            assert_process_gone(*pid);
        }
        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the hook timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_ignoring_stdin_respects_deadline() {
        // Payload larger than the pipe buffer: the old implementation
        // blocked in the stdin write before the timeout even started.
        let payload = "x".repeat(1 << 20);
        let started = std::time::Instant::now();
        let out = HookSystem::run_hook_cmd("sleep 30", &payload, Duration::from_millis(300)).await;
        assert!(out.decision.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stdin write must be bounded by the hook deadline"
        );
    }
}
