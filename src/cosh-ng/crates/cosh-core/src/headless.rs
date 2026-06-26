use std::io;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::auth::{apply_auth_credentials, builtin_auth_providers, wait_for_auth_response};
use crate::cli::CliArgs;
use crate::config::{self, CoreConfig};
use crate::core::CoshCore;
use crate::extension::ExtensionManager;
use crate::protocol::{AuthReason, InputMessage, OutputMessage, ShellControlRequest};
use crate::skill::manager::expand_path;
use crate::skill::SkillManager;
use crate::tool::ToolRegistry;

pub async fn run(args: &CliArgs, mut config: CoreConfig) {
    apply_cli_overrides(args, &mut config);

    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut lines = stdin.lines();

    // --- Auth check: if no API key, request auth from Shell ---
    let mut buffered_lines: Vec<String> = Vec::new();
    let provider = if crate::needs_auth(&config) {
        match request_auth(&mut config, &mut lines, &mut writer, &mut buffered_lines).await {
            Some(p) => p,
            None => {
                // Auth failed/cancelled, use mock provider
                Box::new(crate::provider::mock::MockProvider::text_only(
                    "Authentication required. Please configure API key via environment variable or config.toml.",
                )) as Box<dyn crate::provider::ContentGenerator>
            }
        }
    } else {
        crate::create_provider(&config)
    };

    let resolved = config.resolve_provider();
    let extra_params = resolved.extra_params.clone();

    // --- Extension Manager setup ---
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
    skill_manager.start_watching().await;

    let mut tools = ToolRegistry::with_defaults(skill_manager);
    if args.enable_shell_evidence_tool {
        tools = tools.with_shell_evidence();
    }
    let mut engine = CoshCore::new(config, provider, tools);
    engine.extra_params = extra_params;
    engine
        .hook_system
        .register_extension_hooks(&ext_manager.hook_definitions());

    if let Some(ref sid) = args.resume {
        engine.session_id = sid.clone();
    }

    if let Some(ref prompt) = args.prompt {
        let start = std::time::Instant::now();
        match engine
            .handle_user_message(prompt, &mut lines, &mut writer)
            .await
        {
            Ok(()) => {
                let result_msg = OutputMessage::Result {
                    subtype: Some("success".to_string()),
                    is_error: false,
                    result: Some("completed".to_string()),
                    errors: None,
                    session_id: Some(engine.session_id.clone()),
                    env_delta: None,
                    duration_ms: Some(start.elapsed().as_millis() as u64),
                };
                engine.emit(&mut writer, &result_msg);
            }
            Err(e) => {
                let err_msg = OutputMessage::result_error(&engine.session_id, &e);
                engine.emit(&mut writer, &err_msg);
            }
        }
        return;
    }

    // Replay any lines that were buffered during the auth wait
    for buffered_line in buffered_lines {
        if !process_input_line(
            &buffered_line,
            &mut engine,
            &mut lines,
            &mut writer,
            args.enable_shell_evidence_tool,
        )
        .await
        {
            return;
        }
    }

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        if !process_input_line(
            &line,
            &mut engine,
            &mut lines,
            &mut writer,
            args.enable_shell_evidence_tool,
        )
        .await
        {
            break;
        }
    }
}

