use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

use super::claude::{
    is_terminal_agent_event, line_progress, send_agent_event, terminate_process,
    update_completion_flags,
};
use super::cosh_core_process::{
    drain_process_events, exit_failure_message, replace_synthetic_completion_for_nonzero_exit,
    run_sync_cosh_core_process, start_control_protocol_cosh_core_process,
    suppress_synthetic_completion_after_transport_failure,
};
use super::prompt::provider_prompt_contract_with_evidence_access;
use super::{
    agent_event_is_provider_progress, control_protocol, prompt_from_request_with_evidence_policy,
    record_cancellation_pending_session, run_provider_process_loop, spawn_provider_child,
    start_threaded_adapter_run, AdapterError, AdapterInstance, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, ClaudeStreamParser, PreparedInvocation,
    ProviderCancellationArtifactStore, ProviderLineProgress, ProviderPromptArgMode,
    ProviderRunOutcome, ProviderStdinMode,
};

pub(super) mod question_ingress;
pub(super) mod question_writer;
mod recovery;
mod session;

pub(super) use recovery::{
    begin_session_attempt, commit_pending_session_for_scope, invalidate_resume_on_session_failure,
    mark_recovery_failure, retain_context_session, session_scope_from_request,
    terminal_events_for_session_commit, SessionResumeAttempt,
};
pub use recovery::{SessionRecovery, SessionRecoveryState, SessionRuntimeState};
pub use session::{
    SessionClearFailure, SessionClearInterruption, SessionClearPlan, SessionClearResult,
    SessionErrorInfo, SessionHealth, SessionList, SessionManagementClient, SessionSummary,
};

#[derive(Debug, Clone)]
/// Adapter that delegates Agent turns and session ownership to cosh-core.
pub struct CoshCoreAdapter {
    /// cosh-core executable path.
    pub program: String,
    /// Whether this adapter may start a real provider process.
    pub allow_model_call: bool,
    /// Atomically owned active session, workspace, generation, and recovery state.
    pub session: Arc<Mutex<SessionRuntimeState>>,
}

impl Default for CoshCoreAdapter {
    fn default() -> Self {
        let program = std::env::var("COSH_CORE_PATH").unwrap_or_else(|_| {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let sibling = dir.join("cosh-core");
                    if sibling.is_file() {
                        return sibling.to_string_lossy().into_owned();
                    }
                }
            }
            "cosh-core".to_string()
        });
        Self {
            program,
            allow_model_call: false,
            session: Arc::new(Mutex::new(SessionRuntimeState::default())),
        }
    }
}

impl CoshCoreAdapter {
    /// Enables or disables real model process execution.
    pub fn with_model_call(mut self, allow: bool) -> Self {
        self.allow_model_call = allow;
        self
    }

    /// Lists persisted sessions in a canonical workspace.
    ///
    /// # Errors
    ///
    /// Returns a recoverable management protocol error.
    pub fn list_sessions(&self, workspace_scope: &str) -> Result<SessionList, SessionErrorInfo> {
        self.list_sessions_page(workspace_scope, 20, None)
    }

