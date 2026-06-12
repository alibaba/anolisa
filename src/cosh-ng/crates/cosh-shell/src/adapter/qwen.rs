use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use nix::libc;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

use super::claude::{join_reader_thread, read_lossy, send_agent_event, terminate_process};
use super::qwen_stream::QwenStreamParser;
use super::{
    prompt_from_request, provider_prompt_contract, start_threaded_adapter_run, AdapterError,
    AdapterInstance, AgentAdapter, AgentBackendCapabilities, AgentRunHandle, PreparedInvocation,
};

#[derive(Debug, Clone)]
pub struct QwenCliAdapter {
    pub program: String,
    pub allow_model_call: bool,
    pub session_id: Arc<Mutex<Option<String>>>,
}

impl Default for QwenCliAdapter {
    fn default() -> Self {
        Self {
            program: "co".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(None)),
        }
    }
}

impl QwenCliAdapter {
    pub fn with_model_call(mut self, allow: bool) -> Self {
        self.allow_model_call = allow;
        self
    }

    pub fn prepare_invocation(
        &self,
        request: &AgentRequest,
        mode: CoshApprovalMode,
    ) -> PreparedInvocation {
        let resume_session = self.session_id.lock().ok().and_then(|guard| guard.clone());
        let mut args = vec![
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--include-partial-messages".to_string(),
        ];

        args.extend(["--input-format".to_string(), "stream-json".to_string()]);
        match mode {
            CoshApprovalMode::Suggest => {
                args.extend(["--approval-mode".to_string(), "plan".to_string()]);
            }
            CoshApprovalMode::Ask => {
                args.extend(["--approval-mode".to_string(), "default".to_string()]);
            }
            CoshApprovalMode::Trust => {
                args.extend(["--approval-mode".to_string(), "yolo".to_string()]);
            }
            CoshApprovalMode::Auto => {
                args.extend(["--approval-mode".to_string(), "default".to_string()]);
                args.extend([
                    "--allowed-tools".to_string(),
                    "Read,Grep,Glob,LS,read_file,grep_search,glob,list_directory,read_many_files"
                        .to_string(),
                ]);
            }
        }

        if let Some(session_id) = resume_session {
            args.push("--resume".to_string());
            args.push(session_id);
        }

        PreparedInvocation {
            program: self.program.clone(),
            args,
            prompt: qwen_prompt_from_request(request, mode),
        }
    }

    pub fn start_cancellable(
        &self,
        request: AgentRequest,
        mode: CoshApprovalMode,
    ) -> AgentRunHandle {
        let prepared = self.prepare_invocation(&request, mode);
        if !self.allow_model_call {
            let adapter = AdapterInstance::QwenCli(self.clone());
            return start_threaded_adapter_run(adapter, request);
        }

        if !mode.uses_control_protocol() {
            return start_cancellable_qwen_process(
                request.id,
                prepared,
                Arc::clone(&self.session_id),
            );
        }

        start_control_protocol_qwen_process(request.id, prepared, Arc::clone(&self.session_id))
    }
}

impl AgentAdapter for QwenCliAdapter {
    fn name(&self) -> &'static str {
        "qwen-cli"
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
        let prepared = self.prepare_invocation(request, CoshApprovalMode::Suggest);
        if !self.allow_model_call {
            for event in qwen_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }

        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting qwen-cli stream-json backend".to_string(),
        })?;

        let mut child = Command::new(&prepared.program)
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AdapterError {
                message: format!("failed to run qwen cli: {err}"),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| AdapterError {
            message: "failed to capture qwen cli stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| AdapterError {
            message: "failed to capture qwen cli stderr".to_string(),
        })?;
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let mut parser =
            QwenStreamParser::new(request.id.clone(), Some(Arc::clone(&self.session_id)));
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|err| AdapterError {
                message: format!("failed to read qwen cli stream: {err}"),
            })?;
            for event in parser.parse_line(&line) {
                sink(event)?;
            }
        }

        let status = child.wait().map_err(|err| AdapterError {
            message: format!("failed to wait for qwen cli: {err}"),
        })?;
        if !status.success() {
            let error = join_reader_thread(stderr_handle, "qwen cli stderr")?;
            sink(AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: error.trim().to_string(),
            })?;
            return Ok(());
        }

        let _stderr = join_reader_thread(stderr_handle, "qwen cli stderr")?;
        parser.finish(sink)
    }
}