/// Process a single input line. Returns `false` if the session should shut down.
async fn process_input_line<W, R>(
    line: &str,
    engine: &mut CoshCore,
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    enable_shell_evidence_tool: bool,
) -> bool
where
    W: io::Write,
    R: AsyncBufReadExt + Unpin,
{
    let msg: InputMessage = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("failed to parse input: {e}");
            return true;
        }
    };

    match msg {
        InputMessage::ControlRequest {
            request_id,
            request,
        } => match request {
            ShellControlRequest::Initialize => {
                engine.emit(
                    writer,
                    &OutputMessage::initialize_success(&request_id, enable_shell_evidence_tool),
                );
                let init_msg = OutputMessage::system_init(
                    &engine.session_id,
                    &engine.model,
                    engine.tool_names(),
                );
                engine.emit(writer, &init_msg);

                // ─── Hook: SessionStart ───
                let cwd_str = engine.cwd().to_string_lossy().to_string();
                let ss_result = engine
                    .hook_system
                    .fire_session_start(&engine.session_id, &cwd_str)
                    .await;
                for n in &ss_result.notifications {
                    engine.emit(
                        writer,
                        &OutputMessage::hook_notification(&n.hook_name, &n.message, None, n.decision.as_deref()),
                    );
                }
                if let Some(ref ctx) = ss_result.additional_context {
                    engine
                        .messages
                        .push(crate::provider::Message::system(&format!(
                            "[Hook context] {ctx}"
                        )));
                }
            }
            ShellControlRequest::Interrupt => {
                engine.provider.cancel();
            }
            ShellControlRequest::Shutdown => return false,
            ShellControlRequest::SwitchModel { model } => {
                engine.model = model.clone();
                engine.emit(
                    writer,
                    &OutputMessage::system_status(&format!("model_switched:{model}")),
                );
            }
            ShellControlRequest::ReloadConfig => {
                engine.config = CoreConfig::load();
                engine.emit(writer, &OutputMessage::system_status("config_reloaded"));
            }
            ShellControlRequest::ConfigOverride {
                approval_mode,
                allowed_tools: _,
            } => {
                if let Some(mode) = approval_mode {
                    engine.config.agent.approval_mode = mode;
                }
                engine.emit(
                    writer,
                    &OutputMessage::system_status("config_override_applied"),
                );
            }
        },

        InputMessage::User {
            message,
            session_id,
            shell_context,
            ..
        } => {
            if let Some(sid) = session_id {
                if !sid.is_empty() {
                    engine.session_id = sid;
                }
            }
            if let Some(ctx) = shell_context {
                engine.shell_context = Some(ctx);
            }

            let start = std::time::Instant::now();

            match engine
                .handle_user_message(&message.content, lines, writer)
                .await
            {
                Ok(()) => {
                    let result_msg = OutputMessage::Result {
                        subtype: Some("success".to_string()),
                        is_error: false,
                        result: Some("completed".to_string()),
                        errors: None,
                        session_id: Some(engine.session_id.clone()),
                        env_delta: None,
                        duration_ms: Some(start.elapsed().as_millis() as u64),
                    };
                    engine.emit(writer, &result_msg);
                }
                Err(e) => {
                    let err_msg = OutputMessage::result_error(&engine.session_id, &e);
                    engine.emit(writer, &err_msg);
                }
            }
        }

        InputMessage::ControlResponse { .. } => {}
        InputMessage::RegistryRequest { .. } => {
            // Registry requests are handled in registry mode, ignore here
        }
    }
    true
}

fn apply_cli_overrides(args: &CliArgs, config: &mut CoreConfig) {
    if let Some(ref model) = args.model {
        config.ai.active_model = Some(model.clone());
    }
    if let Some(ref mode) = args.approval_mode {
        config.agent.approval_mode = mode.clone();
    }
}

/// Request authentication from Shell via the control protocol.
/// Returns a Provider if auth succeeds, None otherwise.
/// Buffered lines consumed during auth wait are appended to `buffered`.
async fn request_auth<W, R>(
    config: &mut CoreConfig,
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    buffered: &mut Vec<String>,
) -> Option<Box<dyn crate::provider::ContentGenerator>>
where
    W: std::io::Write,
    R: AsyncBufReadExt + Unpin,
{
    let request_id = "auth-init";
    let providers = builtin_auth_providers();

    let auth_msg =
        OutputMessage::auth_required(request_id, AuthReason::NotConfigured, None, providers);

    // Emit auth request
    if let Ok(json) = serde_json::to_string(&auth_msg) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
    }

    // Wait for response
    let auth_result = wait_for_auth_response(request_id, lines).await;
    buffered.extend(auth_result.buffered_lines);

    let response = auth_result.response?;

    // Apply credentials
    apply_auth_credentials(config, &response);

    // Persist if requested
    if response.persist {
        if let Err(e) = config::persist_config(config) {
            tracing::warn!("failed to persist config: {e}");
        }
    }

    // Emit success status
    let status_msg = OutputMessage::system_status("auth_ok");
    if let Ok(json) = serde_json::to_string(&status_msg) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
    }

    // Create provider from new config
    Some(crate::create_provider(config))
}
