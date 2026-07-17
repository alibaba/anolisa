use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

use super::claude::{
    is_terminal_agent_event, join_reader_thread, line_progress, read_lossy, send_agent_event,
    terminate_process, update_completion_flags,
};
use super::cosh_core_process::start_control_protocol_cosh_core_process;
use super::prompt::provider_prompt_contract_with_evidence_access;
use super::{
    agent_event_is_provider_progress, control_protocol, prompt_from_request_with_evidence_policy,
    record_cancellation_pending_session, run_provider_process_loop, spawn_provider_child,
    start_threaded_adapter_run, AdapterError, AdapterInstance, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, ClaudeStreamParser, PreparedInvocation,
    ProviderCancellationArtifactStore, ProviderLineProgress, ProviderPromptArgMode,
    ProviderRunOutcome, ProviderStdinMode,
};

#[derive(Debug, Clone)]
pub struct CoshCoreAdapter {
    pub program: String,
    pub allow_model_call: bool,
    pub session_id: Arc<Mutex<Option<String>>>,
    pub session_cwd: Arc<Mutex<Option<String>>>,
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
            session_id: Arc::new(Mutex::new(None)),
            session_cwd: Arc::new(Mutex::new(None)),
        }
    }
}

impl CoshCoreAdapter {
    pub fn with_model_call(mut self, allow: bool) -> Self {
        self.allow_model_call = allow;
        self
    }

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
            let session_id = self.session_id.lock().ok().and_then(|guard| guard.clone());
            let session_cwd = self.session_cwd.lock().ok().and_then(|guard| guard.clone());
            session_id.filter(|_| session_cwd.as_deref() == Some(session_scope.as_str()))
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

    pub fn start_cancellable(
        &self,
        request: AgentRequest,
        mode: CoshApprovalMode,
    ) -> AgentRunHandle {
        let session_scope = session_scope_from_request(&request);
        let prepared = self.prepare_invocation(&request, mode);
        if !self.allow_model_call {
            let adapter = AdapterInstance::CoshCore(self.clone());
            return start_threaded_adapter_run(adapter, request);
        }

        if mode.uses_control_protocol() {
            return start_control_protocol_cosh_core_process(
                request.id,
                prepared,
                Arc::clone(&self.session_id),
                Arc::clone(&self.session_cwd),
                session_scope,
            );
        }

        start_cancellable_cosh_core_process(
            request.id,
            prepared,
            Arc::clone(&self.session_id),
            Arc::clone(&self.session_cwd),
            session_scope,
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
        let prepared = self.prepare_invocation(request, CoshApprovalMode::Recommend);
        if !self.allow_model_call {
            for event in cosh_core_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }

        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting cosh-core headless backend".to_string(),
        })?;

        let mut child = Command::new(&prepared.program)
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AdapterError {
                message: format!("failed to run cosh-core: {err}"),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| AdapterError {
            message: "failed to capture cosh-core stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| AdapterError {
            message: "failed to capture cosh-core stderr".to_string(),
        })?;
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let pending_session = Arc::new(Mutex::new(None));
        let mut parser =
            ClaudeStreamParser::new(request.id.clone(), Some(Arc::clone(&pending_session)));
        let mut completed = false;
        let mut failed = false;
        let mut terminal_events = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|err| AdapterError {
                message: format!("failed to read cosh-core stream: {err}"),
            })?;
            for event in parser.parse_line(&line) {
                update_completion_flags(&event, &mut completed, &mut failed);
                if is_terminal_agent_event(&event) {
                    terminal_events.push(event);
                } else {
                    sink(event)?;
                }
            }
        }

        let status = child.wait().map_err(|err| AdapterError {
            message: format!("failed to wait for cosh-core: {err}"),
        })?;
        if !status.success() {
            let error = join_reader_thread(stderr_handle, "cosh-core stderr")?;
            sink(AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: error.trim().to_string(),
            })?;
            return Ok(());
        }

        let _stderr = join_reader_thread(stderr_handle, "cosh-core stderr")?;
        parser.finish(&mut |event| {
            update_completion_flags(&event, &mut completed, &mut failed);
            if is_terminal_agent_event(&event) {
                terminal_events.push(event);
                Ok(())
            } else {
                sink(event)
            }
        })?;
        commit_pending_session_for_scope(
            completed,
            failed,
            &self.session_id,
            &self.session_cwd,
            &pending_session,
            &session_scope_from_request(request),
        );
        for event in terminal_events {
            sink(event)?;
        }
        Ok(())
    }
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

fn session_scope_from_request(request: &AgentRequest) -> String {
    if request.command_block.end_cwd.is_empty() {
        request.command_block.cwd.clone()
    } else {
        request.command_block.end_cwd.clone()
    }
}

pub(super) fn commit_pending_session_for_scope(
    completed: bool,
    failed: bool,
    committed: &Arc<Mutex<Option<String>>>,
    committed_scope: &Arc<Mutex<Option<String>>>,
    pending: &Arc<Mutex<Option<String>>>,
    session_scope: &str,
) {
    if !completed || failed {
        return;
    }
    let Some(pending_id) = pending.lock().ok().and_then(|id| id.clone()) else {
        return;
    };
    if let Ok(mut committed_id) = committed.lock() {
        *committed_id = Some(pending_id);
    }
    if let Ok(mut scope) = committed_scope.lock() {
        *scope = Some(session_scope.to_string());
    }
}

fn start_cancellable_cosh_core_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<Option<String>>>,
    session_cwd: Arc<Mutex<Option<String>>>,
    session_scope: String,
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
        let outcome = run_provider_process_loop(
            run_id.clone(),
            "cosh-core",
            &mut child,
            Arc::clone(&child_pid),
            Arc::clone(&cancelled),
            cancellation_artifacts_for_thread.clone(),
            &sender,
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

        match &outcome {
            ProviderRunOutcome::Cancelled | ProviderRunOutcome::Failed => {
                record_cancellation_pending_session(
                    &cancellation_artifacts_for_thread,
                    "cosh-core",
                    &run_id,
                    pending_session_for_thread
                        .lock()
                        .ok()
                        .and_then(|session| session.clone()),
                );
                return;
            }
            ProviderRunOutcome::Exited {
                status,
                stderr_tail,
            } => {
                if !status.success() {
                    let error = stderr_tail.trim().to_string();
                    send_agent_event(&sender, AgentEvent::AgentFailed { run_id, error });
                    return;
                }
            }
        }

        let _ = parser.finish(&mut |event| {
            update_completion_flags(&event, &mut completed, &mut failed);
            if is_terminal_agent_event(&event) {
                terminal_events.push(event);
            } else {
                send_agent_event(&sender, event);
            }
            Ok(())
        });
        if matches!(outcome, ProviderRunOutcome::Exited { ref status, .. } if status.success()) {
            commit_pending_session_for_scope(
                completed,
                failed,
                &session_state,
                &session_cwd,
                &pending_session_for_thread,
                &session_scope_for_thread,
            );
        }
        for event in terminal_events {
            send_agent_event(&sender, event);
        }
    });

    AgentRunHandle {
        receiver,
        cancel,
        approval_sender: None,
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
            providers,
        } => Some(AgentEvent::AuthRequired {
            run_id: run_id.to_string(),
            request_id,
            reason,
            error_message,
            providers,
        }),
        _ => None,
    }
}