    /// Lists one bounded page while preserving the core-owned opaque cursor.
    ///
    /// # Errors
    ///
    /// Returns a recoverable management protocol error.
    pub fn list_sessions_page(
        &self,
        workspace_scope: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<SessionList, SessionErrorInfo> {
        SessionManagementClient::new(self.program.clone()).list(workspace_scope, limit, cursor)
    }

    /// Inspects a persisted session summary without selecting it.
    ///
    /// # Errors
    ///
    /// Returns a recoverable management protocol error.
    pub fn inspect_session(
        &self,
        workspace_scope: &str,
        session_id: &str,
    ) -> Result<SessionSummary, SessionErrorInfo> {
        SessionManagementClient::new(self.program.clone()).inspect(workspace_scope, session_id)
    }

    /// Validates and selects a persisted session for the next Agent request.
    ///
    /// # Errors
    ///
    /// Returns a recoverable validation or transport error.
    pub fn select_session(
        &self,
        workspace_scope: &str,
        session_id: &str,
    ) -> Result<SessionSummary, SessionErrorInfo> {
        // Serialize with other mutating management calls through the gate;
        // the state lock itself is never held across subprocess I/O, so
        // snapshot readers and turn commits cannot block on validation.
        let gate = self.management_gate();
        let _management = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let summary = SessionManagementClient::new(self.program.clone())
            .validate(workspace_scope, session_id);
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match summary {
            Ok(summary) => {
                session.select_session(summary.session_id.clone(), summary.workspace_scope.clone());
                Ok(summary)
            }
            Err(error) => {
                session.fail_selection(error.clone());
                Err(error)
            }
        }
    }

    /// Clears explicit persisted sessions with active and selected IDs protected.
    ///
    /// # Errors
    ///
    /// Returns a recoverable request-level management error.
    pub fn clear_sessions(
        &self,
        workspace_scope: &str,
        session_ids: &[String],
    ) -> Result<SessionClearResult, SessionErrorInfo> {
        // Hold the gate, not the state lock, across the clear subprocess so a
        // concurrent selection cannot validate an ID that is being deleted.
        let gate = self.management_gate();
        let _management = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let protected = self.protected_session_ids();
        SessionManagementClient::new(self.program.clone()).clear(
            workspace_scope,
            session_ids,
            &protected,
        )
    }

    /// Prepares exact clear-all candidates without eagerly loading all summaries.
    ///
    /// # Errors
    ///
    /// Returns a recoverable request-level management error.
    pub fn prepare_clear_all(
        &self,
        workspace_scope: &str,
    ) -> Result<SessionClearPlan, SessionErrorInfo> {
        let gate = self.management_gate();
        let _management = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let protected = self.protected_session_ids();
        SessionManagementClient::new(self.program.clone())
            .prepare_clear_all(workspace_scope, &protected)
    }

    /// Returns the gate that serializes mutating session-management calls.
    fn management_gate(&self) -> std::sync::Arc<Mutex<()>> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .management_gate()
    }

    /// Returns a consistent snapshot of interactive recovery state.
    pub fn recovery_snapshot(&self) -> SessionRecovery {
        self.session
            .lock()
            .map(|session| session.recovery.clone())
            .unwrap_or_default()
    }

    /// Returns the provider conversation committed after a successful turn.
    pub fn committed_session_id(&self) -> Option<String> {
        self.session
            .lock()
            .ok()
            .and_then(|session| session.active_session_id().map(str::to_string))
    }

    /// Returns active and selected provider IDs that clear must protect.
    pub fn protected_session_ids(&self) -> Vec<String> {
        let session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        protected_session_ids_from_state(&session)
    }

    fn begin_resume_attempt(
        &self,
        prepared: &mut PreparedInvocation,
        session_scope: &str,
    ) -> SessionResumeAttempt {
        let resume_id = prepared
            .args
            .windows(2)
            .find(|arguments| arguments[0] == "--resume")
            .map(|arguments| arguments[1].clone());
        let attempt = begin_session_attempt(&self.session, resume_id.as_deref(), session_scope);
        if resume_id.is_some() && matches!(attempt, SessionResumeAttempt::Fresh { .. }) {
            if let Some(index) = prepared
                .args
                .iter()
                .position(|argument| argument == "--resume")
            {
                prepared.args.drain(index..=(index + 1));
            }
        }
        attempt
    }

