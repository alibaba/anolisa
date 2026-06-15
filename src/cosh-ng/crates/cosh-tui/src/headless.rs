use std::io;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::cli::CliArgs;
use crate::config::CoreConfig;
use crate::core::CoshCore;
use crate::protocol::{InputMessage, OutputMessage, ShellControlRequest};
use crate::tool::ToolRegistry;

pub async fn run(args: &CliArgs, mut config: CoreConfig) {
    apply_cli_overrides(args, &mut config);

    let resolved = config.resolve_provider();
    let extra_params = resolved.extra_params.clone();
    let provider = crate::create_provider(&config);
    let tools = ToolRegistry::with_defaults();
    let mut engine = CoshCore::new(config, provider, tools);
    engine.extra_params = extra_params;

    if let Some(ref sid) = args.resume {
        engine.session_id = sid.clone();
    }

    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut lines = stdin.lines();

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

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let msg: InputMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[cosh-tui] Failed to parse input: {e}");
                continue;
            }
        };

        match msg {
            InputMessage::ControlRequest {
                request_id,
                request,
            } => match request {
                ShellControlRequest::Initialize => {
                    engine.emit(&mut writer, &OutputMessage::initialize_success(&request_id));
                    let init_msg = OutputMessage::system_init(
                        &engine.session_id,
                        &engine.model,
                        engine.tool_names(),
                    );
                    engine.emit(&mut writer, &init_msg);
                }
                ShellControlRequest::Interrupt => {
                    engine.provider.cancel();
                }
                ShellControlRequest::Shutdown => break,
                ShellControlRequest::SwitchModel { model } => {
                    engine.model = model.clone();
                    engine.emit(
                        &mut writer,
                        &OutputMessage::system_status(&format!("model_switched:{model}")),
                    );
                }
                ShellControlRequest::ReloadConfig => {
                    engine.config = CoreConfig::load();
                    engine.emit(
                        &mut writer,
                        &OutputMessage::system_status("config_reloaded"),
                    );
                }
                ShellControlRequest::ConfigOverride {
                    approval_mode,
                    allowed_tools: _,
                } => {
                    if let Some(mode) = approval_mode {
                        engine.config.agent.approval_mode = mode;
                    }
                    engine.emit(
                        &mut writer,
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
                    .handle_user_message(&message.content, &mut lines, &mut writer)
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
            }

            InputMessage::ControlResponse { .. } => {}
        }
    }
}

fn apply_cli_overrides(args: &CliArgs, config: &mut CoreConfig) {
    if let Some(ref model) = args.model {
        config.ai.active_model = Some(model.clone());
    }
    if let Some(ref mode) = args.approval_mode {
        config.agent.approval_mode = mode.clone();
    }
}
