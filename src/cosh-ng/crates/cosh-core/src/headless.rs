use std::io;
use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::auth::{apply_auth_credentials, builtin_auth_providers, wait_for_auth_response};
use crate::cli::CliArgs;
use crate::config::{self, CoreConfig};
use crate::core::CoshCore;
use crate::extension::ExtensionManager;
use crate::metrics::TurnMetrics;
use crate::protocol::{AuthReason, InputMessage, OutputMessage, ShellControlRequest};
use crate::session::{PersistedSession, ProviderSessionId, SessionError, SessionStore};
use crate::skill::manager::expand_path;
use crate::skill::SkillManager;
use crate::sls;
use crate::tool::ToolRegistry;

pub async fn run(args: &CliArgs, mut config: CoreConfig) -> i32 {
    apply_cli_overrides(args, &mut config);

    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut session = match SessionRuntime::initialize(args, &config) {
        Ok(session) => session,
        Err(error) => {
            emit_session_error(&mut writer, args.resume.as_deref(), &error);
            return 0;
        }
    };

    let stdin = BufReader::new(tokio::io::stdin());
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
    session.finalize_model(&resolved.model, args.model.is_some());

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
    engine.session_id = session.record.session_id.to_string();
    engine.messages = session.record.messages.clone();
    if !session.record.model.is_empty() {
        engine.model = session.record.model.clone();
    }
    engine
        .hook_system
        .register_extension_hooks(&ext_manager.hook_definitions());

    if let Some(ref prompt) = args.prompt {
        if !session.resumable() {
            engine.emit(
                &mut writer,
                &OutputMessage::system_init(
                    &engine.session_id,
                    &engine.model,
                    engine.tool_names(),
                    false,
                ),
            );
        }
        let start = std::time::Instant::now();
        let turn_result = engine
            .handle_user_message(prompt, &mut lines, &mut writer)
            .await;
        let persist_result = session.persist(&engine);
        match combine_turn_and_persist(turn_result, persist_result) {
            Ok(()) => {
                let duration = start.elapsed();
                sls::append_sls_log(&engine.build_sls_record(duration));
                let result_msg = OutputMessage::Result {
                    subtype: Some("success".to_string()),
                    is_error: false,
                    result: Some("completed".to_string()),
                    errors: None,
                    error_code: None,
                    session_error_code: None,
                    session_error_phase: None,
                    session_id: Some(engine.session_id.clone()),
                    env_delta: None,
                    duration_ms: Some(duration.as_millis() as u64),
                };
                engine.emit(&mut writer, &result_msg);
            }
            Err(failure) => {
                sls::append_sls_log(&engine.build_sls_record(start.elapsed()));
                let err_msg = failure.output_message(&engine.session_id);
                engine.emit(&mut writer, &err_msg);
            }
        }
        return 0;
    }

    // Replay any lines that were buffered during the auth wait
    for buffered_line in buffered_lines {
        match process_input_line(
            &buffered_line,
            &mut engine,
            &mut lines,
            &mut writer,
            args.enable_shell_evidence_tool,
            &mut session,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => return 0,
            InputLineResult::InvalidJson => return 1,
        }
    }

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        match process_input_line(
            &line,
            &mut engine,
            &mut lines,
            &mut writer,
            args.enable_shell_evidence_tool,
            &mut session,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => return 0,
            InputLineResult::InvalidJson => return 1,
        }
    }
    0
}

enum InputLineResult {
    Continue,
    Shutdown,
    InvalidJson,
}

/// Processes a single JSONL input line.
async fn process_input_line<W, R>(
    line: &str,
    engine: &mut CoshCore,
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    enable_shell_evidence_tool: bool,
    session: &mut SessionRuntime,
) -> InputLineResult
where
    W: io::Write,
    R: AsyncBufReadExt + Unpin,
{
    let msg: InputMessage = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            const ERROR: &str = "failed to parse stdin line as JSON";
            tracing::debug!("{ERROR}: {e}");
            engine.emit(
                writer,
                &OutputMessage::result_error_with_code(
                    &engine.session_id,
                    ERROR,
                    Some("InvalidJsonlInput"),
                ),
            );
            return InputLineResult::InvalidJson;
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
                    session.resumable(),
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
                        &OutputMessage::hook_notification(
                            &n.hook_name,
                            &n.message,
                            None,
                            n.decision.as_deref(),
                        ),
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
            ShellControlRequest::Shutdown => return InputLineResult::Shutdown,
            ShellControlRequest::SwitchModel { model } => {
                engine.model = model.clone();
                engine.emit(
                    writer,
                    &OutputMessage::system_status(&format!("model_switched:{model}")),
                );
            }
            ShellControlRequest::ReloadConfig => {
                engine.config =
                    CoreConfig::load_for_workspace(std::path::Path::new(session.workspace_scope()));
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
                if !sid.is_empty() && sid != "default" && sid != engine.session_id {
                    let error = format!(
                        "session identity conflict: initialized {}, received {sid}",
                        engine.session_id
                    );
                    engine.emit(
                        writer,
                        &OutputMessage::result_error(&engine.session_id, &error),
                    );
                    return InputLineResult::Continue;
                }
            }
            if let Some(ctx) = shell_context {
                engine.shell_context = Some(ctx);
            }

            // Reset per-turn metrics before each user message
            engine.metrics = TurnMetrics::default();
            let start = std::time::Instant::now();

            let turn_result = engine
                .handle_user_message(&message.content, lines, writer)
                .await;
            let persist_result = session.persist(engine);
            match combine_turn_and_persist(turn_result, persist_result) {
                Ok(()) => {
                    let duration = start.elapsed();
                    sls::append_sls_log(&engine.build_sls_record(duration));
                    let result_msg = OutputMessage::Result {
                        subtype: Some("success".to_string()),
                        is_error: false,
                        result: Some("completed".to_string()),
                        errors: None,
                        error_code: None,
                        session_error_code: None,
                        session_error_phase: None,
                        session_id: Some(engine.session_id.clone()),
                        env_delta: None,
                        duration_ms: Some(duration.as_millis() as u64),
                    };
                    engine.emit(writer, &result_msg);
                }
                Err(failure) => {
                    sls::append_sls_log(&engine.build_sls_record(start.elapsed()));
                    let err_msg = failure.output_message(&engine.session_id);
                    engine.emit(writer, &err_msg);
                }
            }
        }

        InputMessage::ControlResponse { .. } => {}
        InputMessage::RegistryRequest { .. } => {
            // Registry requests are handled in registry mode, ignore here
        }
    }
    InputLineResult::Continue
}

