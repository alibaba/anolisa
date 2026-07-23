use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::types::{AgentEvent, AgentRequest};

use super::claude::{
    is_terminal_agent_event, line_progress, send_agent_event, terminate_process,
    update_completion_flags,
};
use super::cosh_core::question_ingress::{
    classify_output_line, protocol_error, CoreQuestionProtocolReason, CoshCoreOutputClass,
    CoshCoreQuestionGate, QuestionGateDecision,
};
use super::cosh_core::question_writer::QuestionWriter;
use super::cosh_core::{
    commit_pending_session_for_scope, invalidate_resume_on_session_failure, mark_recovery_failure,
    retain_context_session, terminal_events_for_session_commit, SessionResumeAttempt,
    SessionRuntimeState,
};
use super::{
    agent_event_is_provider_progress, control_protocol, record_cancellation_pending_session,
    run_provider_process_loop, spawn_provider_child, AdapterError, AgentRunHandle,
    ApprovalResponse, AuthResponse, ClaudeStreamParser, PreparedInvocation,
    ProviderCancellationArtifactStore, ProviderLineProgress, ProviderPromptArgMode,
    ProviderRunOutcome, ProviderStdinMode,
};

pub(super) fn run_sync_cosh_core_process(
    request: &AgentRequest,
    prepared: &PreparedInvocation,
    session_state: &Arc<Mutex<SessionRuntimeState>>,
    session_scope: &str,
    resume_attempt: &SessionResumeAttempt,
    sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
) -> Result<(), AdapterError> {
    let pending_session = Arc::new(Mutex::new(None));
    let mut observed_resumability = None;
    let mut recovery_finalized = false;
    let result = (|| {
        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting cosh-core headless backend".to_string(),
        })?;

        let mut child = spawn_provider_child(
            prepared,
            "cosh-core",
            ProviderStdinMode::Null,
            ProviderPromptArgMode::TrailingArgIfNonEmpty,
        )?;
        let child_pid = Arc::new(Mutex::new(Some(child.id())));
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation_artifacts = ProviderCancellationArtifactStore::default();
        let mut parser =
            ClaudeStreamParser::new(request.id.clone(), Some(Arc::clone(&pending_session)));
        let mut completed = false;
        let mut failed = false;
        let mut terminal_events = Vec::new();
        let (process_tx, process_rx) = mpsc::channel();
        let outcome = run_provider_process_loop(
            request.id.clone(),
            "cosh-core",
            &mut child,
            child_pid,
            cancelled,
            cancellation_artifacts,
            &process_tx,
            |line| {
                let events = parser.parse_line(&line);
                observed_resumability = parser.session_resumable();
                let progressed = events.iter().any(agent_event_is_provider_progress);
                for event in events {
                    update_completion_flags(&event, &mut completed, &mut failed);
                    if is_terminal_agent_event(&event) {
                        terminal_events.push(event);
                    } else {
                        sink(event)?;
                    }
                }
                Ok(line_progress(progressed))
            },
            || Ok(Vec::new()),
        );
        let (process_events, transport_error) = drain_process_events(&process_rx);
        let transport_failed = matches!(outcome, ProviderRunOutcome::Failed);
        let exit_failure = match outcome {
            ProviderRunOutcome::Cancelled => {
                let _ = commit_pending_session_for_scope(
                    false,
                    true,
                    session_state,
                    &pending_session,
                    session_scope,
                    observed_resumability,
                    resume_attempt,
                );
                recovery_finalized = true;
                for event in process_events {
                    sink(event)?;
                }
                if let Some(error) = transport_error {
                    return Err(error);
                }
                return Ok(());
            }
            ProviderRunOutcome::Failed => None,
            ProviderRunOutcome::Exited {
                status,
                stderr_tail,
            } if !status.success() => Some(exit_failure_message(&status, &stderr_tail)),
            ProviderRunOutcome::Exited { .. } => None,
        };

        let had_terminal_result = !terminal_events.is_empty();
        let finish_result = parser.finish(&mut |event| {
            update_completion_flags(&event, &mut completed, &mut failed);
            if is_terminal_agent_event(&event) {
                terminal_events.push(event);
                Ok(())
            } else {
                sink(event)
            }
        });
        suppress_synthetic_completion_after_transport_failure(
            transport_failed,
            had_terminal_result,
            &mut completed,
            &mut failed,
            &mut terminal_events,
        );
        replace_synthetic_completion_for_nonzero_exit(
            &request.id,
            exit_failure,
            had_terminal_result,
            &mut completed,
            &mut failed,
            &mut terminal_events,
        );
        observed_resumability = parser.session_resumable();
        invalidate_resume_on_session_failure(
            resume_attempt,
            parser.session_error_code(),
            parser.session_error_phase(),
            &terminal_events,
            session_state,
        );
        let retain_session = retain_context_session(&terminal_events, parser.session_error_phase());
        let commit_outcome = commit_pending_session_for_scope(
            completed || retain_session,
            failed && !retain_session,
            session_state,
            &pending_session,
            session_scope,
            observed_resumability,
            resume_attempt,
        );
        recovery_finalized = true;
        for event in
            terminal_events_for_session_commit(&request.id, terminal_events, commit_outcome)
        {
            sink(event)?;
        }
        for event in process_events {
            sink(event)?;
        }
        if let Some(error) = transport_error {
            return Err(error);
        }
        finish_result?;
        Ok(())
    })();

    if result.is_err() && !recovery_finalized {
        let _ = commit_pending_session_for_scope(
            false,
            true,
            session_state,
            &pending_session,
            session_scope,
            observed_resumability,
            resume_attempt,
        );
    }
    result
}

