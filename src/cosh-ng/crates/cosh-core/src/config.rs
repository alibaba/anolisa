use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CoreConfig {
    #[serde(default)]
    pub ai: AiConfig,
    // Keeps user-layer AI preferences out of project override persistence.
    #[serde(skip)]
    pub(crate) user_ai: AiConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AiConfig {
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub output_language: Option<String>,
    pub thinking: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderConfig {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_approval_mode")]
    pub approval_mode: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_session_token_limit")]
    pub session_token_limit: u64,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls_per_turn: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            approval_mode: default_approval_mode(),
            max_turns: default_max_turns(),
            session_token_limit: default_session_token_limit(),
            max_tool_calls_per_turn: default_max_tool_calls(),
        }
    }
}

fn default_approval_mode() -> String {
    "balanced".to_string()
}
fn default_max_turns() -> u32 {
    20
}
fn default_session_token_limit() -> u64 {
    128_000
}
fn default_max_tool_calls() -> u32 {
    10
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HooksConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "PreToolUse")]
    pub pre_tool_use: Vec<HookDefinition>,
    #[serde(default, rename = "PostToolUse")]
    pub post_tool_use: Vec<HookDefinition>,
    #[serde(default, rename = "PostToolUseFailure")]
    pub post_tool_use_failure: Vec<HookDefinition>,
    #[serde(default, rename = "UserPromptSubmit")]
    pub user_prompt_submit: Vec<HookDefinition>,
    #[serde(default, rename = "SessionStart")]
    pub session_start: Vec<HookDefinition>,
    #[serde(default, rename = "Stop")]
    pub stop: Vec<HookDefinition>,
    #[serde(default, rename = "BeforeModel")]
    pub before_model: Vec<HookDefinition>,
    #[serde(default, rename = "AfterModel")]
    pub after_model: Vec<HookDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookDefinition {
    pub command: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub sequential: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SkillsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub custom_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub auto_persist: bool,
    #[serde(default = "default_persist_dir")]
    pub persist_dir: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            auto_persist: true,
            persist_dir: default_persist_dir(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_persist_dir() -> String {
    "sessions".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LoggingConfig {
    pub level: Option<String>,
}

impl LoggingConfig {
    pub fn effective_level(&self, verbose: bool) -> String {
        if let Ok(v) = std::env::var("COSH_LOG") {
            return v;
        }
        if verbose {
            return "debug".to_string();
        }
        self.level.clone().unwrap_or_else(|| "warn".to_string())
    }
}

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot-shell")
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialCoreConfig {
    ai: Option<PartialAiConfig>,
    agent: Option<PartialAgentConfig>,
    hooks: Option<PartialHooksConfig>,
    skills: Option<PartialSkillsConfig>,
    session: Option<PartialSessionConfig>,
    logging: Option<PartialLoggingConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialAiConfig {
    active_provider: Option<String>,
    active_model: Option<String>,
    output_language: Option<String>,
    thinking: Option<String>,
    #[serde(default)]
    providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialAgentConfig {
    approval_mode: Option<String>,
    max_turns: Option<u32>,
    session_token_limit: Option<u64>,
    max_tool_calls_per_turn: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialHooksConfig {
    enabled: Option<bool>,
    #[serde(rename = "PreToolUse")]
    pre_tool_use: Option<Vec<HookDefinition>>,
    #[serde(rename = "PostToolUse")]
    post_tool_use: Option<Vec<HookDefinition>>,
    #[serde(rename = "PostToolUseFailure")]
    post_tool_use_failure: Option<Vec<HookDefinition>>,
    #[serde(rename = "UserPromptSubmit")]
    user_prompt_submit: Option<Vec<HookDefinition>>,
    #[serde(rename = "SessionStart")]
    session_start: Option<Vec<HookDefinition>>,
    #[serde(rename = "Stop")]
    stop: Option<Vec<HookDefinition>>,
    #[serde(rename = "BeforeModel")]
    before_model: Option<Vec<HookDefinition>>,
    #[serde(rename = "AfterModel")]
    after_model: Option<Vec<HookDefinition>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialSkillsConfig {
    enabled: Option<bool>,
    custom_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialSessionConfig {
    auto_persist: Option<bool>,
    persist_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PartialLoggingConfig {
    level: Option<String>,
}

fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

fn read_partial_config(path: &std::path::Path) -> Option<PartialCoreConfig> {
    if !path.exists() {
        return None;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!(
                "[cosh-core] Warning: failed to read {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    match toml::from_str::<PartialCoreConfig>(&content) {
        Ok(config) => Some(config),
        Err(e) => {
            eprintln!(
                "[cosh-core] Warning: failed to parse {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

fn apply_user_layer(config: &mut CoreConfig, layer: &PartialCoreConfig) {
    if let Some(ref ai) = layer.ai {
        if let Some(ref value) = ai.active_provider {
            config.ai.active_provider = Some(value.clone());
        }
        apply_ai_preferences(&mut config.ai, ai);
        for (provider_id, provider) in &ai.providers {
            config
                .ai
                .providers
                .insert(provider_id.clone(), provider.clone());
        }
    }
    apply_common_layers(config, layer);
}

fn apply_project_layer(config: &mut CoreConfig, layer: &PartialCoreConfig, path: &std::path::Path) {
    if let Some(ref ai) = layer.ai {
        if ai.active_provider.is_some() {
            eprintln!(
                "[cosh-core] Warning: ignoring active_provider from project config {}",
                path.display()
            );
        }
        if !ai.providers.is_empty() {
            eprintln!(
                "[cosh-core] Warning: ignoring ai.providers from project config {}",
                path.display()
            );
        }
        apply_ai_preferences(&mut config.ai, ai);
    }
    apply_common_layers(config, layer);
}

fn apply_ai_preferences(ai: &mut AiConfig, layer: &PartialAiConfig) {
    if let Some(ref value) = layer.active_model {
        ai.active_model = Some(value.clone());
    }
    if let Some(ref value) = layer.output_language {
        ai.output_language = Some(value.clone());
    }
    if let Some(ref value) = layer.thinking {
        ai.thinking = Some(value.clone());
    }
}

fn apply_common_layers(config: &mut CoreConfig, layer: &PartialCoreConfig) {
    if let Some(ref agent) = layer.agent {
        apply_agent_layer(&mut config.agent, agent);
    }
    if let Some(ref hooks) = layer.hooks {
        apply_hooks_layer(&mut config.hooks, hooks);
    }
    if let Some(ref skills) = layer.skills {
        apply_skills_layer(&mut config.skills, skills);
    }
    if let Some(ref session) = layer.session {
        apply_session_layer(&mut config.session, session);
    }
    if let Some(ref logging) = layer.logging {
        apply_logging_layer(&mut config.logging, logging);
    }
}

fn apply_agent_layer(config: &mut AgentConfig, layer: &PartialAgentConfig) {
    if let Some(ref value) = layer.approval_mode {
        config.approval_mode = value.clone();
    }
    if let Some(value) = layer.max_turns {
        config.max_turns = value;
    }
    if let Some(value) = layer.session_token_limit {
        config.session_token_limit = value;
    }
    if let Some(value) = layer.max_tool_calls_per_turn {
        config.max_tool_calls_per_turn = value;
    }
}

fn apply_hooks_layer(config: &mut HooksConfig, layer: &PartialHooksConfig) {
    if let Some(value) = layer.enabled {
        config.enabled = value;
    }
    if let Some(ref value) = layer.pre_tool_use {
        config.pre_tool_use = value.clone();
    }
    if let Some(ref value) = layer.post_tool_use {
        config.post_tool_use = value.clone();
    }
    if let Some(ref value) = layer.post_tool_use_failure {
        config.post_tool_use_failure = value.clone();
    }
    if let Some(ref value) = layer.user_prompt_submit {
        config.user_prompt_submit = value.clone();
    }
    if let Some(ref value) = layer.session_start {
        config.session_start = value.clone();
    }
    if let Some(ref value) = layer.stop {
        config.stop = value.clone();
    }
    if let Some(ref value) = layer.before_model {
        config.before_model = value.clone();
    }
    if let Some(ref value) = layer.after_model {
        config.after_model = value.clone();
    }
}

fn apply_skills_layer(config: &mut SkillsConfig, layer: &PartialSkillsConfig) {
    if let Some(value) = layer.enabled {
        config.enabled = value;
    }
    if let Some(ref value) = layer.custom_paths {
        config.custom_paths = value.clone();
    }
}

fn apply_session_layer(config: &mut SessionConfig, layer: &PartialSessionConfig) {
    if let Some(value) = layer.auto_persist {
        config.auto_persist = value;
    }
    if let Some(ref value) = layer.persist_dir {
        config.persist_dir = value.clone();
    }
}

fn apply_logging_layer(config: &mut LoggingConfig, layer: &PartialLoggingConfig) {
    if let Some(ref value) = layer.level {
        config.level = Some(value.clone());
    }
}

fn normalize_partial_legacy_sts_auth_sources(config: &mut PartialCoreConfig) -> bool {
    let Some(ref mut ai) = config.ai else {
        return false;
    };

    let mut changed = false;
    for provider in ai.providers.values_mut() {
        let is_aliyun = provider.provider_type.as_deref() == Some("aliyun");
        if is_aliyun && provider.security_token.is_some() {
            provider.auth_source = Some("ecs_ram_role".to_string());
            provider.access_key_id = None;
            provider.access_key_secret = None;
            provider.security_token = None;
            changed = true;
        }
    }
    changed
}

impl CoreConfig {
    pub fn load() -> Self {
        crate::migrate::try_migrate();

        let project_path = std::env::current_dir()
            .ok()
            .map(|p| p.join(".copilot-shell/config.toml"));
        let user_path = config_dir().join("config.toml");
        let system_path = PathBuf::from("/etc/copilot-shell/config.toml");

        let mut config = Self::load_from_paths(
            Some(&system_path),
            Some(&user_path),
            project_path.as_deref(),
        );
        config.apply_env_overrides();
        config
    }

    fn load_from_paths(
        system_path: Option<&std::path::Path>,
        user_path: Option<&std::path::Path>,
        project_path: Option<&std::path::Path>,
    ) -> Self {
        let mut config = CoreConfig::default();

        if let Some(system_path) = system_path {
            if let Some(system) = read_partial_config(system_path) {
                apply_user_layer(&mut config, &system);
            }
        }

        if let Some(user_path) = user_path {
            if let Some(mut user) = read_partial_config(user_path) {
                let normalized = normalize_partial_legacy_sts_auth_sources(&mut user);
                apply_user_layer(&mut config, &user);
                let mut user_snapshot = CoreConfig::default();
                apply_user_layer(&mut user_snapshot, &user);
                user_snapshot.user_ai = user_snapshot.ai.clone();
                config.user_ai = user_snapshot.ai.clone();

                if normalized {
                    if let Some(parent) = user_path.parent() {
                        if let Err(e) = persist_config_to_dir(&user_snapshot, parent) {
                            eprintln!(
                                "[cosh-core] Warning: failed to normalize STS provider config: {e}"
                            );
                        }
                    }
                }
            }
        }

        if let Some(project_path) = project_path {
            if let Some(project) = read_partial_config(project_path) {
                apply_project_layer(&mut config, &project, project_path);
            }
        }

        config
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("COSH_APPROVAL_MODE") {
            self.agent.approval_mode = val;
        }
        if let Ok(val) = std::env::var("COSH_MODEL") {
            self.ai.active_model = Some(val);
        }
        if let Ok(val) = std::env::var("COSH_AI_PROVIDER") {
            self.ai.active_provider = Some(val);
        }
        if let Ok(val) = std::env::var("COSH_OUTPUT_LANGUAGE") {
            self.ai.output_language = Some(val);
        }
        if let Ok(val) = std::env::var("COSH_MAX_TURNS") {
            if let Ok(n) = val.parse::<u32>() {
                self.agent.max_turns = n;
            }
        }
    }

    pub fn normalize_legacy_sts_auth_sources(&mut self) -> bool {
        let mut changed = false;
        for provider in self.ai.providers.values_mut() {
            let is_aliyun = provider.provider_type.as_deref() == Some("aliyun");
            if is_aliyun && provider.security_token.is_some() {
                provider.auth_source = Some("ecs_ram_role".to_string());
                provider.access_key_id = None;
                provider.access_key_secret = None;
                provider.security_token = None;
                changed = true;
            }
        }
        changed
    }

    pub fn resolve_provider(&self) -> ResolvedProvider {
        let provider_name = self
            .ai
            .active_provider
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let provider_cfg = self.ai.providers.get(&provider_name);

        let base_url = provider_cfg
            .and_then(|p| p.base_url.as_deref())
            .map(expand_env_vars)
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());

        let api_key = provider_cfg
            .and_then(|p| p.api_key.as_deref())
            .map(expand_env_vars)
            .or_else(|| std::env::var("DASHSCOPE_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .unwrap_or_default();

        let model = self
            .ai
            .active_model
            .clone()
            .or_else(|| provider_cfg.and_then(|p| p.model.clone()))
            .unwrap_or_else(|| "qwen-max".to_string());

        let provider_type = provider_cfg
            .and_then(|p| p.provider_type.clone())
            .unwrap_or_else(|| "generic".to_string());

        let extra_params = provider_cfg.and_then(|p| p.extra_params.clone());

        let auth_source = provider_cfg.and_then(|p| p.auth_source.clone());

        let access_key_id = provider_cfg
            .and_then(|p| p.access_key_id.as_deref())
            .map(expand_env_vars)
            .or_else(|| std::env::var("ALIBABA_CLOUD_ACCESS_KEY_ID").ok())
            .unwrap_or_default();

        let access_key_secret = provider_cfg
            .and_then(|p| p.access_key_secret.as_deref())
            .map(expand_env_vars)
            .or_else(|| std::env::var("ALIBABA_CLOUD_ACCESS_KEY_SECRET").ok())
            .unwrap_or_default();

        let security_token = provider_cfg
            .and_then(|p| p.security_token.as_deref())
            .map(expand_env_vars)
            .or_else(|| std::env::var("ALIBABA_CLOUD_SECURITY_TOKEN").ok());

        ResolvedProvider {
            base_url,
            api_key,
            model,
            provider_type,
            auth_source,
            extra_params,
            access_key_id,
            access_key_secret,
            security_token,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub provider_type: String,
    pub auth_source: Option<String>,
    pub extra_params: Option<Value>,
    pub access_key_id: String,
    pub access_key_secret: String,
    pub security_token: Option<String>,
}

/// Persist the current provider config to `~/.copilot-shell/config.toml`.
/// Only writes the [ai] section to avoid overwriting other settings.
pub fn persist_config(config: &CoreConfig) -> Result<(), String> {
    let dir = config_dir();
    persist_config_to_dir(config, &dir)
}

fn persist_config_to_dir(config: &CoreConfig, dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create config dir: {e}"))?;

    let config_path = dir.join("config.toml");

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    let mut preserved = String::new();
    let mut in_ai_section = false;
    for line in existing.lines() {
        if line.trim().starts_with("[ai") {
            in_ai_section = true;
            continue;
        }
        if in_ai_section && line.trim().starts_with('[') && !line.trim().starts_with("[ai") {
            in_ai_section = false;
        }
        if !in_ai_section {
            preserved.push_str(line);
            preserved.push('\n');
        }
    }

    preserved.push_str("[ai]\n");
    if let Some(ref active) = config.ai.active_provider {
        preserved.push_str(&format!(
            "active_provider = \"{}\"\n",
            escape_toml_value(active)
        ));
    }
    if let Some(ref model) = config.user_ai.active_model {
        preserved.push_str(&format!(
            "active_model = \"{}\"\n",
            escape_toml_value(model)
        ));
    }
    if let Some(ref lang) = config.user_ai.output_language {
        preserved.push_str(&format!(
            "output_language = \"{}\"\n",
            escape_toml_value(lang)
        ));
    }
    if let Some(ref thinking) = config.user_ai.thinking {
        preserved.push_str(&format!("thinking = \"{}\"\n", escape_toml_value(thinking)));
    }
    preserved.push('\n');

    for (name, provider) in &config.ai.providers {
        preserved.push_str(&format!("[ai.providers.{}]\n", name));
        if let Some(ref t) = provider.provider_type {
            preserved.push_str(&format!("type = \"{}\"\n", escape_toml_value(t)));
        }
        if let Some(ref source) = provider.auth_source {
            preserved.push_str(&format!(
                "auth_source = \"{}\"\n",
                escape_toml_value(source)
            ));
        }
        if let Some(ref url) = provider.base_url {
            preserved.push_str(&format!("base_url = \"{}\"\n", escape_toml_value(url)));
        }
        if let Some(ref key) = provider.api_key {
            preserved.push_str(&format!("api_key = \"{}\"\n", escape_toml_value(key)));
        }
        if let Some(ref m) = provider.model {
            preserved.push_str(&format!("model = \"{}\"\n", escape_toml_value(m)));
        }
        if let Some(ref ak) = provider.access_key_id {
            preserved.push_str(&format!("access_key_id = \"{}\"\n", escape_toml_value(ak)));
        }
        if let Some(ref sk) = provider.access_key_secret {
            preserved.push_str(&format!(
                "access_key_secret = \"{}\"\n",
                escape_toml_value(sk)
            ));
        }
        if let Some(ref st) = provider.security_token {
            preserved.push_str(&format!("security_token = \"{}\"\n", escape_toml_value(st)));
        }
        preserved.push('\n');
    }

    let pid = std::process::id();
    let tmp_path = dir.join(format!("config.toml.tmp.{pid}"));
    std::fs::write(&tmp_path, &preserved).map_err(|e| format!("Failed to write config: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }

    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to rename config: {e}")
    })?;

    Ok(())
}

fn escape_toml_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = CoreConfig::default();
        assert_eq!(config.agent.approval_mode, "balanced");
        assert_eq!(config.agent.max_turns, 20);
        assert_eq!(config.agent.session_token_limit, 128_000);
        assert_eq!(config.agent.max_tool_calls_per_turn, 10);
        assert!(config.session.auto_persist);
    }

    #[test]
    fn parse_toml_config() {
        let toml_str = r#"
[ai]
active_provider = "qwen"
active_model = "qwen3-235b-a22b"
output_language = "zh-CN"

[ai.providers.qwen]
type = "openai_compat"
base_url = "https://example.com/v1"
api_key = "sk-test"
model = "qwen3-235b-a22b"

[agent]
approval_mode = "trust"
max_turns = 50
session_token_limit = 256000
max_tool_calls_per_turn = 20
"#;
        let config: CoreConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ai.active_provider.as_deref(), Some("qwen"));
        assert_eq!(config.ai.active_model.as_deref(), Some("qwen3-235b-a22b"));
        assert_eq!(config.ai.output_language.as_deref(), Some("zh-CN"));

        let qwen = config.ai.providers.get("qwen").unwrap();
        assert_eq!(qwen.provider_type.as_deref(), Some("openai_compat"));
        assert_eq!(qwen.base_url.as_deref(), Some("https://example.com/v1"));
        assert_eq!(qwen.api_key.as_deref(), Some("sk-test"));

        assert_eq!(config.agent.approval_mode, "trust");
        assert_eq!(config.agent.max_turns, 50);
    }

    #[test]
    fn parse_ecs_ram_role_auth_source() {
        let toml_str = r#"
[ai]
active_provider = "aliyun-ecs"

[ai.providers.aliyun-ecs]
type = "aliyun"
auth_source = "ecs_ram_role"
model = "qwen3.7-plus"
"#;

        let config: CoreConfig = toml::from_str(toml_str).unwrap();
        let provider = config.ai.providers.get("aliyun-ecs").unwrap();
        assert_eq!(provider.provider_type.as_deref(), Some("aliyun"));
        assert_eq!(provider.auth_source.as_deref(), Some("ecs_ram_role"));
        assert!(provider.access_key_id.is_none());
        assert!(provider.access_key_secret.is_none());
        assert!(provider.security_token.is_none());
    }

    #[test]
    fn normalize_legacy_aliyun_sts_provider_to_ecs_auth_source() {
        let toml_str = r#"
[ai]
active_provider = "aliyun"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = "legacy-ak"
access_key_secret = "legacy-sk"
security_token = "legacy-token"
model = "qwen3.7-plus"
"#;

        let mut config: CoreConfig = toml::from_str(toml_str).unwrap();
        assert!(config.normalize_legacy_sts_auth_sources());

        let provider = config.ai.providers.get("aliyun").unwrap();
        assert_eq!(provider.auth_source.as_deref(), Some("ecs_ram_role"));
        assert!(provider.access_key_id.is_none());
        assert!(provider.access_key_secret.is_none());
        assert!(provider.security_token.is_none());
        assert_eq!(provider.model.as_deref(), Some("qwen3.7-plus"));
    }

    #[test]
    fn resolve_provider_from_config() {
        let toml_str = r#"
[ai]
active_provider = "qwen"
active_model = "my-model"

[ai.providers.qwen]
type = "openai_compat"
base_url = "https://example.com/v1"
api_key = "sk-test"
model = "qwen3-235b-a22b"
"#;
        let config: CoreConfig = toml::from_str(toml_str).unwrap();
        let resolved = config.resolve_provider();
        assert_eq!(resolved.base_url, "https://example.com/v1");
        assert_eq!(resolved.api_key, "sk-test");
        assert_eq!(resolved.model, "my-model");
    }

    #[test]
    fn expand_env_vars_in_api_key() {
        std::env::set_var("TEST_COSH_KEY", "sk-from-env");
        let result = expand_env_vars("${TEST_COSH_KEY}");
        assert_eq!(result, "sk-from-env");
        std::env::remove_var("TEST_COSH_KEY");
    }

    #[test]
    fn expand_env_vars_no_match() {
        let result = expand_env_vars("plain-text");
        assert_eq!(result, "plain-text");
    }

    #[test]
    fn partial_config_uses_defaults() {
        let toml_str = r#"
[ai]
active_model = "test-model"
"#;
        let config: CoreConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.approval_mode, "balanced");
        assert_eq!(config.agent.max_turns, 20);
        assert!(config.ai.providers.is_empty());
    }

    #[test]
    fn env_overrides() {
        // All env var tests in one function to avoid parallel race conditions.
        // Phase 1: valid overrides
        std::env::set_var("COSH_APPROVAL_MODE", "trust");
        std::env::set_var("COSH_MODEL", "gpt-4");
        std::env::set_var("COSH_MAX_TURNS", "50");
        std::env::set_var("COSH_OUTPUT_LANGUAGE", "zh-CN");

        let mut config = CoreConfig::default();
        config.apply_env_overrides();

        assert_eq!(config.agent.approval_mode, "trust");
        assert_eq!(config.ai.active_model.as_deref(), Some("gpt-4"));
        assert_eq!(config.agent.max_turns, 50);
        assert_eq!(config.ai.output_language.as_deref(), Some("zh-CN"));

        // Phase 2: invalid max_turns — should be ignored
        std::env::set_var("COSH_MAX_TURNS", "not-a-number");
        let mut config2 = CoreConfig::default();
        config2.apply_env_overrides();
        assert_eq!(config2.agent.max_turns, 20);

        // Cleanup
        std::env::remove_var("COSH_APPROVAL_MODE");
        std::env::remove_var("COSH_MODEL");
        std::env::remove_var("COSH_MAX_TURNS");
        std::env::remove_var("COSH_OUTPUT_LANGUAGE");
    }

    #[test]
    fn layered_config_preserves_user_provider_and_project_model() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_path = tmp.path().join("user-config.toml");
        let project_path = tmp.path().join("project-config.toml");

        std::fs::write(
            &user_path,
            r#"
[ai]
active_provider = "dashscope"
active_model = "user-model"

[ai.providers.dashscope]
type = "dashscope"
api_key = "sk-user"
model = "provider-model"
"#,
        )
        .unwrap();
        std::fs::write(
            &project_path,
            r#"
[ai]
active_model = "project-model"
"#,
        )
        .unwrap();

        let config = CoreConfig::load_from_paths(None, Some(&user_path), Some(&project_path));
        let resolved = config.resolve_provider();

        assert_eq!(config.ai.active_provider.as_deref(), Some("dashscope"));
        assert_eq!(config.ai.active_model.as_deref(), Some("project-model"));
        assert_eq!(resolved.api_key, "sk-user");
        assert_eq!(resolved.model, "project-model");
    }

    #[test]
    fn project_auth_fields_are_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_path = tmp.path().join("project-config.toml");

        std::fs::write(
            &project_path,
            r#"
[ai]
active_provider = "project-provider"
active_model = "project-model"

[ai.providers.project-provider]
type = "dashscope"
api_key = "sk-project"
auth_source = "ecs_ram_role"
"#,
        )
        .unwrap();

        let config = CoreConfig::load_from_paths(None, None, Some(&project_path));
        assert!(config.ai.active_provider.is_none());
        assert!(config.ai.providers.is_empty());
        assert_eq!(config.ai.active_model.as_deref(), Some("project-model"));
    }

    #[test]
    fn user_provider_overrides_system_provider_atomically() {
        let tmp = tempfile::TempDir::new().unwrap();
        let system_path = tmp.path().join("system-config.toml");
        let user_path = tmp.path().join("user-config.toml");

        std::fs::write(
            &system_path,
            r#"
[ai]
active_provider = "shared"

[ai.providers.shared]
type = "dashscope"
base_url = "https://system.example/v1"
api_key = "sk-system"
model = "system-model"
"#,
        )
        .unwrap();
        std::fs::write(
            &user_path,
            r#"
[ai]
active_provider = "shared"

[ai.providers.shared]
type = "openai_compat"
api_key = "sk-user"
"#,
        )
        .unwrap();

        let config = CoreConfig::load_from_paths(Some(&system_path), Some(&user_path), None);
        let provider = config.ai.providers.get("shared").unwrap();

        assert_eq!(provider.provider_type.as_deref(), Some("openai_compat"));
        assert_eq!(provider.api_key.as_deref(), Some("sk-user"));
        assert!(provider.base_url.is_none());
        assert!(provider.model.is_none());
    }

    #[test]
    fn persist_user_config_does_not_write_project_overrides() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_dir = tmp.path().join("home-config");
        let user_path = user_dir.join("config.toml");
        let project_path = tmp.path().join("project-config.toml");
        std::fs::create_dir_all(&user_dir).unwrap();

        std::fs::write(
            &user_path,
            r#"
[ai]
active_provider = "dashscope"
active_model = "user-model"
output_language = "en-US"

[ai.providers.dashscope]
type = "dashscope"
api_key = "sk-user"
"#,
        )
        .unwrap();
        std::fs::write(
            &project_path,
            r#"
[ai]
active_model = "project-model"
output_language = "zh-CN"
"#,
        )
        .unwrap();

        let mut config = CoreConfig::load_from_paths(None, Some(&user_path), Some(&project_path));
        config.ai.active_provider = Some("dashscope".to_string());
        persist_config_to_dir(&config, &user_dir).unwrap();

        let content = std::fs::read_to_string(&user_path).unwrap();
        assert!(content.contains("active_model = \"user-model\""));
        assert!(content.contains("output_language = \"en-US\""));
        assert!(!content.contains("project-model"));
        assert!(!content.contains("zh-CN"));
        assert!(content.contains("api_key = \"sk-user\""));
    }
}