struct SessionRuntime {
    store: Option<SessionStore>,
    workspace_scope: String,
    record: PersistedSession,
    auto_persist: bool,
    resumed: bool,
}

impl SessionRuntime {
    fn initialize(args: &CliArgs, config: &CoreConfig) -> Result<Self, SessionError> {
        let workspace = args
            .workspace
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let store = match SessionStore::for_workspace(&config.session.persist_dir, &workspace) {
            Ok(store) => Some(store),
            // Resume cannot proceed without the store, but a fresh turn can:
            // degrade to a non-resumable session instead of refusing to run.
            Err(error) => {
                if args.resume.is_some() {
                    return Err(error);
                }
                tracing::warn!("session persistence disabled: {error}");
                None
            }
        };
        let workspace_scope = store
            .as_ref()
            .map(|store| store.workspace_scope().to_string())
            .unwrap_or_else(|| workspace.to_string_lossy().into_owned());
        let record = match (args.resume.as_deref(), store.as_ref()) {
            (Some(value), Some(store)) => {
                let session_id = ProviderSessionId::parse(value)?;
                store.load(&session_id)?
            }
            _ => PersistedSession::new(
                ProviderSessionId::new(),
                workspace_scope.clone(),
                String::new(),
                Vec::new(),
            ),
        };
        Ok(Self {
            store,
            workspace_scope,
            record,
            auto_persist: config.session.auto_persist,
            resumed: args.resume.is_some(),
        })
    }

    fn workspace_scope(&self) -> &str {
        &self.workspace_scope
    }

    fn finalize_model(&mut self, resolved_model: &str, explicit_override: bool) {
        if !self.resumed || explicit_override || self.record.model.is_empty() {
            self.record.model = resolved_model.to_string();
        }
    }

    fn persist(&mut self, engine: &CoshCore) -> Result<(), SessionError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        if !self.auto_persist {
            return Ok(());
        }
        self.record.messages = engine.messages.clone();
        self.record.model = engine.model.clone();
        store.persist(&mut self.record)
    }

    fn resumable(&self) -> bool {
        self.auto_persist && self.store.is_some()
    }
}

struct TurnFailure {
    message: String,
    session_error_code: Option<&'static str>,
}

impl TurnFailure {
    fn output_message(&self, session_id: &str) -> OutputMessage {
        match self.session_error_code {
            Some(code) => {
                OutputMessage::session_result_error(session_id, &self.message, code, "persist")
            }
            None => OutputMessage::result_error(session_id, &self.message),
        }
    }
}

fn combine_turn_and_persist(
    turn: Result<(), String>,
    persist: Result<(), SessionError>,
) -> Result<(), TurnFailure> {
    match (turn, persist) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(turn_error), Ok(())) => Err(TurnFailure {
            message: turn_error,
            session_error_code: None,
        }),
        (Ok(()), Err(persist_error)) => Err(TurnFailure {
            message: format!(
                "session persistence failed [{}]: {persist_error}",
                persist_error.code()
            ),
            session_error_code: Some(persist_error.code()),
        }),
        (Err(turn_error), Err(persist_error)) => Err(TurnFailure {
            message: format!(
                "{turn_error}; session persistence failed [{}]: {persist_error}",
                persist_error.code()
            ),
            session_error_code: Some(persist_error.code()),
        }),
    }
}

fn emit_session_error<W: io::Write>(
    writer: &mut W,
    requested_id: Option<&str>,
    error: &SessionError,
) {
    let session_id = requested_id.unwrap_or("<new>");
    let message = format!("session recovery failed [{}]: {error}", error.code());
    if let Ok(json) = serde_json::to_string(&OutputMessage::session_result_error(
        session_id,
        &message,
        error.code(),
        "load",
    )) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
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