    /// Builds a workspace-scoped headless cosh-core invocation.
    pub fn prepare_invocation(
        &self,
        request: &AgentRequest,
        mode: CoshApprovalMode,
    ) -> PreparedInvocation {
        let disable_resume = request
            .context_hints
            .iter()
            .any(|hint| hint.contains("disable provider resume"));
        let session_scope = session_scope_from_request(request);
        let resume_session = if disable_resume {
            None
        } else {
            let selected = self.session.lock().ok().and_then(|session| {
                let recovery = &session.recovery;
                (matches!(
                    recovery.state,
                    SessionRecoveryState::Selected | SessionRecoveryState::Restoring
                ) && recovery.selected_workspace_scope.as_deref() == Some(session_scope.as_str()))
                .then(|| recovery.selected_session_id.clone())
                .flatten()
            });
            selected.or_else(|| {
                self.session.lock().ok().and_then(|session| {
                    (session.active_workspace_scope() == Some(session_scope.as_str()))
                        .then(|| session.active_session_id().map(str::to_string))
                        .flatten()
                })
            })
        };

        let approval_mode = match mode {
            CoshApprovalMode::Recommend => "strict",
            CoshApprovalMode::Auto => "auto",
            CoshApprovalMode::Trust => "trust",
        };
        let mut args = vec![
            "--headless".to_string(),
            "--enable-shell-evidence-tool".to_string(),
            "--approval-mode".to_string(),
            approval_mode.to_string(),
            "--workspace".to_string(),
            session_scope,
        ];

        if let Some(session_id) = resume_session {
            args.extend(["--resume".to_string(), session_id]);
        }

        PreparedInvocation {
            program: self.program.clone(),
            args,
            prompt: cosh_core_prompt_from_request(request, mode),
        }
    }

    /// Starts a cancellable cosh-core turn and updates recovery state.
    pub fn start_cancellable(
        &self,
        request: AgentRequest,
        mode: CoshApprovalMode,
    ) -> AgentRunHandle {
        let session_scope = session_scope_from_request(&request);
        let mut prepared = self.prepare_invocation(&request, mode);
        if !self.allow_model_call {
            let adapter = AdapterInstance::CoshCore(self.clone());
            return start_threaded_adapter_run(adapter, request);
        }

        let resume_attempt = self.begin_resume_attempt(&mut prepared, &session_scope);
        if mode.uses_control_protocol() {
            return start_control_protocol_cosh_core_process(
                request.id,
                prepared,
                Arc::clone(&self.session),
                session_scope,
                resume_attempt,
            );
        }

        start_cancellable_cosh_core_process(
            request.id,
            prepared,
            Arc::clone(&self.session),
            session_scope,
            resume_attempt,
        )
    }
}

impl AgentAdapter for CoshCoreAdapter {
    fn name(&self) -> &'static str {
        "cosh-core"
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        AgentBackendCapabilities {
            text_stream: true,
            thinking_stream: false,
            session_resume: true,
            tool_intent: true,
            user_question: true,
            cancellable: true,
            control_protocol: true,
        }
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError> {
        let mut events = Vec::new();
        self.run_stream(request, &mut |event| {
            events.push(event);
            Ok(())
        })?;
        Ok(events)
    }

    fn run_stream(
        &self,
        request: &AgentRequest,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        let mut prepared = self.prepare_invocation(request, CoshApprovalMode::Recommend);
        if !self.allow_model_call {
            for event in cosh_core_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }
        let session_scope = session_scope_from_request(request);
        let resume_attempt = self.begin_resume_attempt(&mut prepared, &session_scope);
        run_sync_cosh_core_process(
            request,
            &prepared,
            &self.session,
            &session_scope,
            &resume_attempt,
            sink,
        )
    }
}

fn protected_session_ids_from_state(session: &SessionRuntimeState) -> Vec<String> {
    let mut protected = session
        .active_session_id()
        .map(str::to_string)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(selected) = matches!(
        session.recovery.state,
        SessionRecoveryState::Selected | SessionRecoveryState::Restoring
    )
    .then(|| session.recovery.selected_session_id.clone())
    .flatten()
    {
        if !protected.contains(&selected) {
            protected.push(selected);
        }
    }
    protected
}

