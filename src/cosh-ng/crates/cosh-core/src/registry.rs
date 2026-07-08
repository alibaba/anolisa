use std::io::{self, BufRead, Write};

use serde_json::Value;

use crate::cli::CliArgs;
use crate::config::CoreConfig;
use crate::extension::config::flatten_hook_groups;
use crate::extension::ExtensionManager;
use crate::protocol::{InputMessage, OutputMessage};
use crate::skill::manager::expand_path;
use crate::skill::SkillManager;

pub async fn run(_args: &CliArgs, mut config: CoreConfig) {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());

    // --- Extension Manager setup (no LLM/provider init) ---
    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut ext_manager = ExtensionManager::new(project_root.clone());
    ext_manager.refresh();

    // --- Skill Manager setup ---
    let custom_paths: Vec<std::path::PathBuf> = config
        .skills
        .custom_paths
        .iter()
        .filter_map(|p| expand_path(p))
        .collect();
    let skill_manager = SkillManager::new(project_root, custom_paths, ext_manager.skill_dirs());
    skill_manager.refresh().await;

    // Read one line from stdin
    let line = {
        let mut buf = String::new();
        match stdin.lock().read_line(&mut buf) {
            Ok(0) => return, // EOF
            Ok(_) => buf,
            Err(_) => return,
        }
    };

    let msg: InputMessage = match serde_json::from_str(line.trim()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("failed to parse input: {e}");
            return;
        }
    };

    match msg {
        InputMessage::RegistryRequest {
            request_id,
            domain,
            action,
            params,
        } => {
            let response = handle_registry_request(
                &request_id,
                &domain,
                &action,
                &params,
                &mut config,
                &ext_manager,
                &skill_manager,
            )
            .await;
            emit(&mut writer, &response);
        }
        _ => {
            tracing::debug!("expected registry_request, got other message type");
        }
    }
}

async fn handle_registry_request(
    request_id: &str,
    domain: &str,
    action: &str,
    params: &Value,
    config: &mut CoreConfig,
    ext_manager: &ExtensionManager,
    skill_manager: &SkillManager,
) -> OutputMessage {
    match domain {
        "auth" => handle_auth(request_id, action, params, config),
        "extensions" => handle_extensions(request_id, action, params, ext_manager),
        "skills" => handle_skills(request_id, action, params, skill_manager).await,
        "hooks" => handle_hooks(request_id, action, params, ext_manager),
        _ => OutputMessage::RegistryResponse {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(format!("unknown domain: {domain}")),
        },
    }
}

fn handle_auth(
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
            crate::auth::apply_auth_credentials(config, &response);
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

fn registry_error(request_id: &str, error: &str) -> OutputMessage {
    OutputMessage::RegistryResponse {
        request_id: request_id.to_string(),
        success: false,
        data: None,
        error: Some(error.to_string()),
    }
}

fn handle_extensions(
    request_id: &str,
    action: &str,
    params: &Value,
    ext_manager: &ExtensionManager,
) -> OutputMessage {
    match action {
        "list" => {
            let extensions: Vec<Value> = ext_manager
                .list()
                .iter()
                .map(|ext| {
                    serde_json::json!({
                        "name": ext.name,
                        "version": ext.version,
                        "is_active": ext.is_active,
                        "path": ext.path.to_string_lossy(),
                    })
                })
                .collect();
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(Value::Array(extensions)),
                error: None,
            }
        }
        "detail" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match ext_manager.list().iter().find(|e| e.name == name) {
                Some(ext) => {
                    let detail = serde_json::json!({
                        "name": ext.name,
                        "version": ext.version,
                        "is_active": ext.is_active,
                        "path": ext.path.to_string_lossy(),
                        "has_hooks": !ext.config.hooks.is_empty(),
                        "skill_dirs": ext.config.skills.0,
                    });
                    OutputMessage::RegistryResponse {
                        request_id: request_id.to_string(),
                        success: true,
                        data: Some(detail),
                        error: None,
                    }
                }
                None => OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("extension not found: {name}")),
                },
            }
        }
        "enable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate extension exists
            if !ext_manager.list().iter().any(|e| e.name == name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("extension not found: {name}")),
                };
            }
            // Remove extension from disabled list
            if let Err(e) = crate::state::remove_disabled(crate::state::EXTENSIONS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to enable extension: {e}")),
                };
            }
            // Cleanup: remove extension's hooks from hooks.json disabled list
            let hook_names = ext_manager.extension_hook_names(name);
            if !hook_names.is_empty() {
                let _ = crate::state::remove_disabled_set(crate::state::HOOKS_STATE, &hook_names);
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "enabled": name })),
                error: None,
            }
        }
        "disable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate extension exists
            if !ext_manager.list().iter().any(|e| e.name == name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("extension not found: {name}")),
                };
            }
            if let Err(e) = crate::state::add_disabled(crate::state::EXTENSIONS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to disable extension: {e}")),
                };
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "disabled": name })),
                error: None,
            }
        }
        _ => OutputMessage::RegistryResponse {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(format!("unsupported action for extensions: {action}")),
        },
    }
}

