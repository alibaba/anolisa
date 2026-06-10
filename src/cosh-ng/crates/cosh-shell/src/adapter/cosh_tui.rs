use std::io::BufRead;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use nix::libc;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

use super::claude::{join_reader_thread, read_lossy};
use super::{
    prompt_from_request, provider_prompt_contract, AdapterError, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, ClaudeStreamParser, PreparedInvocation,
};

struct PersistentProcess {
    #[allow(dead_code)]
    child_pid: u32,
    stdin_tx: mpsc::Sender<StdinCommand>,
    active_run: Arc<Mutex<Option<ActiveRunState>>>,
    alive: Arc<AtomicBool>,
}

impl std::fmt::Debug for PersistentProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentProcess")
            .field("child_pid", &self.child_pid)
            .field("alive", &self.alive.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

struct ActiveRunState {
    run_id: String,
    event_tx: mpsc::Sender<Result<AgentEvent, AdapterError>>,
    cancelled: Arc<AtomicBool>,
}

enum StdinCommand {
    SendLine(String),
}

#[derive(Debug, Clone)]
pub struct CoshTuiAdapter {
    pub program: String,
    pub session_id: Arc<Mutex<Option<String>>>,
    persistent: Arc<Mutex<Option<PersistentProcess>>>,
}

impl Default for CoshTuiAdapter {
    fn default() -> Self {
        let program = std::env::var("COSH_TUI_PATH").unwrap_or_else(|_| "cosh-tui".to_string());
        Self {
            program,
            session_id: Arc::new(Mutex::new(None)),
            persistent: Arc::new(Mutex::new(None)),
        }
    }
}

impl CoshTuiAdapter {
    pub fn prepare_invocation(
        &self,
        request: &AgentRequest,
        mode: CoshApprovalMode,
    ) -> PreparedInvocation {
        let resume_session = self.session_id.lock().ok().and_then(|guard| guard.clone());
        let mut args = vec!["--headless".to_string()];

        let approval_mode = match mode {
            CoshApprovalMode::Suggest => "strict",
            CoshApprovalMode::Ask => "balanced",
            CoshApprovalMode::Auto => "auto",
            CoshApprovalMode::Trust => "trust",
        };
        args.extend([
            "--approval-mode".to_string(),
            approval_mode.to_string(),
        ]);

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
        let mut persistent = self.persistent.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(proc) = persistent.as_ref() {
            if proc.alive.load(Ordering::SeqCst) {
                return attach_run(proc, &request, mode, Arc::clone(&self.session_id));
            }
            *persistent = None;
        }

        let prepared = self.prepare_invocation(&request, mode);
        let (proc, handle) =
            spawn_persistent(prepared, Arc::clone(&self.session_id), &request, mode);
        *persistent = Some(proc);
        handle
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
        let prepared = self.prepare_invocation(request, CoshApprovalMode::Suggest);

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

        let mut parser =
            ClaudeStreamParser::new(request.id.clone(), Some(Arc::clone(&self.session_id)));
        for line in std::io::BufReader::new(stdout).lines() {
            let line = line.map_err(|err| AdapterError {
                message: format!("failed to read cosh-tui stream: {err}"),
            })?;
            for event in parser.parse_line(&line) {
                sink(event)?;
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
        parser.finish(sink)
    }
}

fn cosh_tui_prompt_from_request(request: &AgentRequest, mode: CoshApprovalMode) -> String {
    format!(
        "{}{}\n\n\
         cosh-tui adapter compatibility:\n\
         - Use the shell tool to gather evidence when the cosh-shell Agent contract asks for shell evidence.\n\
         - Do not answer only with a suggested shell command when the shell tool can gather the evidence.\n\
         If you need to ask the user for more input and an AskUserQuestion tool is not available, \
         output exactly one line and no surrounding prose:\n\
         COSH_QUESTION: {{\"question\":\"<visible question>\",\"options\":[\"option 1\",\"option 2\"],\"allow_free_text\":true,\"multi_select\":false}}\n\
         Use an empty options array for free-text-only questions.",
        prompt_from_request(request),
        provider_prompt_contract(mode, "shell")
    )
}

fn spawn_persistent(
    prepared: PreparedInvocation,
    session_state: Arc<Mutex<Option<String>>>,
    request: &AgentRequest,
    mode: CoshApprovalMode,
) -> (PersistentProcess, AgentRunHandle) {
    let (stdin_cmd_tx, stdin_cmd_rx) = mpsc::channel::<StdinCommand>();
    let active_run: Arc<Mutex<Option<ActiveRunState>>> = Arc::new(Mutex::new(None));
    let alive = Arc::new(AtomicBool::new(true));

    let (event_tx, event_rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let run_id = request.id.clone();

    {
        let mut guard = active_run.lock().unwrap();
        *guard = Some(ActiveRunState {
            run_id: run_id.clone(),
            event_tx: event_tx.clone(),
            cancelled: Arc::clone(&cancelled),
        });
    }

    let (approval_tx, approval_rx) = mpsc::channel::<super::ApprovalResponse>();
    let bridge_stdin_tx = stdin_cmd_tx.clone();
    thread::spawn(move || {
        approval_bridge(approval_rx, bridge_stdin_tx);
    });

    let prompt = cosh_tui_prompt_from_request(request, mode);

    let stdin_writer_tx = stdin_cmd_tx.clone();
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

    let child_result = command.spawn();
    let mut child = match child_result {
        Ok(c) => c,
        Err(err) => {
            let _ = event_tx.send(Err(AdapterError {
                message: format!("failed to run cosh-tui: {err}"),
            }));
            return (
                PersistentProcess {
                    child_pid: 0,
                    stdin_tx: stdin_cmd_tx,
                    active_run,
                    alive: Arc::new(AtomicBool::new(false)),
                },
                AgentRunHandle {
                    receiver: event_rx,
                    cancel: Arc::new(|| {}),
                    approval_sender: Some(approval_tx),
                },
            );
        }
    };

    let child_pid = child.id();

    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    thread::spawn(move || {
        let _ = read_lossy(stderr);
    });

    thread::spawn(move || {
        persistent_stdin_writer(stdin, stdin_cmd_rx);
    });

    let init_msg = super::control_protocol::serialize_initialize("init-1");
    let _ = stdin_writer_tx.send(StdinCommand::SendLine(init_msg));

    let user_msg = super::control_protocol::serialize_user_message(&prompt, None);
    let _ = stdin_writer_tx.send(StdinCommand::SendLine(user_msg));

    let reader_active_run = Arc::clone(&active_run);
    let reader_alive = Arc::clone(&alive);
    let reader_session = session_state;
    thread::spawn(move || {
        persistent_stdout_reader(stdout, reader_active_run, reader_alive, reader_session);
    });

    let cancel_active = Arc::clone(&active_run);
    let cancel_flag = Arc::clone(&cancelled);
    let cancel_run_id = run_id;
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Ok(guard) = cancel_active.lock() {
            if let Some(run) = guard.as_ref() {
                if run.run_id == cancel_run_id {
                    let _ = run.event_tx.send(Ok(AgentEvent::AgentCancelled {
                        run_id: cancel_run_id.clone(),
                        reason: "user requested cancellation".to_string(),
                    }));
                }
            }
        }
    });

    let proc = PersistentProcess {
        child_pid,
        stdin_tx: stdin_cmd_tx,
        active_run,
        alive,
    };

    let handle = AgentRunHandle {
        receiver: event_rx,
        cancel,
        approval_sender: Some(approval_tx),
    };

    (proc, handle)
}

fn attach_run(
    proc: &PersistentProcess,
    request: &AgentRequest,
    mode: CoshApprovalMode,
    _session_state: Arc<Mutex<Option<String>>>,
) -> AgentRunHandle {
    let (event_tx, event_rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let run_id = request.id.clone();

    {
        let mut guard = proc.active_run.lock().unwrap();
        *guard = Some(ActiveRunState {
            run_id: run_id.clone(),
            event_tx: event_tx.clone(),
            cancelled: Arc::clone(&cancelled),
        });
    }

    let (approval_tx, approval_rx) = mpsc::channel::<super::ApprovalResponse>();
    let bridge_stdin_tx = proc.stdin_tx.clone();
    thread::spawn(move || {
        approval_bridge(approval_rx, bridge_stdin_tx);
    });

    let prompt = cosh_tui_prompt_from_request(request, mode);
    let user_msg = super::control_protocol::serialize_user_message(&prompt, None);
    let _ = proc.stdin_tx.send(StdinCommand::SendLine(user_msg));

    let cancel_active = Arc::clone(&proc.active_run);
    let cancel_flag = Arc::clone(&cancelled);
    let cancel_run_id = run_id;
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Ok(guard) = cancel_active.lock() {
            if let Some(run) = guard.as_ref() {
                if run.run_id == cancel_run_id {
                    let _ = run.event_tx.send(Ok(AgentEvent::AgentCancelled {
                        run_id: cancel_run_id.clone(),
                        reason: "user requested cancellation".to_string(),
                    }));
                }
            }
        }
    });

    AgentRunHandle {
        receiver: event_rx,
        cancel,
        approval_sender: Some(approval_tx),
    }
}

fn persistent_stdin_writer(
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<StdinCommand>,
) {
    use std::io::Write;
    let mut writer = std::io::BufWriter::new(stdin);

    while let Ok(cmd) = rx.recv() {
        match cmd {
            StdinCommand::SendLine(line) => {
                if writeln!(writer, "{line}").is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        }
    }
}

fn persistent_stdout_reader(
    stdout: std::process::ChildStdout,
    active_run: Arc<Mutex<Option<ActiveRunState>>>,
    alive: Arc<AtomicBool>,
    session_state: Arc<Mutex<Option<String>>>,
) {
    let mut parser: Option<ClaudeStreamParser> = None;
    let mut current_run_id: Option<String> = None;

    for line in std::io::BufReader::new(stdout).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let (event_tx, run_id, is_cancelled) = {
            let guard = active_run.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(run) => (
                    run.event_tx.clone(),
                    run.run_id.clone(),
                    run.cancelled.load(Ordering::SeqCst),
                ),
                None => {
                    continue;
                }
            }
        };

        if current_run_id.as_deref() != Some(&run_id) {
            current_run_id = Some(run_id.clone());
            parser = Some(ClaudeStreamParser::new(
                run_id.clone(),
                Some(Arc::clone(&session_state)),
            ));
        }

        if is_cancelled {
            if is_result_line(&line) {
                let mut guard = active_run.lock().unwrap_or_else(|e| e.into_inner());
                *guard = None;
                parser = None;
                current_run_id = None;
            }
            continue;
        }

        if let Some(ctrl) = super::control_protocol::parse_control_request(&line) {
            match ctrl {
                super::control_protocol::ControlRequest::CanUseTool {
                    request_id,
                    tool_name,
                    tool_input,
                    tool_use_id,
                } => {
                    let _ = event_tx.send(Ok(AgentEvent::ToolPermissionRequest {
                        run_id: run_id.clone(),
                        request_id,
                        tool_name,
                        tool_input,
                        tool_use_id,
                    }));
                }
                super::control_protocol::ControlRequest::Initialize { .. } => {}
            }
            continue;
        }

        if let Some(p) = parser.as_mut() {
            let events = p.parse_line(&line);
            let mut completed = false;
            for event in events {
                if matches!(
                    event,
                    AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
                ) {
                    completed = true;
                }
                let _ = event_tx.send(Ok(event));
            }
            if completed {
                let mut guard = active_run.lock().unwrap_or_else(|e| e.into_inner());
                if guard.as_ref().is_some_and(|r| r.run_id == run_id) {
                    *guard = None;
                }
                parser = None;
                current_run_id = None;
            }
        }
    }

    alive.store(false, Ordering::SeqCst);
    let mut guard = active_run.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = guard.take() {
        let _ = run.event_tx.send(Ok(AgentEvent::AgentFailed {
            run_id: run.run_id,
            error: "cosh-tui process exited unexpectedly".to_string(),
        }));
    }
}

