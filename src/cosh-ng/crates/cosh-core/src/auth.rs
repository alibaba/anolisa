use std::collections::HashMap;
use std::fmt;

use crate::config::{CoreConfig, ProviderConfig};
use crate::protocol::{AuthField, AuthProvider};

mod exchange;
mod validation;

pub(crate) use exchange::request_validated_auth;
pub(crate) use validation::AuthConfigValidationError;
use validation::{validate_auth_response, validate_base_url};

/// Returns the builtin provider templates for the auth UI.
pub fn builtin_auth_providers() -> Vec<AuthProvider> {
    vec![
        AuthProvider {
            id: "dashscope".to_string(),
            label: "DashScope (百炼)".to_string(),
            fields: vec![
                AuthField {
                    name: "api_key".to_string(),
                    label: "API Key".to_string(),
                    hint: Some(
                        "获取地址: https://bailian.console.aliyun.com/?tab=model#/api-key"
                            .to_string(),
                    ),
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthField {
                    name: "model".to_string(),
                    label: "Model".to_string(),
                    hint: Some("默认: qwen3.7-plus, e.g. qwen3.7-max, deepseek-v4-pro".to_string()),
                    secret: false,
                    required: false,
                    placeholder: Some("qwen3.7-plus".to_string()),
                },
            ],
            builtin_base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
            builtin_provider_type: "dashscope".to_string(),
            builtin_default_model: Some("qwen3.7-plus".to_string()),
        },
        AuthProvider {
            id: "openai_compat".to_string(),
            label: "OpenAI Compatible".to_string(),
            fields: vec![
                AuthField {
                    name: "base_url".to_string(),
                    label: "Base URL".to_string(),
                    hint: Some("例如: https://api.openai.com/v1".to_string()),
                    secret: false,
                    required: true,
                    placeholder: Some("https://api.openai.com/v1".to_string()),
                },
                AuthField {
                    name: "api_key".to_string(),
                    label: "API Key".to_string(),
                    hint: Some("sk-...".to_string()),
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthField {
                    name: "model".to_string(),
                    label: "Model".to_string(),
                    hint: Some("e.g. qwen3.7-max, deepseek-v4-pro".to_string()),
                    secret: false,
                    required: true,
                    placeholder: None,
                },
            ],
            builtin_base_url: None,
            builtin_provider_type: "openai".to_string(),
            builtin_default_model: None,
        },
        AuthProvider {
            id: "aliyun".to_string(),
            label: "Aliyun Authentication".to_string(),
            fields: vec![
                AuthField {
                    name: "access_key_id".to_string(),
                    label: "Access Key ID".to_string(),
                    hint: Some("获取地址: https://ram.console.aliyun.com/manage/ak".to_string()),
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthField {
                    name: "access_key_secret".to_string(),
                    label: "Access Key Secret".to_string(),
                    hint: None,
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthField {
                    name: "model".to_string(),
                    label: "Model".to_string(),
                    hint: Some("默认: qwen3.7-plus".to_string()),
                    secret: false,
                    required: false,
                    placeholder: Some("qwen3.7-plus".to_string()),
                },
            ],
            builtin_base_url: None,
            builtin_provider_type: "aliyun".to_string(),
            builtin_default_model: Some("qwen3.7-plus".to_string()),
        },
    ]
}

/// Response from the auth flow.
pub struct AuthResponse {
    pub provider_id: String,
    pub provider_type: Option<String>,
    pub values: HashMap<String, String>,
    pub persist: bool,
}

/// Applies validated auth credentials and rebuilds provider settings.
///
/// # Errors
///
/// Returns an error when a required credential field is missing or a supplied
/// base URL is malformed.
pub(crate) fn apply_auth_credentials(
    config: &mut CoreConfig,
    response: &AuthResponse,
) -> Result<(), AuthConfigValidationError> {
    let template_id = response
        .provider_type
        .as_deref()
        .unwrap_or(response.provider_id.as_str());
    let template = builtin_auth_providers()
        .into_iter()
        .find(|provider| provider.id == template_id);
    match template.as_ref() {
        Some(template) => validate_auth_response(template, response)?,
        None => {
            let base_url = response
                .values
                .get("base_url")
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AuthConfigValidationError::MissingRequiredField("base_url".to_string())
                })?;
            validate_base_url(base_url)?;
        }
    }

    let (base_url, provider_type, default_model) = match template {
        Some(template) => (
            response
                .values
                .get("base_url")
                .cloned()
                .or(template.builtin_base_url)
                .unwrap_or_default(),
            template.builtin_provider_type,
            template.builtin_default_model,
        ),
        None => (
            response.values.get("base_url").cloned().unwrap_or_default(),
            "generic".to_string(),
            None,
        ),
    };

    let user_model = response
        .values
        .get("model")
        .filter(|m| !m.is_empty())
        .cloned();
    let final_model = user_model.or(default_model);

    let api_key = response.values.get("api_key").cloned().unwrap_or_default();

    // Aliyun provider uses AK/SK instead of API key
    let auth_source = response.values.get("auth_source").cloned();
    let is_ecs_ram_role = auth_source.as_deref() == Some("ecs_ram_role");
    let access_key_id = if is_ecs_ram_role {
        None
    } else {
        response.values.get("access_key_id").cloned()
    };
    let access_key_secret = if is_ecs_ram_role {
        None
    } else {
        response.values.get("access_key_secret").cloned()
    };
    let security_token = if is_ecs_ram_role {
        None
    } else {
        response.values.get("security_token").cloned()
    };

    config.ai.active_provider = Some(response.provider_id.clone());
    let provider = ProviderConfig {
        provider_type: Some(provider_type),
        auth_source,
        base_url: Some(base_url),
        api_key: Some(api_key),
        model: final_model,
        extra_params: None,
        access_key_id,
        access_key_secret,
        security_token,
    };
    config
        .ai
        .providers
        .insert(response.provider_id.clone(), provider.clone());
    if response.persist {
        config.user_ai.active_provider = Some(response.provider_id.clone());
        config
            .user_ai
            .providers
            .insert(response.provider_id.clone(), provider);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoveAuthProviderError {
    NotFound,
    NotEditable,
}

impl fmt::Display for RemoveAuthProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("provider not found"),
            Self::NotEditable => formatter.write_str("provider is not removable"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemovedAuthProvider {
    pub(crate) active_provider: Option<String>,
}

pub(crate) fn remove_auth_provider(
    config: &mut CoreConfig,
    provider_id: &str,
) -> Result<RemovedAuthProvider, RemoveAuthProviderError> {
    if !config.user_ai.providers.contains_key(provider_id) {
        return Err(if config.ai.providers.contains_key(provider_id) {
            RemoveAuthProviderError::NotEditable
        } else {
            RemoveAuthProviderError::NotFound
        });
    }

    let was_active = config.ai.active_provider.as_deref() == Some(provider_id);
    config.user_ai.providers.remove(provider_id);
    if config.user_ai.active_provider.as_deref() == Some(provider_id) {
        config.user_ai.active_provider = None;
    }

    if let Some(system_provider) = config.system_ai.providers.get(provider_id).cloned() {
        config
            .ai
            .providers
            .insert(provider_id.to_string(), system_provider);
    } else {
        config.ai.providers.remove(provider_id);
    }

    if was_active {
        let active_provider = config
            .system_ai
            .active_provider
            .clone()
            .filter(|fallback| config.ai.providers.contains_key(fallback));
        config.ai.active_provider = active_provider.clone();
        config.ai.active_model = active_provider.as_ref().and_then(|fallback| {
            config
                .ai
                .providers
                .get(fallback)
                .and_then(|provider| provider.model.clone())
        });
    }

    Ok(RemovedAuthProvider {
        active_provider: config.ai.active_provider.clone(),
    })
}

/// Check if an error string indicates an auth failure (401/403).
pub fn is_auth_error(error: &str) -> bool {
    error.contains("401") || error.contains("403") || error.contains("Unauthorized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_providers_have_correct_ids() {
        let providers = builtin_auth_providers();
        assert_eq!(providers.len(), 3);
        assert_eq!(providers[0].id, "dashscope");
        assert_eq!(providers[1].id, "openai_compat");
        assert_eq!(providers[2].id, "aliyun");
    }

    #[test]
    fn dashscope_has_builtin_base_url() {
        let providers = builtin_auth_providers();
        let ds = &providers[0];
        assert!(ds.builtin_base_url.is_some());
        assert_eq!(ds.fields.len(), 2);
        assert_eq!(ds.fields[0].name, "api_key");
        assert_eq!(ds.fields[1].name, "model");
    }

    #[test]
    fn openai_compat_has_no_builtin_base_url() {
        let providers = builtin_auth_providers();
        let oc = &providers[1];
        assert!(oc.builtin_base_url.is_none());
        assert_eq!(oc.fields.len(), 3);
        assert_eq!(oc.fields[0].name, "base_url");
        assert_eq!(oc.fields[1].name, "api_key");
        assert_eq!(oc.fields[2].name, "model");
    }

    #[test]
    fn apply_dashscope_credentials() {
        let mut config = CoreConfig::default();
        let response = AuthResponse {
            provider_id: "dashscope".to_string(),
            provider_type: None,
            values: HashMap::from([("api_key".to_string(), "sk-test123".to_string())]),
            persist: true,
        };
        apply_auth_credentials(&mut config, &response).unwrap();

        assert_eq!(config.ai.active_provider.as_deref(), Some("dashscope"));
        let p = config.ai.providers.get("dashscope").unwrap();
        assert_eq!(p.api_key.as_deref(), Some("sk-test123"));
        assert_eq!(
            p.base_url.as_deref(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
        assert_eq!(p.provider_type.as_deref(), Some("dashscope"));
        assert_eq!(p.model.as_deref(), Some("qwen3.7-plus"));
    }

    #[test]
    fn apply_openai_compat_credentials() {
        let mut config = CoreConfig::default();
        let response = AuthResponse {
            provider_id: "openai_compat".to_string(),
            provider_type: None,
            values: HashMap::from([
                (
                    "base_url".to_string(),
                    "https://api.openai.com/v1".to_string(),
                ),
                ("api_key".to_string(), "sk-openai".to_string()),
                ("model".to_string(), "gpt-4o".to_string()),
            ]),
            persist: false,
        };
        apply_auth_credentials(&mut config, &response).unwrap();

        assert_eq!(config.ai.active_provider.as_deref(), Some("openai_compat"));
        let p = config.ai.providers.get("openai_compat").unwrap();
        assert_eq!(p.api_key.as_deref(), Some("sk-openai"));
        assert_eq!(p.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(p.provider_type.as_deref(), Some("openai"));
    }

    #[test]
    fn apply_credentials_uses_provider_id_as_config_key_and_type_as_template() {
        let mut config = CoreConfig::default();
        let response = AuthResponse {
            provider_id: "qwen-prod".to_string(),
            provider_type: Some("dashscope".to_string()),
            values: HashMap::from([("api_key".to_string(), "sk-prod".to_string())]),
            persist: true,
        };

        apply_auth_credentials(&mut config, &response).unwrap();

        assert_eq!(config.ai.active_provider.as_deref(), Some("qwen-prod"));
        assert!(config.ai.providers.contains_key("qwen-prod"));
        assert!(!config.ai.providers.contains_key("dashscope"));
        let provider = config.ai.providers.get("qwen-prod").unwrap();
        assert_eq!(provider.provider_type.as_deref(), Some("dashscope"));
        assert_eq!(provider.api_key.as_deref(), Some("sk-prod"));
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
    }

    #[test]
    fn apply_unknown_provider_preserves_generic_fallback() {
        let mut config = CoreConfig::default();
        let response = AuthResponse {
            provider_id: "custom-provider".to_string(),
            provider_type: Some("custom-provider".to_string()),
            values: HashMap::from([
                (
                    "base_url".to_string(),
                    "https://api.example.com/v1".to_string(),
                ),
                ("api_key".to_string(), "sk-custom".to_string()),
                ("model".to_string(), "custom-model".to_string()),
            ]),
            persist: true,
        };

        apply_auth_credentials(&mut config, &response).unwrap();

        let provider = config.ai.providers.get("custom-provider").unwrap();
        assert_eq!(provider.provider_type.as_deref(), Some("generic"));
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(provider.model.as_deref(), Some("custom-model"));
    }

    #[test]
    fn apply_unknown_provider_rejects_invalid_base_url() {
        let mut config = CoreConfig::default();
        let response = AuthResponse {
            provider_id: "custom-provider".to_string(),
            provider_type: Some("custom-provider".to_string()),
            values: HashMap::from([(
                "base_url".to_string(),
                "file:///tmp/custom-provider".to_string(),
            )]),
            persist: true,
        };

        assert_eq!(
            apply_auth_credentials(&mut config, &response),
            Err(AuthConfigValidationError::InvalidBaseUrl)
        );
        assert!(config.ai.providers.is_empty());
        assert!(config.user_ai.providers.is_empty());
    }

    #[test]
    fn apply_unknown_provider_requires_base_url() {
        for values in [
            HashMap::new(),
            HashMap::from([("base_url".to_string(), " ".to_string())]),
        ] {
            let mut config = CoreConfig::default();
            let response = AuthResponse {
                provider_id: "custom-provider".to_string(),
                provider_type: Some("custom-provider".to_string()),
                values,
                persist: true,
            };

            assert_eq!(
                apply_auth_credentials(&mut config, &response),
                Err(AuthConfigValidationError::MissingRequiredField(
                    "base_url".to_string()
                ))
            );
            assert!(config.ai.providers.is_empty());
            assert!(config.user_ai.providers.is_empty());
        }
    }

    #[test]
    fn persisted_auth_credentials_update_user_layer_only_for_persistence() {
        let mut config = CoreConfig::default();
        config.ai.providers.insert(
            "system-provider".to_string(),
            ProviderConfig {
                provider_type: Some("dashscope".to_string()),
                api_key: Some("sk-system".to_string()),
                ..Default::default()
            },
        );
        let response = AuthResponse {
            provider_id: "user-provider".to_string(),
            provider_type: Some("dashscope".to_string()),
            values: HashMap::from([("api_key".to_string(), "sk-user".to_string())]),
            persist: true,
        };

        apply_auth_credentials(&mut config, &response).unwrap();

        assert!(config.ai.providers.contains_key("system-provider"));
        assert!(config.ai.providers.contains_key("user-provider"));
        assert!(!config.user_ai.providers.contains_key("system-provider"));
        assert_eq!(
            config.user_ai.active_provider.as_deref(),
            Some("user-provider")
        );
        assert_eq!(
            config
                .user_ai
                .providers
                .get("user-provider")
                .and_then(|provider| provider.api_key.as_deref()),
            Some("sk-user")
        );
    }

    #[test]
    fn remove_auth_provider_drops_user_credentials_and_active_selection() {
        let provider = ProviderConfig {
            provider_type: Some("dashscope".to_string()),
            api_key: Some("sk-user".to_string()),
            model: Some("user-model".to_string()),
            ..Default::default()
        };
        let mut config = CoreConfig::default();
        config.ai.active_provider = Some("user-provider".to_string());
        config.ai.active_model = Some("user-model".to_string());
        config
            .ai
            .providers
            .insert("user-provider".to_string(), provider.clone());
        config.user_ai.active_provider = Some("user-provider".to_string());
        config
            .user_ai
            .providers
            .insert("user-provider".to_string(), provider);

        let removed = remove_auth_provider(&mut config, "user-provider").unwrap();

        assert_eq!(removed.active_provider, None);
        assert_eq!(config.ai.active_provider, None);
        assert_eq!(config.ai.active_model, None);
        assert!(!config.ai.providers.contains_key("user-provider"));
        assert!(!config.user_ai.providers.contains_key("user-provider"));
    }

    #[test]
    fn remove_auth_provider_reveals_system_provider_with_same_id() {
        let system_provider = ProviderConfig {
            provider_type: Some("dashscope".to_string()),
            api_key: Some("sk-system".to_string()),
            model: Some("system-model".to_string()),
            ..Default::default()
        };
        let user_provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-user".to_string()),
            model: Some("user-model".to_string()),
            ..Default::default()
        };
        let mut config = CoreConfig::default();
        config.ai.active_provider = Some("shared".to_string());
        config
            .ai
            .providers
            .insert("shared".to_string(), user_provider.clone());
        config
            .system_ai
            .providers
            .insert("shared".to_string(), system_provider);
        config.system_ai.active_provider = Some("shared".to_string());
        config.user_ai.active_provider = Some("shared".to_string());
        config
            .user_ai
            .providers
            .insert("shared".to_string(), user_provider);

        let removed = remove_auth_provider(&mut config, "shared").unwrap();

        assert_eq!(removed.active_provider.as_deref(), Some("shared"));
        assert_eq!(
            config
                .ai
                .providers
                .get("shared")
                .and_then(|provider| provider.api_key.as_deref()),
            Some("sk-system")
        );
        assert_eq!(config.ai.active_model.as_deref(), Some("system-model"));
        assert_eq!(config.user_ai.active_provider, None);
    }

    #[test]
    fn remove_auth_provider_rejects_system_provider() {
        let mut config = CoreConfig::default();
        config
            .ai
            .providers
            .insert("system-provider".to_string(), ProviderConfig::default());

        assert_eq!(
            remove_auth_provider(&mut config, "system-provider"),
            Err(RemoveAuthProviderError::NotEditable)
        );
    }

    #[test]
    fn is_auth_error_detects_401() {
        assert!(is_auth_error("API error 401: invalid api key"));
        assert!(is_auth_error("HTTP 403 Forbidden"));
        assert!(is_auth_error("Unauthorized access"));
        assert!(!is_auth_error("API error 500: internal server error"));
    }
}
