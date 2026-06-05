use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

use super::claude::{
    is_terminal_agent_event, join_reader_thread, line_progress, read_lossy, send_agent_event,
    terminate_process, update_completion_flags,
};
use super::{
    agent_event_is_provider_progress, control_protocol, prompt_from_request,
    provider_prompt_contract, record_cancellation_pending_session, run_provider_process_loop,
    spawn_provider_child, start_threaded_adapter_run, AdapterError, AdapterInstance, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, ApprovalDecision, ApprovalResponse,
    ClaudeStreamParser, PreparedInvocation, ProviderCancellationArtifactStore,
    ProviderLineProgress, ProviderPromptArgMode, ProviderRunOutcome, ProviderStdinMode,
};

#[derive(Debug, Clone)]
pub struct CoshTuiAdapter {
    pub program: String,
    pub allow_model_call: bool,
    pub session_id: Arc<Mutex<Option<String>>>,
    pub session_cwd: Arc<Mutex<Option<String>>>,
}

impl Default for CoshTuiAdapter {
    fn default() -> Self {
        let program = std::env::var("COSH_TUI_PATH").unwrap_or_else(|_| {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let sibling = dir.join("cosh-tui");
                    if sibling.is_file() {
                        return sibling.to_string_lossy().into_owned();
                    }
                }
            }
            "cosh-tui".to_string()
        });
        Self {
            program,
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(None)),
            session_cwd: Arc::new(Mutex::new(None)),
        }
    }
}

impl CoshTuiAdapter {
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
            "--approval-mode".to_string(),
            approval_mode.to_string(),
        ];

        if let Some(session_id) = resume_session {
            args.extend(["--resume".to_string(), session_id]);
        }

        PreparedInvocation {
            program: self.program.clone(),
            args,
            prompt: cosh_tui_prompt_from_request(request, mode),
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
            let adapter = AdapterInstance::CoshTui(self.clone());
            return start_threaded_adapter_run(adapter, request);
        }

        if mode.uses_control_protocol() {
            return start_control_protocol_cosh_tui_process(
                request.id,
                prepared,
                Arc::clone(&self.session_id),
                Arc::clone(&self.session_cwd),
                session_scope,
            );
        }

        start_cancellable_cosh_tui_process(
            request.id,
            prepared,
            Arc::clone(&self.session_id),
            Arc::clone(&self.session_cwd),
            session_scope,
        )
    }
}

impl AgentAdapter for CoshTuiAdapter {
    fn name(&self) -> &'static str {
        "cosh-tui"
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
            for event in cosh_tui_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }

        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting cosh-tui headless backend".to_string(),
        })?;

        let mut child = Command::new(&prepared.program)
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AdapterError {
                message: format!("failed to run cosh-tui: {err}"),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| AdapterError {
            message: "failed to capture cosh-tui stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| AdapterError {
            message: "failed to capture cosh-tui stderr".to_string(),
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
                message: format!("failed to read cosh-tui stream: {err}"),
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
            message: format!("failed to wait for cosh-tui: {err}"),
        })?;
        if !status.success() {
            let error = join_reader_thread(stderr_handle, "cosh-tui stderr")?;
            sink(AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: error.trim().to_string(),
            })?;
            return Ok(());
        }

        let _stderr = join_reader_thread(stderr_handle, "cosh-tui stderr")?;
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

fn cosh_tui_prompt_from_request(request: &AgentRequest, mode: CoshApprovalMode) -> String {
    format!(
        "{}{}\n\n\
         cosh-tui adapter compatibility:\n\
         - Use the shell tool to gather evidence when the cosh-shell Agent contract asks for shell evidence.\n\
         - Do not answer only with a suggested shell command when the shell tool can gather the evidence.\n\
         - If host-executed shell results are required, cosh-tui must support that control response before running the shell itself.",
        prompt_from_request(request),
        provider_prompt_contract(mode, "shell")
    )
}

