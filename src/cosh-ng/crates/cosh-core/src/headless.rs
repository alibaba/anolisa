use std::io::{self, Write};
use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::cli::CliArgs;
use crate::compaction::{ContextBudget, ModelCapability};
#[cfg(test)]
use crate::config::ApprovalMode;
use crate::config::CoreConfig;
use crate::core::{AgentTurnOutcome, CoshCore};
use crate::extension::{ExtensionManager, RuntimeSnapshotBuilder};
use crate::metrics::TurnMetrics;
use crate::protocol::{InputMessage, OutputMessage, ShellControlRequest, CONTROL_PROTOCOL_VERSION};
use crate::session::{PersistedSession, ProviderSessionId, SessionError, SessionStore};
use crate::sls;
use crate::tool::SessionWorkspace;

mod auth;

use auth::request_auth;

/// Exit code for a session that lost the JSONL control transport.
///
/// The Shell owns recovery: it already replaces a missing terminal result when
/// the child exits non-zero, and stderr carries the reason. Staying alive on a
/// transport we cannot write to would only reproduce the #1994 hang.
const EXIT_CONTROL_TRANSPORT_FAILURE: i32 = 74;
const EXIT_PROTOCOL_MISMATCH: i32 = 65;

pub async fn run(
    args: &CliArgs,
    mut config: CoreConfig,
    project_root: PathBuf,
    workspace: SessionWorkspace,
) -> Result<i32, String> {
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

    // Build and validate the complete runtime before authentication so invalid
    // tool selections fail without entering the interactive auth protocol.
    // Use the workspace passed from the shell host (e.g. cosh-shell) so tools
    // such as MCP filesystem servers resolve the user's project root, not the
    // cosh-core process cwd. The helper absolutizes relative paths and treats
    // empty --workspace "" as missing.
    let mut ext_manager = ExtensionManager::new(project_root.clone());
    if !args.bare {
        ext_manager.refresh();
    }
    let generation_id = crate::extension::state::publish_next_generation(None).unwrap_or_else(
        |error| {
            tracing::error!(code = %error.code(), "failed to persist extension generation: {error}");
            1
        },
    );
    let snapshot = RuntimeSnapshotBuilder::new(
        &mut ext_manager,
        &config,
        project_root.clone(),
        generation_id,
    )
    .with_shell_evidence(args.enable_shell_evidence_tool)
    .with_skill_loading(!args.bare)
    .with_tool_selection(args.tools.as_deref())
    .build()
    .await;
    if let Some(diagnostic) = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "tool_selection_invalid")
    {
        let error = diagnostic.message.clone();
        snapshot.mcp.shutdown().await;
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
    for diagnostic in &snapshot.diagnostics {
        tracing::warn!(code = %diagnostic.code, "{}", diagnostic.message);
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

    tracing::debug!(
        generation = snapshot.generation.id,
        declared_agents = snapshot.agents.list().len(),
        executable_agents = 0,
        "loaded extension runtime snapshot"
    );
    let mut engine = CoshCore::new_with_snapshot_and_session_id(
        config,
        provider,
        snapshot,
        session.record.session_id.to_string(),
        project_root,
        workspace,
    );
    engine.set_session_resumed(session.resumed);
    let live_extension_runtime = crate::registry::LiveExtensionRuntime::new(
        engine.extension_generation.clone(),
        args.enable_shell_evidence_tool,
        !args.bare,
        args.tools.clone(),
    );
    engine.extra_params = extra_params;
    engine.messages = session.record.messages.clone();
    engine.compaction.load_state(
        session.record.compaction.clone(),
        session.record.compaction_revision,
    );
    if !session.record.model.is_empty() {
        engine.model = session.record.model.clone();
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
                    max_turns: None,
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
        let transport_failed = engine.control_transport_failure().is_some();
        engine.shutdown_extension_runtime().await;
        return Ok(if transport_failed {
            EXIT_CONTROL_TRANSPORT_FAILURE
        } else {
            0
        });
    }

    let mut extensions = HeadlessExtensionRuntime {
        manager: &mut ext_manager,
        live: &live_extension_runtime,
    };

    // Replay any lines that were buffered during the auth wait
    for buffered_line in buffered_lines {
        match process_input_line(
            &buffered_line,
            &mut engine,
            &mut lines,
            &mut writer,
            args,
            &mut session,
            &mut extensions,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => {
                engine.shutdown_extension_runtime().await;
                return Ok(0);
            }
            InputLineResult::InvalidJson => {
                engine.shutdown_extension_runtime().await;
                return Ok(1);
            }
            InputLineResult::ProtocolMismatch => {
                engine.shutdown_extension_runtime().await;
                return Ok(EXIT_PROTOCOL_MISMATCH);
            }
        }
        if engine.control_transport_failure().is_some() {
            engine.shutdown_extension_runtime().await;
            return Ok(EXIT_CONTROL_TRANSPORT_FAILURE);
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
            &mut extensions,
        )
        .await
        {
            InputLineResult::Continue => {}
            InputLineResult::Shutdown => {
                engine.shutdown_extension_runtime().await;
                return Ok(0);
            }
            InputLineResult::InvalidJson => {
                engine.shutdown_extension_runtime().await;
                return Ok(1);
            }
            InputLineResult::ProtocolMismatch => {
                engine.shutdown_extension_runtime().await;
                return Ok(EXIT_PROTOCOL_MISMATCH);
            }
        }
        // Checked after every line, not only after a turn: once the transport
        // is gone the process can neither answer nor be answered.
        if engine.control_transport_failure().is_some() {
            engine.shutdown_extension_runtime().await;
            return Ok(EXIT_CONTROL_TRANSPORT_FAILURE);
        }
    }
    engine.shutdown_extension_runtime().await;
    Ok(0)
}

enum InputLineResult {
    Continue,
    Shutdown,
    InvalidJson,
    ProtocolMismatch,
}

/// Processes a single JSONL input line.
async fn process_input_line<W, R>(
    line: &str,
    engine: &mut CoshCore,
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    args: &CliArgs,
    session: &mut SessionRuntime,
    extensions: &mut HeadlessExtensionRuntime<'_>,
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

    match extensions
        .live
        .refresh_linked_runtime(&engine.config, extensions.manager)
        .await
    {
        Ok(true) => {
            engine.drain_retired_extension_snapshots().await;
            if let Err(error) = extensions.live.persist_current_generation() {
                tracing::error!("failed to persist linked extension generation: {error}");
            }
        }
        Ok(false) => {}
        Err(error) => tracing::warn!("linked extension reload deferred: {error}"),
    }

    match msg {
        InputMessage::ControlRequest {
            request_id,
            request,
        } => match request {
            ShellControlRequest::Initialize {
                fire_session_start,
                protocol_version,
                capabilities,
            } => {
                if let Some(received_version) = protocol_version {
                    if received_version != CONTROL_PROTOCOL_VERSION {
                        engine.emit(
                            writer,
                            &OutputMessage::initialize_version_error(&request_id, received_version),
                        );
                        return InputLineResult::ProtocolMismatch;
                    }
                }
                engine.client_capabilities = capabilities;
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

                // Only the shell-owned transport may suppress SessionStart.
                // Generic headless clients keep the historical lifecycle even
                // if they send the optional control field themselves.
                if fire_session_start || !args.cosh_shell_transport {
                    // ─── Hook: SessionStart ───
                    let cwd_str = engine.cwd().to_string_lossy().to_string();
                    let ss_result = engine
                        .hook_system
                        .fire_session_start(&engine.session_id, &cwd_str)
                        .await;
                    engine
                        .audit
                        .record_session_hook_decision("session_start", "observed");
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

            let raw_user_input = args
                .cosh_shell_transport
                .then_some(message.raw_user_input.as_deref())
                .flatten();
            let turn_result = engine
                .handle_user_message_with_raw_input(&message.content, raw_user_input, lines, writer)
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
                        max_turns: None,
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
            engine.drain_retired_extension_snapshots().await;
            if let Err(error) = extensions.live.persist_current_generation() {
                tracing::error!("failed to persist activated extension generation: {error}");
            }
        }

        InputMessage::ControlResponse { .. } => {}
        // Receipts only matter to an interactive `wait_for_approval` loop;
        // the headless path never waits on one.
        InputMessage::ApprovalReceipt { .. } => {}
        InputMessage::RegistryRequest {
            request_id,
            domain,
            action,
            params,
        } => {
            let response = crate::registry::handle_registry_request(
                &request_id,
                &domain,
                &action,
                &params,
                &mut engine.config,
                extensions.manager,
                None,
                Some(extensions.live),
            )
            .await;
            engine.emit(writer, &response);
            engine.drain_retired_extension_snapshots().await;
        }
    }
    InputLineResult::Continue
}

struct HeadlessExtensionRuntime<'a> {
    manager: &'a mut ExtensionManager,
    live: &'a crate::registry::LiveExtensionRuntime,
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
        let workspace = args.workspace_root();
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
        // The revision clock is persisted even when no projection survives, so
        // the next commit cannot reuse an already published revision.
        self.record.compaction_revision = engine.compaction.revision();
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
        let compacted_through = engine
            .compaction
            .state()
            .map(|state| state.compacted_through)
            .unwrap_or(0);
        match crate::compaction::has_new_compactable_prefix(
            &engine.messages,
            policy.preserve_recent_runs,
            compacted_through,
        ) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(
                    session_id = %self.record.session_id,
                    "automatic compaction preflight rejected the transcript: {error}"
                );
                return;
            }
        }
        // Bind to the durable revision clock, not the live projection: this
        // recommendation is emitted right after a persist, and a projection
        // that sanitization later rejects must not make it look stale.
        let projection_revision = engine.compaction.revision();
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
    error_code: Option<&'static str>,
    max_turns: Option<u32>,
    session_error_code: Option<&'static str>,
}