async fn handle_skills(
    request_id: &str,
    action: &str,
    params: &Value,
    skill_manager: &SkillManager,
) -> OutputMessage {
    match action {
        "list" => {
            let disabled = crate::state::load_disabled(crate::state::SKILLS_STATE);
            let skills: Vec<Value> = skill_manager
                .list()
                .await
                .iter()
                .map(|s| {
                    let is_disabled = disabled.contains(&s.name);
                    serde_json::json!({
                        "name": s.name,
                        "description": s.description,
                        "level": s.level.to_string(),
                        "disabled": is_disabled,
                    })
                })
                .collect();
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(Value::Array(skills)),
                error: None,
            }
        }
        "detail" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match skill_manager.load(name).await {
                Some(skill) => {
                    let disabled = crate::state::load_disabled(crate::state::SKILLS_STATE);
                    let is_disabled = disabled.contains(&skill.name);
                    let detail = serde_json::json!({
                        "name": skill.name,
                        "description": skill.description,
                        "level": skill.level.to_string(),
                        "base_dir": skill.base_dir.to_string_lossy(),
                        "disabled": is_disabled,
                    });
                    OutputMessage::RegistryResponse {
                        request_id: request_id.to_string(),
                        success: true,
                        data: Some(detail),
                        error: None,
                    }
                }
                None => OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("skill not found: {name}")),
                },
            }
        }
        "enable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate skill exists
            if skill_manager.load(name).await.is_none() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("skill not found: {name}")),
                };
            }
            if let Err(e) = crate::state::remove_disabled(crate::state::SKILLS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to enable skill: {e}")),
                };
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "enabled": name })),
                error: None,
            }
        }
        "disable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate skill exists
            if skill_manager.load(name).await.is_none() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("skill not found: {name}")),
                };
            }
            if let Err(e) = crate::state::add_disabled(crate::state::SKILLS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to disable skill: {e}")),
                };
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "disabled": name })),
                error: None,
            }
        }
        _ => OutputMessage::RegistryResponse {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(format!("unsupported action for skills: {action}")),
        },
    }
}