fn qwen_prompt_from_request(request: &AgentRequest, mode: CoshApprovalMode) -> String {
    format!(
        "{}{}\
         \n\n\
         Qwen adapter compatibility:\n\
         - When the cosh-shell Agent contract asks for shell evidence, use co's `run_shell_command` tool.\n\
         - Do not answer only with a suggested shell command in agent mode when `run_shell_command` can gather the evidence.\n\
         If you need to ask the user for more input and an AskUserQuestion tool is not available, \
         output exactly one line and no surrounding prose:\n\
         COSH_QUESTION: {{\"question\":\"<visible question>\",\"options\":[\"option 1\",\"option 2\"],\"allow_free_text\":true,\"multi_select\":false}}\n\
         Use an empty options array for free-text-only questions. Never say AskUserQuestion is unavailable.",
        prompt_from_request(request),
        provider_prompt_contract(mode, "run_shell_command")
    )
}

fn start_cancellable_qwen_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<Option<String>>>,
) -> AgentRunHandle {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let child_pid = Arc::new(Mutex::new(None::<u32>));

    let cancel_flag = Arc::clone(&cancelled);
    let cancel_pid = Arc::clone(&child_pid);
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(pid) = cancel_pid.lock().ok().and_then(|guard| *guard) {
            terminate_process(pid);
        }
    });

    thread::spawn(move || {
        send_agent_event(
            &sender,
            AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "starting".to_string(),
                message: "starting qwen-cli stream-json backend".to_string(),
            },
        );

        let mut command = Command::new(&prepared.program);
        command
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = sender.send(Err(AdapterError {
                    message: format!("failed to run qwen cli: {err}"),
                }));
                return;
            }
        };

        if let Ok(mut pid) = child_pid.lock() {
            *pid = Some(child.id());
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_process(child.id());
        }

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = sender.send(Err(AdapterError {
                    message: "failed to capture qwen cli stdout".to_string(),
                }));
                return;
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = sender.send(Err(AdapterError {
                    message: "failed to capture qwen cli stderr".to_string(),
                }));
                return;
            }
        };
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let mut parser = QwenStreamParser::new(run_id.clone(), Some(session_state));
        for line in BufReader::new(stdout).lines() {
            if cancelled.load(Ordering::SeqCst) {
                terminate_process(child.id());
                break;
            }
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    let _ = sender.send(Err(AdapterError {
                        message: format!("failed to read qwen cli stream: {err}"),
                    }));
                    return;
                }
            };
            for event in parser.parse_line(&line) {
                send_agent_event(&sender, event);
            }
        }

        let status = match child.wait() {
            Ok(status) => status,
            Err(err) => {
                let _ = sender.send(Err(AdapterError {
                    message: format!("failed to wait for qwen cli: {err}"),
                }));
                return;
            }
        };
        if let Ok(mut pid) = child_pid.lock() {
            *pid = None;
        }

        if cancelled.load(Ordering::SeqCst) {
            let _stderr = join_reader_thread(stderr_handle, "qwen cli stderr");
            send_agent_event(
                &sender,
                AgentEvent::AgentCancelled {
                    run_id,
                    reason: "user requested cancellation".to_string(),
                },
            );
            return;
        }

        if !status.success() {
            let error = join_reader_thread(stderr_handle, "qwen cli stderr")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|e| e.message);
            send_agent_event(&sender, AgentEvent::AgentFailed { run_id, error });
            return;
        }

        let _stderr = join_reader_thread(stderr_handle, "qwen cli stderr");
        let _ = parser.finish(&mut |event| {
            send_agent_event(&sender, event);
            Ok(())
        });
    });

    AgentRunHandle {
        receiver,
        cancel,
        approval_sender: None,
        question_sender: None,
    }
}