fn approval_bridge(
    rx: mpsc::Receiver<super::ApprovalResponse>,
    stdin_tx: mpsc::Sender<StdinCommand>,
) {
    while let Ok(response) = rx.recv() {
        let msg = match &response.decision {
            super::ApprovalDecision::Allow => match response.tool_use_id.as_deref() {
                Some(tool_use_id) => {
                    super::control_protocol::serialize_allow(&response.request_id, tool_use_id)
                }
                None => super::control_protocol::serialize_deny(
                    &response.request_id,
                    "Missing provider tool_use_id",
                ),
            },
            super::ApprovalDecision::Deny { message } => {
                super::control_protocol::serialize_deny(&response.request_id, message)
            }
        };
        if stdin_tx.send(StdinCommand::SendLine(msg)).is_err() {
            break;
        }
    }
}

fn is_result_line(line: &str) -> bool {
    let trimmed = line.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        v.get("type").and_then(|t| t.as_str()) == Some("result")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::CoshTuiAdapter;
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
            session_id: Arc::new(Mutex::new(None)),
            persistent: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn prepare_invocation_headless_flag() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert_eq!(inv.program, "cosh-tui");
        assert!(inv.args.contains(&"--headless".to_string()));
    }

    #[test]
    fn prepare_invocation_approval_modes() {
        let suggest = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Suggest);
        assert!(suggest.args.contains(&"strict".to_string()));

        let ask = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert!(ask.args.contains(&"balanced".to_string()));

        let auto = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(auto.args.contains(&"auto".to_string()));

        let trust = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
        assert!(trust.args.contains(&"trust".to_string()));
    }

    #[test]
    fn prepare_invocation_session_resume() {
        let adapter = CoshTuiAdapter {
            program: "cosh-tui".to_string(),
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
            persistent: Arc::new(Mutex::new(None)),
        };
        let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert!(inv.args.contains(&"--resume".to_string()));
        assert!(inv.args.contains(&"prev-sess".to_string()));
    }

    #[test]
    fn prepare_invocation_no_resume_when_empty() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Ask);
        assert!(!inv.args.contains(&"--resume".to_string()));
    }

    #[test]
    fn capabilities_match_expected() {
        use super::super::AgentAdapter;
        let adapter = test_adapter();
        let caps = adapter.capabilities();
        assert!(caps.text_stream);
        assert!(caps.session_resume);
        assert!(caps.tool_intent);
        assert!(caps.user_question);
        assert!(caps.cancellable);
        assert!(caps.control_protocol);
    }

    #[test]
    fn adapter_name() {
        use super::super::AgentAdapter;
        let adapter = test_adapter();
        assert_eq!(adapter.name(), "cosh-tui");
    }

    #[test]
    fn persistent_default_state_is_none() {
        let adapter = CoshTuiAdapter::default();
        let guard = adapter.persistent.lock().unwrap();
        assert!(guard.is_none());
    }
}
