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
            eprintln!("[cosh-core/registry] Failed to parse input: {e}");
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
            eprintln!("[cosh-core/registry] Expected registry_request, got other message type");
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
        "hooks" => handle_hooks(request_id, action, ext_manager),
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
            let skills: Vec<Value> = skill_manager
                .list()
                .await
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "description": s.description,
                        "level": s.level.to_string(),
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
                    let detail = serde_json::json!({
                        "name": skill.name,
                        "description": skill.description,
                        "level": skill.level.to_string(),
                        "base_dir": skill.base_dir.to_string_lossy(),
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
    ext_manager: &ExtensionManager,
) -> OutputMessage {
    match action {
        "list" => {
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
                        hooks_list.push(serde_json::json!({
                            "name": name,
                            "event": event_name,
                            "extension": ext.name,
                            "command": hook_def.command,
                            "matcher": hook_def.matcher,
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
        _ => OutputMessage::RegistryResponse {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(format!("unsupported action for hooks: {action}")),
        },
    }
}

fn emit<W: Write>(writer: &mut W, msg: &OutputMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
    }
}
