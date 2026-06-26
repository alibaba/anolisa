use std::io::{self, BufRead, Write};

use serde_json::Value;

use crate::cli::CliArgs;
use crate::config::CoreConfig;
use crate::extension::config::flatten_hook_groups;
use crate::extension::ExtensionManager;
use crate::protocol::{InputMessage, OutputMessage};
use crate::skill::manager::expand_path;
use crate::skill::SkillManager;

pub async fn run(_args: &CliArgs, config: CoreConfig) {
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
            let response =
                handle_registry_request(&request_id, &domain, &action, &params, &ext_manager, &skill_manager).await;
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
    ext_manager: &ExtensionManager,
    skill_manager: &SkillManager,
) -> OutputMessage {
    match domain {
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
                    ("PostToolUseFailure", &ext.config.hooks.post_tool_use_failure),
                    ("UserPromptSubmit", &ext.config.hooks.user_prompt_submit),
                    ("SessionStart", &ext.config.hooks.session_start),
                    ("Stop", &ext.config.hooks.stop),
                    ("BeforeModel", &ext.config.hooks.before_model),
                    ("AfterModel", &ext.config.hooks.after_model),
                ];
                for (event_name, groups) in events {
                    for hook_def in flatten_hook_groups(groups) {
                        let name = hook_def
                            .name
                            .as_deref()
                            .unwrap_or(&hook_def.command);
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