fn start_control_protocol_qwen_process(
    run_id: String,
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<Option<String>>>,
) -> AgentRunHandle {
    let (event_tx, event_rx) = mpsc::channel();
    let (approval_tx, approval_rx) = mpsc::channel::<super::ApprovalResponse>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let child_pid = Arc::new(Mutex::new(None::<u32>));

    let cancel_flag = Arc::clone(&cancelled);
    let cancel_pid = Arc::clone(&child_pid);
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(pid) = cancel_pid.lock().ok().and_then(|guard| *guard) {
            terminate_process(pid);
        }
    });

    let prompt = prepared.prompt.clone();

    thread::spawn(move || {
        send_agent_event(
            &event_tx,
            AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "starting".to_string(),
                message: "starting qwen-cli control protocol backend".to_string(),
            },
        );

        let mut command = Command::new(&prepared.program);
        command
            .args(&prepared.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = event_tx.send(Err(AdapterError {
                    message: format!("failed to run qwen cli: {err}"),
                }));
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
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = event_tx.send(Err(AdapterError {
                    message: "failed to capture stdout".to_string(),
                }));
                return;
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = event_tx.send(Err(AdapterError {
                    message: "failed to capture stderr".to_string(),
                }));
                return;
            }
        };

        // stderr reader thread
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        // stdin writer thread
        thread::spawn(move || {
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(stdin);

            let init_msg = super::control_protocol::serialize_initialize("init-1");
            let _ = writeln!(writer, "{init_msg}");
            let _ = writer.flush();

            if !prompt.is_empty() {
                let user_msg = super::control_protocol::serialize_user_message(&prompt, None);
                let _ = writeln!(writer, "{user_msg}");
                let _ = writer.flush();
            }

            while let Ok(response) = approval_rx.recv() {
                let msg = match &response.decision {
                    super::ApprovalDecision::Allow => match response.tool_use_id.as_deref() {
                        Some(tool_use_id) => super::control_protocol::serialize_allow(
                            &response.request_id,
                            tool_use_id,
                        ),
                        None => super::control_protocol::serialize_deny(
                            &response.request_id,
                            "Missing provider tool_use_id",
                        ),
                    },
                    super::ApprovalDecision::Deny { message } => {
                        super::control_protocol::serialize_deny(&response.request_id, message)
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

        // stdout reader — parse events + control requests
        let mut parser = QwenStreamParser::new(run_id.clone(), Some(session_state));
        for line in BufReader::new(stdout).lines() {
            if cancelled.load(Ordering::SeqCst) {
                terminate_process(child.id());
                break;
            }
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    let _ = event_tx.send(Err(AdapterError {
                        message: format!("failed to read stream: {err}"),
                    }));
                    return;
                }
            };

            if let Some(ctrl) = super::control_protocol::parse_control_request(&line) {
                match ctrl {
                    super::control_protocol::ControlRequest::CanUseTool {
                        request_id,
                        tool_name,
                        tool_input,
                        tool_use_id,
                    } => {
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
                    }
                    super::control_protocol::ControlRequest::Initialize { .. } => {}
                    super::control_protocol::ControlRequest::AskUser { .. } => {}
                }
                continue;
            }

            for event in parser.parse_line(&line) {
                send_agent_event(&event_tx, event);
            }
        }

        let status = match child.wait() {
            Ok(status) => status,
            Err(err) => {
                let _ = event_tx.send(Err(AdapterError {
                    message: format!("failed to wait for qwen cli: {err}"),
                }));
                return;
            }
        };
        if let Ok(mut pid) = child_pid.lock() {
            *pid = None;
        }

        if cancelled.load(Ordering::SeqCst) {
            let _stderr = join_reader_thread(stderr_handle, "stderr");
            send_agent_event(
                &event_tx,
                AgentEvent::AgentCancelled {
                    run_id,
                    reason: "user requested cancellation".to_string(),
                },
            );
            return;
        }

        if !status.success() {
            let error = join_reader_thread(stderr_handle, "stderr")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|e| e.message);
            send_agent_event(&event_tx, AgentEvent::AgentFailed { run_id, error });
            return;
        }

        let _stderr = join_reader_thread(stderr_handle, "stderr");
        let _ = parser.finish(&mut |event| {
            send_agent_event(&event_tx, event);
            Ok(())
        });
    });

    AgentRunHandle {
        receiver: event_rx,
        cancel,
        approval_sender: Some(approval_tx),
        question_sender: None,
    }
}