fn cosh_core_prompt_from_request(request: &AgentRequest, mode: CoshApprovalMode) -> String {
    let access = crate::evidence::ShellEvidenceAccess::ControlProtocolTool;
    let request_prompt = prompt_from_request_with_evidence_policy(
        request,
        access,
        mode != CoshApprovalMode::Recommend,
    );
    format!(
        "{}{}",
        request_prompt,
        provider_prompt_contract_with_evidence_access(mode, "shell", access)
    )
}

fn cosh_core_dry_run_events(
    request: &AgentRequest,
    prepared: &PreparedInvocation,
) -> Vec<AgentEvent> {
    vec![
        AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "prepared".to_string(),
            message: format!(
                "cosh-core invocation prepared: {} {}",
                prepared.program,
                prepared.args.join(" ")
            ),
        },
        AgentEvent::Recommendation {
            run_id: request.id.clone(),
            summary:
                "cosh-core adapter is configured but model calls are disabled in dry-run mode."
                    .to_string(),
            commands: vec![format!("{} {}", prepared.program, prepared.args.join(" "))],
            auto_execute: false,
        },
    ]
}

fn start_cancellable_cosh_core_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<SessionRuntimeState>>,
    session_scope: String,
    resume_attempt: SessionResumeAttempt,
) -> AgentRunHandle {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let child_pid = Arc::new(Mutex::new(None::<u32>));
    let pending_session = Arc::new(Mutex::new(None));
    let cancellation_artifacts = ProviderCancellationArtifactStore::default();

    let cancel_flag = Arc::clone(&cancelled);
    let cancel_pid = Arc::clone(&child_pid);
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(pid) = cancel_pid.lock().ok().and_then(|guard| *guard) {
            terminate_process(pid);
        }
    });

    let pending_session_for_thread = Arc::clone(&pending_session);
    let session_scope_for_thread = session_scope;
    let cancellation_artifacts_for_thread = cancellation_artifacts.clone();
    thread::spawn(move || {
        send_agent_event(
            &sender,
            AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "starting".to_string(),
                message: "starting cosh-core headless backend".to_string(),
            },
        );

        let mut child = match spawn_provider_child(
            &prepared,
            "cosh-core",
            ProviderStdinMode::Null,
            ProviderPromptArgMode::TrailingArgIfNonEmpty,
        ) {
            Ok(child) => child,
            Err(err) => {
                let _ = mark_recovery_failure(&session_state, &resume_attempt, &err.message);
                let _ = sender.send(Err(err));
                return;
            }
        };

        if let Ok(mut pid) = child_pid.lock() {
            *pid = Some(child.id());
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_process(child.id());
        }

        let mut parser = ClaudeStreamParser::new(
            run_id.clone(),
            Some(Arc::clone(&pending_session_for_thread)),
        );
        let mut completed = false;
        let mut failed = false;
        let mut terminal_events = Vec::new();
        let (process_tx, process_rx) = mpsc::channel();
        let outcome = run_provider_process_loop(
            run_id.clone(),
            "cosh-core",
            &mut child,
            Arc::clone(&child_pid),
            Arc::clone(&cancelled),
            cancellation_artifacts_for_thread.clone(),
            &process_tx,
            |line| {
                let events = parser.parse_line(&line);
                let progressed = events.iter().any(agent_event_is_provider_progress);
                if events.is_empty() {
                    if let Some(auth_event) = try_parse_auth_required_from_line(&line, &run_id) {
                        send_agent_event(&sender, auth_event);
                        return Ok(ProviderLineProgress::AwaitingApproval);
                    }
                }
                for event in events {
                    update_completion_flags(&event, &mut completed, &mut failed);
                    if is_terminal_agent_event(&event) {
                        terminal_events.push(event);
                    } else {
                        send_agent_event(&sender, event);
                    }
                }
                Ok(line_progress(progressed))
            },
            || Ok(Vec::new()),
        );

        let (process_events, transport_error) = drain_process_events(&process_rx);
        let transport_failed = matches!(outcome, ProviderRunOutcome::Failed);
        let exit_failure = match &outcome {
            ProviderRunOutcome::Cancelled => {
                invalidate_resume_on_session_failure(
                    &resume_attempt,
                    parser.session_error_code(),
                    parser.session_error_phase(),
                    &terminal_events,
                    &session_state,
                );
                let _ = commit_pending_session_for_scope(
                    false,
                    true,
                    &session_state,
                    &pending_session_for_thread,
                    &session_scope_for_thread,
                    parser.session_resumable(),
                    &resume_attempt,
                );
                record_cancellation_pending_session(
                    &cancellation_artifacts_for_thread,
                    "cosh-core",
                    &run_id,
                    pending_session_for_thread
                        .lock()
                        .ok()
                        .and_then(|session| session.clone()),
                );
                for event in process_events {
                    send_agent_event(&sender, event);
                }
                if let Some(error) = transport_error {
                    let _ = sender.send(Err(error));
                }
                return;
            }
            ProviderRunOutcome::Failed => None,
            ProviderRunOutcome::Exited {
                status,
                stderr_tail,
            } if !status.success() => Some(exit_failure_message(status, stderr_tail)),
            ProviderRunOutcome::Exited { .. } => None,
        };

        let had_terminal_result = !terminal_events.is_empty();
        let finish_result = parser.finish(&mut |event| {
            update_completion_flags(&event, &mut completed, &mut failed);
            if is_terminal_agent_event(&event) {
                terminal_events.push(event);
            } else {
                send_agent_event(&sender, event);
            }
            Ok(())
        });
        suppress_synthetic_completion_after_transport_failure(
            transport_failed,
            had_terminal_result,
            &mut completed,
            &mut failed,
            &mut terminal_events,
        );
        replace_synthetic_completion_for_nonzero_exit(
            &run_id,
            exit_failure,
            had_terminal_result,
            &mut completed,
            &mut failed,
            &mut terminal_events,
        );
        invalidate_resume_on_session_failure(
            &resume_attempt,
            parser.session_error_code(),
            parser.session_error_phase(),
            &terminal_events,
            &session_state,
        );
        let retain_session = retain_context_session(&terminal_events, parser.session_error_phase());
        let commit_outcome = commit_pending_session_for_scope(
            completed || retain_session,
            failed && !retain_session,
            &session_state,
            &pending_session_for_thread,
            &session_scope_for_thread,
            parser.session_resumable(),
            &resume_attempt,
        );
        for event in terminal_events_for_session_commit(&run_id, terminal_events, commit_outcome) {
            send_agent_event(&sender, event);
        }
        for event in process_events {
            send_agent_event(&sender, event);
        }
        if let Some(error) = transport_error.or_else(|| finish_result.err()) {
            let _ = sender.send(Err(error));
        }
    });

    AgentRunHandle {
        receiver,
        cancel,
        approval_sender: None,
        question_answer_confirmation: None,
        auth_sender: None,
        control_capabilities: Arc::new(Mutex::new(
            control_protocol::ControlProtocolCapabilities::default(),
        )),
        pending_provider_session: Some(pending_session),
        cancellation_artifacts,
    }
}

fn try_parse_auth_required_from_line(line: &str, run_id: &str) -> Option<AgentEvent> {
    let trimmed = line.trim();
    if !trimmed.contains("auth_required") {
        return None;
    }
    let parsed = control_protocol::parse_control_request(trimmed)?;
    match parsed {
        control_protocol::ControlRequest::AuthRequired {
            request_id,
            reason,
            error_message,
            credentials_unavailable,
            providers,
        } => Some(AgentEvent::AuthRequired {
            run_id: run_id.to_string(),
            request_id,
            reason,
            error_message,
            credentials_unavailable,
            providers,
        }),
        _ => None,
    }
}
