use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CoreConfig {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub session: SessionConfig,
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub extra_params: Option<Value>,
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

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot-shell")
}

fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!("{}{}{}", &result[..start], replacement, &result[start + end + 1..]);
        } else {
            break;
        }
    }
    result
}

impl CoreConfig {
    pub fn load() -> Self {
        crate::migrate::try_migrate();

        let candidates = [
            std::env::current_dir()
                .ok()
                .map(|p| p.join(".copilot-shell/config.toml")),
            Some(config_dir().join("config.toml")),
            Some(PathBuf::from("/etc/copilot-shell/config.toml")),
        ];

        for candidate in candidates.iter().flatten() {
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(candidate) {
                    match toml::from_str::<CoreConfig>(&content) {
                        Ok(mut config) => {
                            config.apply_env_overrides();
                            return config;
                        }
                        Err(e) => {
                            eprintln!(
                                "[cosh-core] Warning: failed to parse {}: {}",
                                candidate.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        let mut config = CoreConfig::default();
        config.apply_env_overrides();
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
            .unwrap_or_else(|| {
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
            });

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

        ResolvedProvider {
            base_url,
            api_key,
            model,
            provider_type,
            extra_params,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub provider_type: String,
    pub extra_params: Option<Value>,
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
}