pub(super) fn start_control_protocol_cosh_core_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<SessionRuntimeState>>,
    session_scope: String,
    resume_attempt: SessionResumeAttempt,
) -> AgentRunHandle {
    let (event_tx, event_rx) = mpsc::channel();
    let (approval_tx, approval_rx) = mpsc::channel::<ApprovalResponse>();
    let (answer_confirmation_tx, answer_confirmation_rx) = mpsc::channel();
    let (auth_tx, auth_rx) = mpsc::channel::<AuthResponse>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::new(AtomicBool::new(false));
    let child_pid = Arc::new(Mutex::new(None::<u32>));
    let pending_session = Arc::new(Mutex::new(None));
    let cancellation_artifacts = ProviderCancellationArtifactStore::default();
    let control_capabilities = Arc::new(Mutex::new(
        control_protocol::ControlProtocolCapabilities::default(),
    ));
    let question_gate = Arc::new(Mutex::new(CoshCoreQuestionGate::default()));

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
                message: "starting cosh-core control protocol backend".to_string(),
            },
        );

        let mut child = match spawn_provider_child(
            &prepared,
            "cosh-core",
            ProviderStdinMode::Piped,
            ProviderPromptArgMode::None,
        ) {
            Ok(child) => child,
            Err(err) => {
                let _ = mark_recovery_failure(&session_state, &resume_attempt, &err.message);
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

        let prompt_for_loop = prompt;
        let (writer_failure_tx, writer_failure_rx) = mpsc::channel();
        let writer_thread = QuestionWriter {
            stdin,
            prompt: prompt_for_loop.clone(),
            approval_rx,
            auth_rx,
            done: Arc::clone(&writer_done),
            cancelled: Arc::clone(&cancelled),
            gate: Arc::clone(&question_gate),
            failure_tx: writer_failure_tx,
            answer_confirmation_tx,
        }
        .spawn();

        let mut parser = ClaudeStreamParser::new(
            run_id.clone(),
            Some(Arc::clone(&pending_session_for_thread)),
        );
        let pending_control_tool_call =
            RefCell::new(control_protocol::PendingControlProtocolToolCall::default());
        let control_capabilities_for_loop = Arc::clone(&control_capabilities_for_thread);
        let question_gate_for_loop = Arc::clone(&question_gate);
        let approval_tx_for_loop = approval_tx_for_thread.clone();
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
                match classify_output_line(&line).map_err(protocol_error)? {
                    CoshCoreOutputClass::ValidAskUser(question) => {
                        let decision = question_gate_for_loop
                            .lock()
                            .map_err(|_| {
                                protocol_error(CoreQuestionProtocolReason::InvalidControlShape)
                            })?
                            .accept(&question)
                            .map_err(protocol_error)?;
                        if decision == QuestionGateDecision::Duplicate {
                            return Ok(ProviderLineProgress::NoProgress);
                        }
                        send_agent_event(
                            &event_tx,
                            AgentEvent::UserQuestion {
                                run_id: run_id.clone(),
                                provider_request_id: Some(question.request_id),
                                question: question.question,
                                options: question.options,
                                allow_free_text: question.allow_free_text,
                                selection_mode: question.selection_mode,
                            },
                        );
                        return Ok(ProviderLineProgress::AwaitingApproval);
                    }
                    CoshCoreOutputClass::PassThrough => {}
                }
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
                            hook_requires_approval,
                        } => {
                            let _ = pending_control_tool_call
                                .borrow_mut()
                                .take_matching_control_shell(&run_id, &tool_use_id);
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
                                    hook_requires_approval,
                                },
                            );
                            return Ok(ProviderLineProgress::AwaitingApproval);
                        }
                        control_protocol::ControlRequest::Initialize { request_id } => {
                            let _ = request_id;
                        }
                        control_protocol::ControlRequest::AskUser { .. } => unreachable!(
                            "ask_user is classified before the permissive control parser"
                        ),
                        control_protocol::ControlRequest::AuthRequired {
                            request_id,
                            reason,
                            error_message,
                            providers,
                        } => {
                            send_agent_event(
                                &event_tx,
                                AgentEvent::AuthRequired {
                                    run_id: run_id.clone(),
                                    request_id,
                                    reason,
                                    error_message,
                                    providers,
                                },
                            );
                            return Ok(ProviderLineProgress::AwaitingApproval);
                        }
                        control_protocol::ControlRequest::ShellEvidence {
                            request_id,
                            tool_use_id,
                            action,
                        } => {
                            let _ = pending_control_tool_call
                                .borrow_mut()
                                .take_matching_control_tool_call(&run_id, &tool_use_id);
                            send_agent_event(
                                &event_tx,
                                AgentEvent::ShellEvidenceRequest {
                                    run_id: run_id.clone(),
                                    request_id,
                                    tool_use_id,
                                    action,
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
                    for event in pending_control_tool_call.borrow_mut().stage_or_emit(event) {
                        question_gate_for_loop
                            .lock()
                            .map_err(|_| {
                                protocol_error(CoreQuestionProtocolReason::InvalidControlShape)
                            })?
                            .observe_terminal(&event)
                            .map_err(protocol_error)?;
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
            || {
                if let Ok(error) = writer_failure_rx.try_recv() {
                    return Err(error);
                }
                let events = pending_control_tool_call
                    .borrow_mut()
                    .flush_stalled(control_protocol::PENDING_CONTROL_TOOL_CALL_GRACE);
                for event in events {
                    send_agent_event(&event_tx, event);
                }
                Ok(Vec::new())
            },
        );

        writer_done.store(true, Ordering::SeqCst);
        let _ = writer_thread.join();
        let (process_events, mut transport_error) = drain_process_events(&process_rx);
        if let Ok(error) = writer_failure_rx.try_recv() {
            transport_error = Some(error);
        }
        let transport_failed =
            matches!(outcome, ProviderRunOutcome::Failed) || transport_error.is_some();
        let exit_failure = match &outcome {
            ProviderRunOutcome::Cancelled => {
                writer_done.store(true, Ordering::SeqCst);
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
                    send_agent_event(&event_tx, event);
                }
                if let Some(error) = transport_error {
                    let _ = event_tx.send(Err(error));
                }
                return;
            }
            _ if transport_error.is_some() => None,
            ProviderRunOutcome::Failed => None,
            ProviderRunOutcome::Exited {
                status,
                stderr_tail,
            } if !status.success() => Some(exit_failure_message(status, stderr_tail)),
            ProviderRunOutcome::Exited { .. } => None,
        };

        let had_terminal_result = !terminal_events.is_empty();
        let finish_result = parser.finish(&mut |event| {
            for event in pending_control_tool_call.borrow_mut().stage_or_emit(event) {
                question_gate
                    .lock()
                    .map_err(|_| protocol_error(CoreQuestionProtocolReason::InvalidControlShape))?
                    .observe_terminal(&event)
                    .map_err(protocol_error)?;
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
        let finish_failed = finish_result.is_err();
        suppress_synthetic_completion_after_transport_failure(
            transport_failed || finish_failed,
            had_terminal_result,
            &mut completed,
            &mut failed,
            &mut terminal_events,
        );
        replace_synthetic_completion_for_nonzero_exit(
            &run_id,
            exit_failure.filter(|_| !finish_failed),
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
        let terminal_events =
            terminal_events_for_session_commit(&run_id, terminal_events, commit_outcome);
        for event in terminal_events {
            send_agent_event(&event_tx, event);
        }
        for event in process_events {
            send_agent_event(&event_tx, event);
        }
        if let Some(error) = transport_error.or_else(|| finish_result.err()) {
            let _ = event_tx.send(Err(error));
        }
    });

    AgentRunHandle {
        receiver: event_rx,
        cancel,
        approval_sender: Some(approval_tx),
        question_answer_confirmation: Some(answer_confirmation_rx),
        auth_sender: Some(auth_tx),
        control_capabilities,
        pending_provider_session: Some(pending_session),
        cancellation_artifacts,
    }
}

pub(super) fn exit_failure_message(status: &std::process::ExitStatus, stderr_tail: &str) -> String {
    let stderr = stderr_tail.trim();
    if stderr.is_empty() {
        format!("cosh-core exited with status {status}")
    } else {
        stderr.to_string()
    }
}

pub(super) fn replace_synthetic_completion_for_nonzero_exit(
    run_id: &str,
    exit_failure: Option<String>,
    had_terminal_result: bool,
    completed: &mut bool,
    failed: &mut bool,
    terminal_events: &mut Vec<AgentEvent>,
) {
    let Some(error) = exit_failure.filter(|_| !had_terminal_result) else {
        return;
    };
    // Parser finish synthesizes completion for legacy zero-result providers. A nonzero
    // process exit is authoritative only when no structured terminal result was parsed.
    terminal_events.clear();
    *completed = false;
    *failed = true;
    terminal_events.push(AgentEvent::AgentFailed {
        run_id: run_id.to_string(),
        error,
    });
}

pub(super) fn suppress_synthetic_completion_after_transport_failure(
    transport_failed: bool,
    had_terminal_result: bool,
    completed: &mut bool,
    failed: &mut bool,
    terminal_events: &mut Vec<AgentEvent>,
) {
    if !transport_failed || had_terminal_result {
        return;
    }
    terminal_events.clear();
    *completed = false;
    *failed = true;
}

pub(super) fn drain_process_events(
    receiver: &mpsc::Receiver<Result<AgentEvent, AdapterError>>,
) -> (Vec<AgentEvent>, Option<AdapterError>) {
    let mut events = Vec::new();
    let mut first_error = None;
    for event in receiver.try_iter() {
        match event {
            Ok(event) => events.push(event),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    (events, first_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosh_core_driver_deduplicates_late_shell_evidence_snapshot_result() {
        let mut pending = control_protocol::PendingControlProtocolToolCall::default();

        assert!(pending
            .stage_or_emit(AgentEvent::ToolCall {
                run_id: "run-cosh-core".to_string(),
                tool_id: Some("toolu-evidence".to_string()),
                name: "cosh_shell_evidence".to_string(),
                input: r#"{"action":"list_commands"}"#.to_string(),
            })
            .is_empty());
        assert_eq!(
            pending
                .flush_stalled(control_protocol::PENDING_CONTROL_TOOL_CALL_GRACE)
                .len(),
            0
        );

        let released = pending.flush_stalled(Duration::from_millis(0));
        assert_eq!(released.len(), 1);
        assert!(!pending.take_matching_control_tool_call("run-cosh-core", "toolu-evidence"));
        assert!(pending
            .stage_or_emit(AgentEvent::ToolCompleted {
                run_id: "run-cosh-core".to_string(),
                tool_id: "toolu-evidence".to_string(),
                status: "success".to_string(),
            })
            .is_empty());
    }
}