fn handle_hooks(
    request_id: &str,
    action: &str,
    params: &Value,
    ext_manager: &ExtensionManager,
) -> OutputMessage {
    match action {
        "list" => {
            let disabled = crate::state::load_disabled(crate::state::HOOKS_STATE);
            let mut hooks_list: Vec<Value> = Vec::new();
            for ext in ext_manager.list() {
                if !ext.is_active || ext.config.hooks.is_empty() {
                    continue;
                }
                // Collect all hook events for this extension
                let events = [
                    ("PreToolUse", &ext.config.hooks.pre_tool_use),
                    ("PostToolUse", &ext.config.hooks.post_tool_use),
                    (
                        "PostToolUseFailure",
                        &ext.config.hooks.post_tool_use_failure,
                    ),
                    ("UserPromptSubmit", &ext.config.hooks.user_prompt_submit),
                    ("SessionStart", &ext.config.hooks.session_start),
                    ("Stop", &ext.config.hooks.stop),
                    ("BeforeModel", &ext.config.hooks.before_model),
                    ("AfterModel", &ext.config.hooks.after_model),
                ];
                for (event_name, groups) in events {
                    for hook_def in flatten_hook_groups(groups) {
                        let name = hook_def.name.as_deref().unwrap_or(&hook_def.command);
                        let is_disabled = disabled.contains(name);
                        hooks_list.push(serde_json::json!({
                            "name": name,
                            "event": event_name,
                            "extension": ext.name,
                            "command": hook_def.command,
                            "matcher": hook_def.matcher,
                            "disabled": is_disabled,
                        }));
                    }
                }
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(Value::Array(hooks_list)),
                error: None,
            }
        }
        "enable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate hook exists in known extensions
            let known = collect_all_hook_names(ext_manager);
            if !known.contains(name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("unknown hook: {name}")),
                };
            }
            if let Err(e) = crate::state::remove_disabled(crate::state::HOOKS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to enable hook: {e}")),
                };
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "enabled": name })),
                error: None,
            }
        }
        "disable" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some("missing 'name' parameter".to_string()),
                };
            }
            // Validate hook exists in known extensions
            let known = collect_all_hook_names(ext_manager);
            if !known.contains(name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("unknown hook: {name}")),
                };
            }
            if let Err(e) = crate::state::add_disabled(crate::state::HOOKS_STATE, name) {
                return OutputMessage::RegistryResponse {
                    request_id: request_id.to_string(),
                    success: false,
                    data: None,
                    error: Some(format!("failed to disable hook: {e}")),
                };
            }
            OutputMessage::RegistryResponse {
                request_id: request_id.to_string(),
                success: true,
                data: Some(serde_json::json!({ "disabled": name })),
                error: None,
            }
        }
        _ => OutputMessage::RegistryResponse {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(format!("unsupported action for hooks: {action}")),
        },
    }
}

fn collect_all_hook_names(ext_manager: &ExtensionManager) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for ext in ext_manager.list() {
        let events = [
            &ext.config.hooks.pre_tool_use,
            &ext.config.hooks.post_tool_use,
            &ext.config.hooks.post_tool_use_failure,
            &ext.config.hooks.user_prompt_submit,
            &ext.config.hooks.session_start,
            &ext.config.hooks.stop,
            &ext.config.hooks.before_model,
            &ext.config.hooks.after_model,
        ];
        for groups in events {
            for def in flatten_hook_groups(groups) {
                if let Some(name) = def.name {
                    names.insert(name);
                }
            }
        }
    }
    names
}

fn emit<W: Write>(writer: &mut W, msg: &OutputMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, ProviderConfig};

    #[test]
    fn auth_state_marks_system_providers_as_not_editable() {
        let mut config = CoreConfig::default();
        config.ai.active_provider = Some("system-provider".to_string());
        config.ai.providers.insert(
            "system-provider".to_string(),
            ProviderConfig {
                provider_type: Some("dashscope".to_string()),
                api_key: Some("sk-system".to_string()),
                model: Some("system-model".to_string()),
                ..Default::default()
            },
        );
        config.system_ai = AiConfig {
            providers: config.ai.providers.clone(),
            ..Default::default()
        };
        config.user_ai.providers.insert(
            "user-provider".to_string(),
            ProviderConfig {
                provider_type: Some("dashscope".to_string()),
                api_key: Some("sk-user".to_string()),
                ..Default::default()
            },
        );
        config.ai.providers.extend(config.user_ai.providers.clone());

        let response = handle_auth("test-1", "state", &Value::Null, &mut config);
        let OutputMessage::RegistryResponse {
            success: true,
            data: Some(data),
            ..
        } = response
        else {
            panic!("unexpected response: {response:?}");
        };
        let saved = data["saved_providers"].as_array().unwrap();
        let system = saved
            .iter()
            .find(|provider| provider["provider_id"] == "system-provider")
            .unwrap();
        let user = saved
            .iter()
            .find(|provider| provider["provider_id"] == "user-provider")
            .unwrap();

        assert_eq!(system["source"], "system");
        assert_eq!(system["editable"], false);
        assert_eq!(user["source"], "user");
        assert_eq!(user["editable"], true);
    }
}
