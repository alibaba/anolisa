//! Authentication registry commands and credential update rules.

use super::*;

pub(super) fn handle_auth(
    request_id: &str,
    action: &str,
    params: &Value,
    config: &mut CoreConfig,
) -> OutputMessage {
    match action {
        "state" => {
            let templates: Vec<Value> = crate::auth::builtin_auth_providers()
                .into_iter()
                .map(|provider| {
                    serde_json::json!({
                        "id": provider.id,
                        "provider_type": provider.id,
                        "label": provider.label,
                        "fields": provider.fields,
                        "builtin_base_url": provider.builtin_base_url,
                        "builtin_default_model": provider.builtin_default_model,
                    })
                })
                .collect();
            let active_provider = config.ai.active_provider.clone();
            let effective_auth_required = config.resolve_provider().auth_required();
            let saved_providers: Vec<Value> = config
                .ai
                .providers
                .iter()
                .map(|(provider_id, provider)| {
                    let source = if config.user_ai.providers.contains_key(provider_id) {
                        "user"
                    } else if config.system_ai.providers.contains_key(provider_id) {
                        "system"
                    } else {
                        "runtime"
                    };
                    let editable = source == "user";
                    serde_json::json!({
                        "provider_id": provider_id,
                        "provider_type": provider.provider_type,
                        "source": source,
                        "editable": editable,
                        "auth_source": provider.auth_source,
                        "model": provider.model,
                        "base_url": provider.base_url,
                        "api_key_len": provider.api_key.as_ref().map(|v| v.chars().count()).unwrap_or(0),
                        "access_key_id_len": provider.access_key_id.as_ref().map(|v| v.chars().count()).unwrap_or(0),
                        "access_key_secret_len": provider.access_key_secret.as_ref().map(|v| v.chars().count()).unwrap_or(0),
                        "security_token_len": provider.security_token.as_ref().map(|v| v.chars().count()).unwrap_or(0),
                        "active": Some(provider_id) == active_provider.as_ref(),
                        "has_api_key": provider.api_key.as_ref().is_some_and(|v| !v.is_empty()),
                        "has_access_key_id": provider.access_key_id.as_ref().is_some_and(|v| !v.is_empty()),
                        "has_access_key_secret": provider.access_key_secret.as_ref().is_some_and(|v| !v.is_empty()),
                    })
                })
                .collect();
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({
                    "templates": templates,
                    "saved_providers": saved_providers,
                    "active_provider": active_provider,
                    "effective_auth_required": effective_auth_required,
                })),
                error: None,
            }
        }
        "activate" => {
            let provider_id = params
                .get("provider_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if provider_id.is_empty() || !config.ai.providers.contains_key(provider_id) {
                return registry_error(request_id, "provider not found");
            }
            config.ai.active_provider = Some(provider_id.to_string());
            config.user_ai.active_provider = Some(provider_id.to_string());
            if let Some(model) = config
                .ai
                .providers
                .get(provider_id)
                .and_then(|provider| provider.model.clone())
            {
                config.ai.active_model = Some(model);
            }
            if let Err(e) = crate::config::persist_config(config) {
                return registry_error(request_id, &format!("failed to persist config: {e}"));
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "active_provider": provider_id })),
                error: None,
            }
        }
        "delete" => {
            let provider_id = params
                .get("provider_id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if provider_id.is_empty() {
                return registry_error(request_id, "missing provider_id");
            }
            let removed = match crate::auth::remove_auth_provider(config, provider_id) {
                Ok(removed) => removed,
                Err(error) => return registry_error(request_id, &error.to_string()),
            };
            if let Err(error) = crate::config::persist_config(config) {
                return registry_error(request_id, &format!("failed to persist config: {error}"));
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({
                    "deleted_provider": provider_id,
                    "active_provider": removed.active_provider,
                })),
                error: None,
            }
        }
        "prepare" => {
            let provider_type = params
                .get("provider_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if provider_type.is_empty() {
                return registry_error(request_id, "missing provider_type");
            }
            let data = if provider_type == "aliyun" {
                match crate::provider::sysom::detect_ecs_auth_challenge() {
                    Some(challenge) => serde_json::json!({
                        "mode": "ecs_ram_role",
                        "instance_id": challenge.instance_id,
                        "console_url": challenge.console_url,
                        "values": {
                            "auth_source": "ecs_ram_role"
                        }
                    }),
                    None => serde_json::json!({
                        "mode": "manual"
                    }),
                }
            } else {
                serde_json::json!({
                    "mode": "manual"
                })
            };
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(data),
                error: None,
            }
        }
        "verify" => {
            let provider_type = params
                .get("provider_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let auth_source = params
                .get("auth_source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if provider_type == "aliyun" && auth_source == "ecs_ram_role" {
                let authorized = crate::provider::sysom::ecs_ram_role_credentials_available();
                OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: true,
                    data: Some(serde_json::json!({
                        "authorized": authorized
                    })),
                    error: None,
                }
            } else {
                OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: true,
                    data: Some(serde_json::json!({
                        "authorized": true
                    })),
                    error: None,
                }
            }
        }
        "configure" => {
            let provider_id = params
                .get("provider_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let provider_type = params
                .get("provider_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if provider_id.is_empty() || provider_type.is_empty() {
                return registry_error(request_id, "missing provider_id or provider_type");
            }
            if !is_valid_provider_id(provider_id) {
                return registry_error(request_id, "invalid provider_id");
            }
            if config.ai.providers.contains_key(provider_id)
                && !config.user_ai.providers.contains_key(provider_id)
            {
                return registry_error(request_id, "provider is not editable");
            }
            let mut values: std::collections::HashMap<String, String> = params
                .get("values")
                .and_then(|v| v.as_object())
                .map(|object| {
                    object
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|s| (key.clone(), s.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(existing) = config.ai.providers.get(provider_id) {
                preserve_masked_secret(&mut values, "api_key", existing.api_key.as_deref());
                preserve_masked_secret(
                    &mut values,
                    "access_key_id",
                    existing.access_key_id.as_deref(),
                );
                preserve_masked_secret(
                    &mut values,
                    "access_key_secret",
                    existing.access_key_secret.as_deref(),
                );
                preserve_masked_secret(
                    &mut values,
                    "security_token",
                    existing.security_token.as_deref(),
                );
            }
            let response = crate::auth::AuthResponse {
                provider_id: provider_id.to_string(),
                provider_type: Some(provider_type.to_string()),
                values,
                persist: true,
            };
            if let Err(error) = crate::auth::apply_auth_credentials(config, &response) {
                return registry_error(request_id, &error.to_string());
            }
            if let Err(e) = crate::config::persist_config(config) {
                return registry_error(request_id, &format!("failed to persist config: {e}"));
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "provider_id": provider_id })),
                error: None,
            }
        }
        _ => registry_error(
            request_id,
            &format!("unsupported action for auth: {action}"),
        ),
    }
}

fn is_valid_provider_id(provider_id: &str) -> bool {
    !provider_id.is_empty()
        && provider_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn preserve_masked_secret(
    values: &mut std::collections::HashMap<String, String>,
    key: &str,
    existing: Option<&str>,
) {
    let Some(value) = values.get(key) else {
        return;
    };
    if !value.is_empty() && value.chars().all(|ch| ch == '•') {
        if let Some(existing) = existing {
            values.insert(key.to_string(), existing.to_string());
        }
    }
}
