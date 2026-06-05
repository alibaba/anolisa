use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use nix::libc;

use crate::types::{AgentEvent, AgentRequest};

use super::{
    prompt_from_request, start_threaded_adapter_run, AdapterError, AdapterInstance, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, ClaudeStreamParser, PreparedInvocation,
};

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter {
    pub program: String,
    pub model: String,
    pub max_budget_usd: String,
    pub allow_model_call: bool,
    session_id: Arc<Mutex<Option<String>>>,
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self {
            program: "claude".to_string(),
            model: "sonnet".to_string(),
            max_budget_usd: std::env::var("COSH_CLAUDE_MAX_BUDGET_USD")
                .unwrap_or_else(|_| "1.00".to_string()),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(None)),
        }
    }
}

impl ClaudeCodeAdapter {
    pub fn with_model_call(mut self, allow_model_call: bool) -> Self {
        self.allow_model_call = allow_model_call;
        self
    }

    pub fn start_cancellable(&self, request: AgentRequest) -> AgentRunHandle {
        let prepared = self.prepare_invocation(&request);
        if !self.allow_model_call {
            let adapter = AdapterInstance::ClaudeCode(self.clone());
            return start_threaded_adapter_run(adapter, request);
        }

        start_cancellable_claude_process(request.id, prepared, Arc::clone(&self.session_id))
    }

    pub fn prepare_invocation(&self, request: &AgentRequest) -> PreparedInvocation {
        let resume_session = self.session_id.lock().ok().and_then(|guard| guard.clone());
        let mut args = vec![
            "--print".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--include-partial-messages".to_string(),
            "--model".to_string(),
            self.model.clone(),
            "--permission-mode".to_string(),
            "plan".to_string(),
            "--tools".to_string(),
            "default".to_string(),
            "--max-budget-usd".to_string(),
            self.max_budget_usd.clone(),
        ];
        if let Some(session_id) = resume_session {
            args.push("--resume".to_string());
            args.push(session_id);
        }

        PreparedInvocation {
            program: self.program.clone(),
            args,
            prompt: prompt_from_request(request),
        }
    }
}

impl AgentAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        AgentBackendCapabilities {
            text_stream: true,
            thinking_stream: true,
            session_resume: true,
            tool_intent: true,
            user_question: true,
            cancellable: true,
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
        let prepared = self.prepare_invocation(request);
        if !self.allow_model_call {
            for event in claude_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }

        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting claude-code stream-json backend".to_string(),
        })?;

        let mut child = Command::new(&prepared.program)
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AdapterError {
                message: format!("failed to run claude code: {err}"),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| AdapterError {
            message: "failed to capture claude code stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| AdapterError {
            message: "failed to capture claude code stderr".to_string(),
        })?;
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let mut parser =
            ClaudeStreamParser::new(request.id.clone(), Some(Arc::clone(&self.session_id)));
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|err| AdapterError {
                message: format!("failed to read claude code stream: {err}"),
            })?;
            for event in parser.parse_line(&line) {
                sink(event)?;
            }
        }

        let status = child.wait().map_err(|err| AdapterError {
            message: format!("failed to wait for claude code: {err}"),
        })?;
        if !status.success() {
            let error = join_reader_thread(stderr_handle, "claude code stderr")?;
            sink(AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: error.trim().to_string(),
            })?;
            return Ok(());
        }

        let _stderr = join_reader_thread(stderr_handle, "claude code stderr")?;
        parser.finish(sink)
    }
}

fn start_cancellable_claude_process(
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
                message: "starting claude-code stream-json backend".to_string(),
            },
        );

        let mut command = Command::new(&prepared.program);
        command
            .args(&prepared.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !prepared.prompt.is_empty() {
            command.arg(&prepared.prompt);
        }
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
                    message: format!("failed to run claude code: {err}"),
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
                    message: "failed to capture claude code stdout".to_string(),
                }));
                return;
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = sender.send(Err(AdapterError {
                    message: "failed to capture claude code stderr".to_string(),
                }));
                return;
            }
        };
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let mut parser = ClaudeStreamParser::new(run_id.clone(), Some(session_state));
        for line in BufReader::new(stdout).lines() {
            if cancelled.load(Ordering::SeqCst) {
                terminate_process(child.id());
                break;
            }
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    let _ = sender.send(Err(AdapterError {
                        message: format!("failed to read claude code stream: {err}"),
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
                    message: format!("failed to wait for claude code: {err}"),
                }));
                return;
            }
        };
        if let Ok(mut pid) = child_pid.lock() {
            *pid = None;
        }

        let was_cancelled = cancelled.load(Ordering::SeqCst);
        if was_cancelled {
            let _stderr = join_reader_thread(stderr_handle, "claude code stderr");
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
            let error = join_reader_thread(stderr_handle, "claude code stderr")
                .map(|stderr| stderr.trim().to_string())
                .unwrap_or_else(|err| err.message);
            send_agent_event(&sender, AgentEvent::AgentFailed { run_id, error });
            return;
        }

        let _stderr = join_reader_thread(stderr_handle, "claude code stderr");
        let _ = parser.finish(&mut |event| {
            send_agent_event(&sender, event);
            Ok(())
        });
    });

    AgentRunHandle { receiver, cancel }
}

fn send_agent_event(sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>, event: AgentEvent) {
    let _ = sender.send(Ok(event));
}

fn terminate_process(pid: u32) {
    let pid = pid as i32;
    unsafe {
        if libc::kill(-pid, libc::SIGKILL) < 0 {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

fn read_lossy(mut reader: impl Read) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn join_reader_thread(
    handle: thread::JoinHandle<std::io::Result<String>>,
    stream_name: &str,
) -> Result<String, AdapterError> {
    handle
        .join()
        .map_err(|_| AdapterError {
            message: format!("failed to join {stream_name} reader"),
        })?
        .map_err(|err| AdapterError {
            message: format!("failed to read {stream_name}: {err}"),
        })
}

fn claude_dry_run_events(request: &AgentRequest, prepared: &PreparedInvocation) -> Vec<AgentEvent> {
    let run_id = format!("claude-dry-run-{}", request.command_block.id);
    vec![
        AgentEvent::StatusChanged {
            run_id: run_id.clone(),
            phase: "dry_run".to_string(),
            message: "prepared claude-code invocation without model call".to_string(),
        },
        AgentEvent::TextDelta {
            run_id: run_id.clone(),
            text: format!(
                "Claude Code adapter prepared a safe recommend-only invocation; model execution requires --run.\n\nPrepared invocation:\n  {}",
                prepared.argv_preview().join(" ")
            ),
        },
        AgentEvent::AgentCompleted {
            run_id,
            summary: "claude code dry-run completed without model call".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{start_cancellable_claude_process, PreparedInvocation};
    use crate::types::AgentEvent;

    #[test]
    fn cancellable_claude_process_emits_cancelled_event() {
        let handle = start_cancellable_claude_process(
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
