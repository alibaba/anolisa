use std::collections::HashMap;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

use crate::config::{CoreConfig, ProviderConfig};
use crate::protocol::{
    AuthField, AuthProvider, ControlResponseBody, InputMessage,
    ShellControlRequest,
};

/// Timeout for waiting for auth response from Shell.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

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
                        "获取地址: https://dashscope.console.aliyun.com/apiKey".to_string(),
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
            builtin_base_url: Some(
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            ),
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
    pub values: HashMap<String, String>,
    pub persist: bool,
}

/// Apply auth credentials to the config, rebuilding provider settings.
pub fn apply_auth_credentials(config: &mut CoreConfig, response: &AuthResponse) {
    let template = builtin_auth_providers()
        .into_iter()
        .find(|p| p.id == response.provider_id);

    let (base_url, provider_type, default_model) = match template {
        Some(ref t) => (
            response
                .values
                .get("base_url")
                .cloned()
                .or_else(|| t.builtin_base_url.clone())
                .unwrap_or_default(),
            t.builtin_provider_type.clone(),
            t.builtin_default_model.clone(),
        ),
        None => (
            response
                .values
                .get("base_url")
                .cloned()
                .unwrap_or_default(),
            "generic".to_string(),
            None,
        ),
    };

    let user_model = response.values.get("model").filter(|m| !m.is_empty()).cloned();
    let final_model = user_model.or(default_model);

    let api_key = response
        .values
        .get("api_key")
        .cloned()
        .unwrap_or_default();

    // Aliyun provider uses AK/SK instead of API key
    let access_key_id = response.values.get("access_key_id").cloned();
    let access_key_secret = response.values.get("access_key_secret").cloned();
    let security_token = response.values.get("security_token").cloned();

    config.ai.active_provider = Some(response.provider_id.clone());
    config.ai.providers.insert(
        response.provider_id.clone(),
        ProviderConfig {
            provider_type: Some(provider_type),
            base_url: Some(base_url),
            api_key: Some(api_key),
            model: final_model,
            extra_params: None,
            access_key_id,
            access_key_secret,
            security_token,
        },
    );
}

/// Result of waiting for auth, including any stdin lines consumed during the wait.
pub struct AuthWaitResult {
    pub response: Option<AuthResponse>,
    /// Lines consumed from stdin during auth wait that should be replayed.
    pub buffered_lines: Vec<String>,
}

/// Wait for an auth response from Shell via stdin JSONL.
/// Returns the auth response (if any) and any non-auth messages that were
/// consumed from stdin during the wait (so callers can replay them).
pub async fn wait_for_auth_response<R: AsyncBufReadExt + Unpin>(
    expected_request_id: &str,
    reader: &mut tokio::io::Lines<R>,
) -> AuthWaitResult {
    let mut buffered_lines: Vec<String> = Vec::new();
    let result = tokio::time::timeout(AUTH_TIMEOUT, async {
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let msg: InputMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            match msg {
                InputMessage::ControlResponse { response } => {
                    if response.request_id != expected_request_id {
                        continue;
                    }
                    return parse_auth_response(&response.response);
                }
                InputMessage::ControlRequest { request, .. } => {
                    if matches!(request, ShellControlRequest::Interrupt) {
                        return None;
                    }
                    // Buffer non-interrupt control requests for later
                    buffered_lines.push(line);
                }
                _ => {
                    // Buffer user messages and other lines for later replay
                    buffered_lines.push(line);
                }
            }
        }
        None
    })
    .await;

    match result {
        Ok(response) => AuthWaitResult {
            response,
            buffered_lines,
        },
        Err(_) => {
            tracing::warn!("Auth timeout after {}s", AUTH_TIMEOUT.as_secs());
            AuthWaitResult {
                response: None,
                buffered_lines,
            }
        }
    }
}

/// Parse auth-specific fields from the ControlResponseBody.
fn parse_auth_response(body: &ControlResponseBody) -> Option<AuthResponse> {
    // Check if user denied
    if body.behavior.as_deref() == Some("deny") {
        return None;
    }

    let provider_id = body.provider_id.as_ref()?;
    let values = body.values.clone().unwrap_or_default();

    Some(AuthResponse {
        provider_id: provider_id.clone(),
        values,
        persist: body.persist.unwrap_or(true),
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
            values: HashMap::from([("api_key".to_string(), "sk-test123".to_string())]),
            persist: true,
        };
        apply_auth_credentials(&mut config, &response);

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
            values: HashMap::from([
                ("base_url".to_string(), "https://api.openai.com/v1".to_string()),
                ("api_key".to_string(), "sk-openai".to_string()),
            ]),
            persist: false,
        };
        apply_auth_credentials(&mut config, &response);

        assert_eq!(config.ai.active_provider.as_deref(), Some("openai_compat"));
        let p = config.ai.providers.get("openai_compat").unwrap();
        assert_eq!(p.api_key.as_deref(), Some("sk-openai"));
        assert_eq!(p.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(p.provider_type.as_deref(), Some("openai"));
    }

    #[test]
    fn is_auth_error_detects_401() {
        assert!(is_auth_error("API error 401: invalid api key"));
        assert!(is_auth_error("HTTP 403 Forbidden"));
        assert!(is_auth_error("Unauthorized access"));
        assert!(!is_auth_error("API error 500: internal server error"));
    }

    #[test]
    fn parse_auth_response_deny() {
        let body = ControlResponseBody {
            behavior: Some("deny".to_string()),
            message: None,
            result: None,
            tool_use_id: None,
            updated_permissions: None,
            answer: None,
            selected_options: None,
            provider_id: None,
            values: None,
            persist: None,
        };
        assert!(parse_auth_response(&body).is_none());
    }

    #[test]
    fn parse_auth_response_success() {
        let body = ControlResponseBody {
            behavior: None,
            message: None,
            result: None,
            tool_use_id: None,
            updated_permissions: None,
            answer: None,
            selected_options: None,
            provider_id: Some("dashscope".to_string()),
            values: Some(HashMap::from([("api_key".to_string(), "sk-xxx".to_string())])),
            persist: Some(true),
        };
        let resp = parse_auth_response(&body).unwrap();
        assert_eq!(resp.provider_id, "dashscope");
        assert_eq!(resp.values.get("api_key").unwrap(), "sk-xxx");
        assert!(resp.persist);
    }
}
