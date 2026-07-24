use std::io::{self, Write};
use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::auth::{apply_auth_credentials, builtin_auth_providers, wait_for_auth_response};
use crate::cli::CliArgs;
use crate::compaction::{ContextBudget, ModelCapability};
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

pub async fn run(args: &CliArgs, mut config: CoreConfig) -> Result<i32, String> {
    apply_cli_overrides(args, &mut config);

    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut session = match SessionRuntime::initialize(args, &config) {
        Ok(session) => session,
        Err(error) => {
            emit_session_error(&mut writer, args.resume.as_deref(), &error);
            return Ok(0);
        }
    };

    // --- Extension Manager setup ---
    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut ext_manager = ExtensionManager::new(project_root.clone());
    if !args.bare {
        ext_manager.refresh();
    }

    // --- Skill Manager setup ---
    let custom_paths: Vec<std::path::PathBuf> = config
        .skills
        .custom_paths
        .iter()
        .filter_map(|p| expand_path(p))
        .collect();
    let skill_manager = SkillManager::new(project_root, custom_paths, ext_manager.skill_dirs());
    if !args.bare {
        skill_manager.refresh().await;
        skill_manager.start_watching().await;
    }

    let mut tools = ToolRegistry::with_defaults(skill_manager);
    if args.enable_shell_evidence_tool {
        tools = tools.with_shell_evidence();
    }
    crate::tool::mcp::register_configured_tools(&mut tools, &config.mcp.servers).await;
    if let Some(selection) = args.tools.as_deref() {
        if let Err(error) = tools.retain_selected_tools(selection) {
            let message = OutputMessage::result_error_with_code(
                session.record.session_id.as_str(),
                &error,
                Some("InvalidToolSelection"),
            );
            if let Ok(json) = serde_json::to_string(&message) {
                let _ = writeln!(writer, "{json}");
                let _ = writer.flush();
            }
            eprintln!("[cosh-core] {error}");
            return Ok(2);
        }
    }

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

    let mut engine = CoshCore::new(config, provider, tools);
    engine.extra_params = extra_params;
    engine.session_id = session.record.session_id.to_string();
    engine.messages = session.record.messages.clone();
    engine
        .compaction
        .load_state(session.record.compaction.clone());
    if !session.record.model.is_empty() {
        engine.model = session.record.model.clone();
    }
    if !args.bare {
        engine
            .hook_system
            .register_extension_hooks(&ext_manager.hook_definitions());
    }

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
                session.recommend_auto_compaction(&mut engine, &mut writer);
            }
            Err(failure) => {
                sls::append_sls_log(&engine.build_sls_record(start.elapsed()));
                let err_msg = failure.output_message(&engine.session_id);
                engine.emit(&mut writer, &err_msg);
            }
        }
        return Ok(0);
    }

    // Replay any lines that were buffered during the auth wait
    for buffered_line in buffered_lines {
        match process_input_line(
            &buffered_line,
            &mut engine,
            &mut lines,
            &mut writer,
            args,
            &mut session,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => return Ok(0),
            InputLineResult::InvalidJson => return Ok(1),
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
            args,
            &mut session,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => return Ok(0),
            InputLineResult::InvalidJson => return Ok(1),
        }
    }
    Ok(0)
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
    args: &CliArgs,
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
                    &OutputMessage::initialize_success(
                        &request_id,
                        args.enable_shell_evidence_tool,
                    ),
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

                // A resumed session may already exceed the soft threshold;
                // surface the same background-compaction recommendation here.
                session.recommend_auto_compaction(engine, writer);
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
                    load_runtime_config(args, std::path::Path::new(session.workspace_scope()));
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
                    // Idle boundary: the Agent run finished and its transcript
                    // was persisted, so background compaction is safe now. The
                    // shell owns the compactor process so its prompt returns
                    // immediately; this process only reports the pressure.
                    session.recommend_auto_compaction(engine, writer);
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
        // Emergency in-run compaction updates the projection in memory; it
        // commits together with the transcript it belongs to.
        self.record.compaction = engine.compaction.state().cloned();
        store.persist(&mut self.record)
    }

    fn resumable(&self) -> bool {
        self.auto_persist && self.store.is_some()
    }

    /// Runs idle-boundary automatic compaction when the soft threshold is
    /// crossed, with per-context-revision failure suppression.
    /// Emits a background-compaction recommendation when the soft threshold
    /// is crossed at an idle boundary.
    ///
    /// The recommendation is a cheap synchronous status line; the shell owns
    /// the actual compactor process (`cosh-core --compact`) so this process
    /// can exit and the user gets the normal shell prompt back immediately.
    /// The payload carries the context revision (generation + projection
    /// revision) so the shell can suppress retrigger loops per revision.
    fn recommend_auto_compaction<W: io::Write>(&self, engine: &mut CoshCore, writer: &mut W) {
        let policy = &engine.config.session.compaction;
        if !policy.enabled || !policy.auto || !self.resumable() {
            return;
        }
        let capability = ModelCapability::resolve(
            policy,
            engine.config.agent.session_token_limit,
            &engine.model,
        );
        let prefix_tokens = engine.estimate_prefix_tokens();
        let budget = ContextBudget::compute(capability, prefix_tokens, policy);
        let history_tokens = engine.effective_history_tokens(prefix_tokens);
        if !budget.over_trigger(history_tokens) {
            return;
        }
        let projection_revision = self
            .record
            .compaction
            .as_ref()
            .map(|state| state.revision)
            .unwrap_or(0);
        // Versioned protocol: the shell must be able to bind the recommendation
        // to the exact session and context revision it was emitted for, and
        // reject anything malformed. Field order is fixed:
        //   compaction_recommended_v1:<session-id>:<generation>:<revision>:<history>:<usable>
        engine.emit(
            writer,
            &OutputMessage::system_status(&format!(
                "compaction_recommended_v1:{}:{}:{}:{}:{}",
                self.record.session_id,
                self.record.generation,
                projection_revision,
                history_tokens,
                budget.usable_history
            )),
        );
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
    if let Some(ref tools) = args.allowed_tools {
        config.agent.allowed_tools = tools
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(ToString::to_string)
            .collect();
    }
}

fn load_runtime_config(args: &CliArgs, workspace: &std::path::Path) -> CoreConfig {
    let mut config = if args.bare {
        CoreConfig::load_bare()
    } else {
        CoreConfig::load_for_workspace(workspace)
    };
    apply_cli_overrides(args, &mut config);
    config
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

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;

    use super::*;

    #[test]
    fn bare_reload_keeps_project_config_isolated() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        let project_config = workspace.path().join(".copilot-shell/config.toml");
        fs::create_dir_all(project_config.parent().expect("config parent"))
            .expect("create project config directory");
        fs::write(
            project_config,
            "[agent]\napproval_mode = \"project-only-mode\"\n",
        )
        .expect("write project config");
        let args = CliArgs::try_parse_from(["cosh-core", "--headless", "--bare"])
            .expect("parse bare args");

        let config = load_runtime_config(&args, workspace.path());

        assert_ne!(config.agent.approval_mode, "project-only-mode");
        assert!(!config.session.auto_persist);
    }
}
