#![forbid(unsafe_code)]

mod compression;
mod config;
mod context;
mod core;
mod hook;
mod loop_detect;
mod migrate;
mod protocol;
mod provider;
mod session;
mod tool;
mod truncator;

use std::io;

use tokio::io::{AsyncBufReadExt, BufReader};

use config::CoreConfig;
use protocol::{InputMessage, OutputMessage, ShellControlRequest};
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;
use tool::ToolRegistry;

use crate::core::CoshCore;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    if resolved.api_key.is_empty() {
        eprintln!("[cosh-core] Warning: no API key configured, using mock provider");
        return Box::new(provider::mock::MockProvider::text_only(
            "No API key configured. Please set DASHSCOPE_API_KEY or configure [ai.providers] in config.toml.",
        ));
    }
    let provider_profile = profile::profile_from_name(&resolved.provider_type);
    Box::new(OpenAICompatProvider::new(
        &resolved.base_url,
        &resolved.api_key,
        provider_profile,
    ))
}

#[tokio::main]
async fn main() {
    let config = CoreConfig::load();
    let resolved = config.resolve_provider();
    let extra_params = resolved.extra_params.clone();
    let provider = create_provider(&config);
    let tools = ToolRegistry::with_defaults();
    let mut engine = CoshCore::new(config, provider, tools);
    engine.extra_params = extra_params;

    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut lines = stdin.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let msg: InputMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[cosh-core] Failed to parse input: {e}");
                continue;
            }
        };

        match msg {
            InputMessage::ControlRequest {
                request_id: _,
                request,
            } => match request {
                ShellControlRequest::Initialize => {
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