fn qwen_dry_run_events(request: &AgentRequest, prepared: &PreparedInvocation) -> Vec<AgentEvent> {
    let run_id = format!("qwen-dry-run-{}", request.command_block.id);
    vec![
        AgentEvent::StatusChanged {
            run_id: run_id.clone(),
            phase: "dry_run".to_string(),
            message: "prepared qwen cli invocation without model call".to_string(),
        },
        AgentEvent::TextDelta {
            run_id: run_id.clone(),
            text: format!(
                "Qwen CLI adapter prepared a safe recommend-only invocation; model execution is disabled by default.\n\nPrepared invocation:\n  {}",
                prepared.argv_preview().join(" ")
            ),
        },
        AgentEvent::AgentCompleted {
            run_id,
            summary: "qwen cli dry-run completed without model call".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{start_cancellable_qwen_process, PreparedInvocation, QwenCliAdapter};
    use crate::adapter::AgentRunHandle;
    use crate::types::{
        AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode,
        OutputRefs,
    };

    fn test_request() -> AgentRequest {
        AgentRequest {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            command_block: CommandBlock {
                id: "blk".to_string(),
                session_id: "sess".to_string(),
                command: "echo test".to_string(),
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

    fn test_adapter() -> QwenCliAdapter {
        QwenCliAdapter {
            program: "qwen".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn mode_flags_co_suggest() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Suggest);
        assert!(inv.args.contains(&"--approval-mode".to_string()));
        assert!(inv.args.contains(&"plan".to_string()));
        assert!(inv.args.contains(&"--input-format".to_string()));
        assert!(inv.args.contains(&"stream-json".to_string()));
        assert!(inv.prompt.contains("recommend mode"), "{}", inv.prompt);
        assert!(
            inv.prompt.contains("do not emit tool calls"),
            "{}",
            inv.prompt
        );
        assert!(inv.prompt.contains("run_shell_command"), "{}", inv.prompt);
        assert!(!inv.args.contains(&"--allowed-tools".to_string()));
    }

    #[test]
    fn mode_flags_co_ask() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert!(inv.args.contains(&"--approval-mode".to_string()));
        assert!(inv.args.contains(&"default".to_string()));
        assert!(inv.args.contains(&"--input-format".to_string()));
        assert!(inv.args.contains(&"stream-json".to_string()));
        assert!(inv.prompt.contains("COSH_QUESTION:"));
        assert!(inv
            .prompt
            .contains("Never say AskUserQuestion is unavailable"));
        assert!(inv.prompt.contains("agent mode"), "{}", inv.prompt);
        assert!(inv
            .prompt
            .contains("approval system is handled by cosh-shell"));
        assert!(inv.prompt.contains("run_shell_command"), "{}", inv.prompt);
        assert!(!inv.args.contains(&"--verbose".to_string()));
        assert!(!inv.args.contains(&"--permission-prompt-tool".to_string()));
    }

    #[test]
    fn mode_flags_co_auto() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(inv.args.contains(&"--allowed-tools".to_string()));
        assert!(inv
            .args
            .iter()
            .any(|arg| arg.contains("read_file") && arg.contains("grep_search")));
        assert!(!inv.args.contains(&"--allowedTools".to_string()));
    }

    #[test]
    fn mode_flags_co_trust() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
        assert!(inv.args.contains(&"--approval-mode".to_string()));
        assert!(inv.args.contains(&"yolo".to_string()));
        assert!(inv.args.contains(&"--input-format".to_string()));
    }

    #[test]
    fn mode_flags_co_session_resume() {
        let adapter = QwenCliAdapter {
            program: "qwen".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        };
        let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert!(inv.args.contains(&"--resume".to_string()));
        assert!(inv.args.contains(&"prev-sess".to_string()));
    }

    #[test]
    fn cancellable_qwen_process_emits_cancelled_event() {
        let handle: AgentRunHandle = start_cancellable_qwen_process(
            "run-cancel".to_string(),
            PreparedInvocation {
                program: "/bin/sleep".to_string(),
                args: vec!["10".to_string()],
                prompt: String::new(),
            },
            Arc::new(Mutex::new(None)),
        );

        assert!(matches!(
            handle
                .next_event_timeout(Duration::from_secs(1))
                .expect("starting event"),
            Some(AgentEvent::StatusChanged { phase, .. }) if phase == "starting"
        ));
        handle.cancel();

        let mut saw_cancelled = false;
        for _ in 0..10 {
            if matches!(
                handle
                    .next_event_timeout(Duration::from_millis(300))
                    .expect("cancel event"),
                Some(AgentEvent::AgentCancelled { .. })
            ) {
                saw_cancelled = true;
                break;
            }
        }
        assert!(saw_cancelled);
    }
}