fn cosh_tui_dry_run_events(
    request: &AgentRequest,
    prepared: &PreparedInvocation,
) -> Vec<AgentEvent> {
    vec![
        AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "prepared".to_string(),
            message: format!(
                "cosh-tui invocation prepared: {} {}",
                prepared.program,
                prepared.args.join(" ")
            ),
        },
        AgentEvent::Recommendation {
            run_id: request.id.clone(),
            summary: "cosh-tui adapter is configured but model calls are disabled in dry-run mode."
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

fn commit_pending_session_for_scope(
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

fn start_cancellable_cosh_tui_process(
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
                message: "starting cosh-tui headless backend".to_string(),
            },
        );

        let mut child = match spawn_provider_child(
            &prepared,
            "cosh-tui",
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
            "cosh-tui",
            &mut child,
            Arc::clone(&child_pid),
            Arc::clone(&cancelled),
            cancellation_artifacts_for_thread.clone(),
            &sender,
            |line| {
                let events = parser.parse_line(&line);
                let progressed = events.iter().any(agent_event_is_provider_progress);
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
        );

        match &outcome {
            ProviderRunOutcome::Cancelled | ProviderRunOutcome::Failed => {
                record_cancellation_pending_session(
                    &cancellation_artifacts_for_thread,
                    "cosh-tui",
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
        control_capabilities: Arc::new(Mutex::new(
            control_protocol::ControlProtocolCapabilities::default(),
        )),
        pending_provider_session: Some(pending_session),
        cancellation_artifacts,
    }
}

fn start_control_protocol_cosh_tui_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<Option<String>>>,
    session_cwd: Arc<Mutex<Option<String>>>,
    session_scope: String,
) -> AgentRunHandle {
    let (event_tx, event_rx) = mpsc::channel();
    let (approval_tx, approval_rx) = mpsc::channel::<ApprovalResponse>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::new(AtomicBool::new(false));
    let child_pid = Arc::new(Mutex::new(None::<u32>));
    let pending_session = Arc::new(Mutex::new(None));
    let cancellation_artifacts = ProviderCancellationArtifactStore::default();
    let control_capabilities = Arc::new(Mutex::new(
        control_protocol::ControlProtocolCapabilities::default(),
    ));

    let cancel_flag = Arc::clone(&cancelled);
    let cancel_pid = Arc::clone(&child_pid);
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(pid) = cancel_pid.lock().ok().and_then(|guard| *guard) {
            terminate_process(pid);
        }
    });

    let prompt = prepared.prompt.clone();
    let pending_session_for_thread = Arc::clone(&pending_session);
    let session_scope_for_thread = session_scope;
    let cancellation_artifacts_for_thread = cancellation_artifacts.clone();
    let control_capabilities_for_thread = Arc::clone(&control_capabilities);
    let approval_tx_for_thread = approval_tx.clone();
    thread::spawn(move || {
        send_agent_event(
            &event_tx,
            AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "starting".to_string(),
                message: "starting cosh-tui control protocol backend".to_string(),
            },
        );

        let mut child = match spawn_provider_child(
            &prepared,
            "cosh-tui",
            ProviderStdinMode::Piped,
            ProviderPromptArgMode::None,
        ) {
            Ok(child) => child,
            Err(err) => {
                let _ = event_tx.send(Err(err));
                return;
            }
        };

        if let Ok(mut pid) = child_pid.lock() {
            *pid = Some(child.id());
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_process(child.id());
        }

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = event_tx.send(Err(AdapterError {
                    message: "failed to capture stdin".to_string(),
                }));
                return;
            }
        };

        let writer_done_for_thread = Arc::clone(&writer_done);
        let writer_cancelled = Arc::clone(&cancelled);
        let prompt_for_writer = prompt.clone();
        let prompt_for_loop = prompt;
        thread::spawn(move || {
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(stdin);

            let init_msg = control_protocol::serialize_initialize("init-1");
            let _ = writeln!(writer, "{init_msg}");
            let _ = writer.flush();

            if !prompt_for_writer.is_empty() {
                let user_msg = control_protocol::serialize_user_message(&prompt_for_writer, None);
                let _ = writeln!(writer, "{user_msg}");
                let _ = writer.flush();
            }

            while !writer_done_for_thread.load(Ordering::SeqCst)
                && !writer_cancelled.load(Ordering::SeqCst)
            {
                let response = match approval_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(response) => response,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let msg = match &response.decision {
                    ApprovalDecision::Allow => {
                        control_protocol::serialize_co_allow(&response.request_id)
                    }
                    ApprovalDecision::Deny { message } => {
                        control_protocol::serialize_deny(&response.request_id, message)
                    }
                    ApprovalDecision::HostExecutedShell { result } => {
                        control_protocol::serialize_host_executed_shell_result(
                            &response.request_id,
                            result,
                        )
                    }
                    ApprovalDecision::Answer { answer } => {
                        control_protocol::serialize_answer(&response.request_id, answer)
                    }
                };
                if writeln!(writer, "{msg}").is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        let mut parser = ClaudeStreamParser::new(
            run_id.clone(),
            Some(Arc::clone(&pending_session_for_thread)),
        );
        let mut pending_control_tool_call =
            control_protocol::PendingControlProtocolToolCall::default();
        let control_capabilities_for_loop = Arc::clone(&control_capabilities_for_thread);
        let approval_tx_for_loop = approval_tx_for_thread.clone();
        let mut completed = false;
        let mut failed = false;
        let mut terminal_events = Vec::new();
        let outcome = run_provider_process_loop(
            run_id.clone(),
            "cosh-tui",
            &mut child,
            Arc::clone(&child_pid),
            Arc::clone(&cancelled),
            cancellation_artifacts_for_thread.clone(),
            &event_tx,
            |line| {
                if let Some(capabilities) = control_protocol::parse_initialize_capabilities(&line) {
                    if let Ok(mut current) = control_capabilities_for_loop.lock() {
                        *current = capabilities;
                    }
                    return Ok(ProviderLineProgress::NoProgress);
                }

                if let Some(ctrl) = control_protocol::parse_control_request(&line) {
                    match ctrl {
                        control_protocol::ControlRequest::CanUseTool {
                            request_id,
                            tool_name,
                            tool_input,
                            tool_use_id,
                        } => {
                            let _ =
                                pending_control_tool_call.take_matching_control_shell(&tool_use_id);
                            if let Some(response) =
                                control_protocol::analysis_continuation_shell_deny_response(
                                    &prompt_for_loop,
                                    &request_id,
                                    &tool_name,
                                    &tool_input,
                                    &tool_use_id,
                                )
                            {
                                let _ = approval_tx_for_loop.send(response);
                                return Ok(ProviderLineProgress::AwaitingApproval);
                            }
                            send_agent_event(
                                &event_tx,
                                AgentEvent::ToolPermissionRequest {
                                    run_id: run_id.clone(),
                                    request_id,
                                    tool_name,
                                    tool_input,
                                    tool_use_id,
                                },
                            );
                            return Ok(ProviderLineProgress::AwaitingApproval);
                        }
                        control_protocol::ControlRequest::Initialize { request_id } => {
                            let _ = request_id;
                        }
                        control_protocol::ControlRequest::AskUser {
                            request_id,
                            question,
                            options,
                            allow_free_text,
                            selection_mode,
                        } => {
                            send_agent_event(
                                &event_tx,
                                AgentEvent::UserQuestion {
                                    run_id: run_id.clone(),
                                    provider_request_id: Some(request_id),
                                    question,
                                    options,
                                    allow_free_text,
                                    selection_mode,
                                },
                            );
                            return Ok(ProviderLineProgress::AwaitingApproval);
                        }
                    }
                    return Ok(ProviderLineProgress::NoProgress);
                }

                let events = parser.parse_line(&line);
                let progressed = events.iter().any(agent_event_is_provider_progress);
                for event in events {
                    for event in pending_control_tool_call.stage_or_emit(event) {
                        update_completion_flags(&event, &mut completed, &mut failed);
                        if is_terminal_agent_event(&event) {
                            writer_done.store(true, Ordering::SeqCst);
                            terminal_events.push(event);
                        } else {
                            send_agent_event(&event_tx, event);
                        }
                    }
                }
                Ok(line_progress(progressed))
            },
        );

        match &outcome {
            ProviderRunOutcome::Cancelled | ProviderRunOutcome::Failed => {
                writer_done.store(true, Ordering::SeqCst);
                record_cancellation_pending_session(
                    &cancellation_artifacts_for_thread,
                    "cosh-tui",
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
                    writer_done.store(true, Ordering::SeqCst);
                    let error = stderr_tail.trim().to_string();
                    send_agent_event(&event_tx, AgentEvent::AgentFailed { run_id, error });
                    return;
                }
            }
        }

        let _ = parser.finish(&mut |event| {
            for event in pending_control_tool_call.stage_or_emit(event) {
                update_completion_flags(&event, &mut completed, &mut failed);
                if is_terminal_agent_event(&event) {
                    writer_done.store(true, Ordering::SeqCst);
                    terminal_events.push(event);
                } else {
                    send_agent_event(&event_tx, event);
                }
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
            send_agent_event(&event_tx, event);
        }
    });

    AgentRunHandle {
        receiver: event_rx,
        cancel,
        approval_sender: Some(approval_tx),
        control_capabilities,
        pending_provider_session: Some(pending_session),
        cancellation_artifacts,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::CoshTuiAdapter;
    use crate::adapter::AgentAdapter;
    use crate::types::{
        AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
    };

    fn test_request() -> AgentRequest {
        AgentRequest {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            command_block: CommandBlock {
                id: "blk".to_string(),
                session_id: "sess".to_string(),
                command: "echo test".to_string(),
                origin: Default::default(),
                cwd: "/tmp".to_string(),
                end_cwd: "/tmp".to_string(),
                started_at_ms: 0,
                ended_at_ms: 0,
                duration_ms: 0,
                exit_code: 1,
                status: CommandStatus::Failed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
            },
            context_blocks: vec![],
            context_hints: vec![],
            user_input: Some("test".to_string()),
            findings: vec![],
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }

    fn test_adapter() -> CoshTuiAdapter {
        CoshTuiAdapter {
            program: "cosh-tui".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(None)),
            session_cwd: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn prepare_invocation_headless_flag() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert_eq!(inv.program, "cosh-tui");
        assert!(inv.args.contains(&"--headless".to_string()));
    }

    #[test]
    fn prepare_invocation_approval_modes() {
        let recommend =
            test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Recommend);
        assert!(recommend.args.contains(&"strict".to_string()));

        let auto = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(auto.args.contains(&"auto".to_string()));

        let trust = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
        assert!(trust.args.contains(&"trust".to_string()));
    }

    #[test]
    fn prepare_invocation_session_resume() {
        let adapter = CoshTuiAdapter {
            program: "cosh-tui".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
            session_cwd: Arc::new(Mutex::new(Some("/tmp".to_string()))),
        };
        let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(inv.args.contains(&"--resume".to_string()));
        assert!(inv.args.contains(&"prev-sess".to_string()));
    }

    #[test]
    fn prepare_invocation_does_not_resume_across_cwd_scope() {
        let adapter = CoshTuiAdapter {
            program: "cosh-tui".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
            session_cwd: Arc::new(Mutex::new(Some("/other".to_string()))),
        };
        let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(!inv.args.contains(&"--resume".to_string()));
        assert!(!inv.args.contains(&"prev-sess".to_string()));
    }

    #[test]
    fn capabilities_match_expected() {
        let adapter = test_adapter();
        let caps = adapter.capabilities();
        assert!(caps.text_stream);
        assert!(caps.session_resume);
        assert!(caps.tool_intent);
        assert!(caps.user_question);
        assert!(caps.cancellable);
        assert!(caps.control_protocol);
    }
}