impl TurnFailure {
    fn output_message(&self, session_id: &str) -> OutputMessage {
        match self.session_error_code {
            Some(code) => {
                OutputMessage::session_result_error(session_id, &self.message, code, "persist")
            }
            None if self.error_code == Some("max_turns") && self.max_turns.is_some() => {
                OutputMessage::max_turns_result_error(
                    session_id,
                    &self.message,
                    self.max_turns.unwrap_or_default(),
                )
            }
            None => {
                OutputMessage::result_error_with_code(session_id, &self.message, self.error_code)
            }
        }
    }
}

fn combine_turn_and_persist(
    turn: Result<AgentTurnOutcome, String>,
    persist: Result<(), SessionError>,
) -> Result<(), TurnFailure> {
    match (turn, persist) {
        (Ok(AgentTurnOutcome::Completed), Ok(())) => Ok(()),
        (Ok(AgentTurnOutcome::MaxTurns { limit }), Ok(())) => Err(TurnFailure {
            message: crate::core::max_turns_error(limit),
            error_code: Some("max_turns"),
            max_turns: Some(limit),
            session_error_code: None,
        }),
        (Err(turn_error), Ok(())) => Err(TurnFailure {
            error_code: None,
            max_turns: None,
            message: turn_error,
            session_error_code: None,
        }),
        (Ok(AgentTurnOutcome::Completed), Err(persist_error)) => Err(TurnFailure {
            message: format!(
                "session persistence failed [{}]: {persist_error}",
                persist_error.code()
            ),
            error_code: None,
            max_turns: None,
            session_error_code: Some(persist_error.code()),
        }),
        (Ok(AgentTurnOutcome::MaxTurns { limit }), Err(persist_error)) => Err(TurnFailure {
            message: format!(
                "{}; session persistence failed [{}]: {persist_error}",
                crate::core::max_turns_error(limit),
                persist_error.code()
            ),
            error_code: None,
            max_turns: None,
            session_error_code: Some(persist_error.code()),
        }),
        (Err(turn_error), Err(persist_error)) => Err(TurnFailure {
            message: format!(
                "{turn_error}; session persistence failed [{}]: {persist_error}",
                persist_error.code()
            ),
            error_code: None,
            max_turns: None,
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
        config.agent.approval_mode = *mode;
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

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;

    use super::*;

    #[test]
    fn typed_max_turn_outcome_sets_structured_result_fields() {
        let failure = combine_turn_and_persist(Ok(AgentTurnOutcome::MaxTurns { limit: 5 }), Ok(()))
            .expect_err("max-turn outcome");

        assert_eq!(failure.error_code, Some("max_turns"));
        assert_eq!(failure.max_turns, Some(5));
        assert_eq!(failure.message, "Agent exceeded max turns (5)");
        assert_eq!(failure.session_error_code, None);
    }

    #[test]
    fn matching_provider_error_text_stays_an_ordinary_failure() {
        let failure = combine_turn_and_persist(Err(crate::core::max_turns_error(5)), Ok(()))
            .expect_err("provider error");

        assert_eq!(failure.error_code, None);
        assert_eq!(failure.max_turns, None);
        assert_eq!(failure.message, "Agent exceeded max turns (5)");
        assert_eq!(failure.session_error_code, None);
    }

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

        assert_eq!(config.agent.approval_mode, ApprovalMode::Recommend);
        assert!(!config.session.auto_persist);
    }
}
