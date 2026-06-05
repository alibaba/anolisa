use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::types::{AgentEvent, AgentRequest, CoshApprovalMode};

mod driver;

use self::driver::{start_cancellable_qwen_process, start_control_protocol_qwen_process};
use super::claude::{join_reader_thread, read_lossy, send_agent_event, terminate_process};
use super::qwen_stream::QwenStreamParser;
use super::{
    commit_pending_session, prompt_from_request, provider_prompt_contract,
    start_threaded_adapter_run, AdapterError, AdapterInstance, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, PreparedInvocation, ProviderLineProgress,
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
        let disable_resume = request
            .context_hints
            .iter()
            .any(|hint| hint.contains("disable provider resume"));
        let resume_session = if disable_resume {
            None
        } else {
            self.session_id.lock().ok().and_then(|guard| guard.clone())
        };
        let mut args = vec![
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--include-partial-messages".to_string(),
        ];

        args.extend(["--input-format".to_string(), "stream-json".to_string()]);
        match mode {
            CoshApprovalMode::Recommend => {
                args.extend(["--approval-mode".to_string(), "plan".to_string()]);
            }
            CoshApprovalMode::Auto => {
                args.extend(["--approval-mode".to_string(), "default".to_string()]);
                args.extend([
                    "--allowed-tools".to_string(),
                    "Read,Grep,Glob,LS,read_file,grep_search,glob,list_directory,read_many_files"
                        .to_string(),
                ]);
            }
            CoshApprovalMode::Trust => {
                args.extend(["--approval-mode".to_string(), "default".to_string()]);
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

        if mode.uses_control_protocol() {
            return start_control_protocol_qwen_process(
                request.id,
                prepared,
                Arc::clone(&self.session_id),
            );
        }

        start_cancellable_qwen_process(request.id, prepared, Arc::clone(&self.session_id))
    }
}

impl AgentAdapter for QwenCliAdapter {
    fn name(&self) -> &'static str {
        "co"
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
            for event in qwen_dry_run_events(request, &prepared) {
                sink(event)?;
            }
            return Ok(());
        }

        sink(AgentEvent::StatusChanged {
            run_id: request.id.clone(),
            phase: "starting".to_string(),
            message: "starting co stream-json backend".to_string(),
        })?;

        let mut child = Command::new(&prepared.program)
            .args(qwen_args_with_prompt(&prepared))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| AdapterError {
                message: format!("failed to run co cli: {err}"),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| AdapterError {
            message: "failed to capture co cli stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| AdapterError {
            message: "failed to capture co cli stderr".to_string(),
        })?;
        let stderr_handle = thread::spawn(move || read_lossy(stderr));

        let pending_session = Arc::new(Mutex::new(None));
        let mut parser =
            QwenStreamParser::new(request.id.clone(), Some(Arc::clone(&pending_session)));
        let mut completed = false;
        let mut failed = false;
        let mut terminal_events = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|err| AdapterError {
                message: format!("failed to read co cli stream: {err}"),
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
            message: format!("failed to wait for co cli: {err}"),
        })?;
        if !status.success() {
            let error = join_reader_thread(stderr_handle, "co cli stderr")?;
            sink(AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: error.trim().to_string(),
            })?;
            return Ok(());
        }

        let _stderr = join_reader_thread(stderr_handle, "co cli stderr")?;
        parser.finish(&mut |event| {
            update_completion_flags(&event, &mut completed, &mut failed);
            if is_terminal_agent_event(&event) {
                terminal_events.push(event);
                Ok(())
            } else {
                sink(event)
            }
        })?;
        if completed && !failed {
            commit_pending_session(&self.session_id, &pending_session);
        }
        for event in terminal_events {
            sink(event)?;
        }
        Ok(())
    }
}

fn qwen_prompt_from_request(request: &AgentRequest, mode: CoshApprovalMode) -> String {
    format!(
        "{}{}",
        prompt_from_request(request),
        provider_prompt_contract(mode, "run_shell_command")
    )
}

fn line_progress(progressed: bool) -> ProviderLineProgress {
    if progressed {
        ProviderLineProgress::Progress
    } else {
        ProviderLineProgress::NoProgress
    }
}

fn qwen_args_with_prompt(prepared: &PreparedInvocation) -> Vec<String> {
    let mut args = prepared.args.clone();
    args.push("--prompt".to_string());
    args.push(prepared.prompt.clone());
    args
}

fn update_completion_flags(event: &AgentEvent, completed: &mut bool, failed: &mut bool) {
    match event {
        AgentEvent::AgentCompleted { .. } => *completed = true,
        AgentEvent::AgentFailed { .. } | AgentEvent::AgentCancelled { .. } => *failed = true,
        _ => {}
    }
}

fn is_terminal_agent_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::AgentCompleted { .. }
            | AgentEvent::AgentFailed { .. }
            | AgentEvent::AgentCancelled { .. }
    )
}

fn qwen_dry_run_events(request: &AgentRequest, prepared: &PreparedInvocation) -> Vec<AgentEvent> {
    let run_id = format!("qwen-dry-run-{}", request.command_block.id);
    vec![
        AgentEvent::StatusChanged {
            run_id: run_id.clone(),
            phase: "dry_run".to_string(),
            message: "prepared co invocation without model call".to_string(),
        },
        AgentEvent::TextDelta {
            run_id: run_id.clone(),
            text: format!(
                "co adapter prepared a safe recommend-only invocation; model execution is disabled by default.\n\nPrepared invocation:\n  {}",
                prepared.argv_preview().join(" ")
            ),
        },
        AgentEvent::AgentCompleted {
            run_id,
            summary: "co dry-run completed without model call".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{driver::start_cancellable_qwen_process, PreparedInvocation, QwenCliAdapter};
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

    fn shell_evidence_request() -> AgentRequest {
        let mut request = test_request();
        request.id = "evidence-follow-up".to_string();
        request.user_input = Some(
            "ShellEvidenceExcerpt\n\
             output_id: terminal-output://sess/blk\n\
             command_id: blk\n\
             command: echo test\n\
             excerpt_status: included\n\
             redaction_status: excerpt_included\n\
             bounded_output_excerpt:\n\
             test\n"
                .to_string(),
        );
        request
    }

    fn mock_provider_script(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("cosh-qwen-{name}-{}", std::process::id()));
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write mock provider");
        let mut permissions = fs::metadata(&path)
            .expect("mock provider metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod mock provider");
        path
    }

    #[test]
    fn mode_flags_co_recommend() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Recommend);
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
    fn mode_flags_co_auto() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(inv.args.contains(&"--approval-mode".to_string()));
        assert!(inv.args.contains(&"default".to_string()));
        assert!(inv.args.contains(&"--allowed-tools".to_string()));
        assert!(inv.args.iter().any(|arg| arg.contains("read_file")));
        assert!(inv.prompt.contains("run_shell_command"), "{}", inv.prompt);
        assert!(
            inv.prompt
                .contains("Always emit a provider permission request"),
            "{}",
            inv.prompt
        );
        assert!(
            !inv.prompt.contains("Qwen adapter compatibility"),
            "{}",
            inv.prompt
        );
        assert!(!inv.prompt.contains("COSH_QUESTION"), "{}", inv.prompt);
        assert!(!inv.args.contains(&"--allowedTools".to_string()));
    }

    #[test]
    fn mode_flags_co_trust() {
        let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
        assert!(inv.args.contains(&"--approval-mode".to_string()));
        assert!(inv.args.contains(&"default".to_string()));
        assert!(!inv.args.contains(&"yolo".to_string()));
        assert!(inv.args.contains(&"--input-format".to_string()));
    }

    #[test]
    fn mode_flags_co_session_resume() {
        let adapter = QwenCliAdapter {
            program: "qwen".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        };
        let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
        assert!(inv.args.contains(&"--resume".to_string()));
        assert!(inv.args.contains(&"prev-sess".to_string()));
    }

    #[test]
    fn co_evidence_follow_up_reuses_committed_session_as_plain_prompt() {
        let adapter = QwenCliAdapter {
            program: "qwen".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        };
        let inv = adapter.prepare_invocation(&shell_evidence_request(), CoshApprovalMode::Auto);
        assert!(inv.args.contains(&"--resume".to_string()));
        assert!(inv.args.contains(&"prev-sess".to_string()));
        assert!(
            inv.prompt.contains("user-requested shell evidence excerpt"),
            "{}",
            inv.prompt
        );
        assert!(
            !inv.prompt.contains("Tool result for request"),
            "{}",
            inv.prompt
        );
        assert!(
            !inv.prompt.contains("host_executed_shell"),
            "{}",
            inv.prompt
        );
    }

    #[test]
    fn mode_flags_co_fallback_can_disable_session_resume() {
        let adapter = QwenCliAdapter {
            program: "qwen".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        };
        let mut request = test_request();
        request
            .context_hints
            .push("disable provider resume for shell handoff fallback".to_string());
        let inv = adapter.prepare_invocation(&request, CoshApprovalMode::Auto);
        assert!(!inv.args.contains(&"--resume".to_string()));
        assert!(!inv.args.contains(&"prev-sess".to_string()));
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

    #[test]
    fn commits_session_only_after_successful_completion() {
        let script = mock_provider_script(
            "success",
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-ok\",\"model\":\"qwen\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"session_id\":\"sess-ok\",\"result\":\"done\"}'",
        );
        let committed = Arc::new(Mutex::new(None));
        let handle: AgentRunHandle = start_cancellable_qwen_process(
            "run-success".to_string(),
            PreparedInvocation {
                program: script.display().to_string(),
                args: Vec::new(),
                prompt: String::new(),
            },
            Arc::clone(&committed),
        );

        let mut saw_completed = false;
        for _ in 0..10 {
            if matches!(
                handle
                    .next_event_timeout(Duration::from_secs(1))
                    .expect("event"),
                Some(AgentEvent::AgentCompleted { .. })
            ) {
                saw_completed = true;
                break;
            }
        }
        let _ = fs::remove_file(script);
        assert!(saw_completed);
        assert_eq!(
            committed.lock().expect("committed session").as_deref(),
            Some("sess-ok")
        );
    }

    #[test]
    fn passes_prompt_with_flag_when_allowed_tools_are_present() {
        let script = mock_provider_script(
            "prompt-flag",
            "saw_prompt=0\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--prompt\" ]; then saw_prompt=1; fi\ndone\nif [ \"$saw_prompt\" -ne 1 ]; then exit 7; fi\nprintf '%s\\n' '{\"type\":\"result\",\"session_id\":\"sess-prompt\",\"result\":\"done\"}'",
        );
        let handle: AgentRunHandle = start_cancellable_qwen_process(
            "run-prompt".to_string(),
            PreparedInvocation {
                program: script.display().to_string(),
                args: vec![
                    "--allowed-tools".to_string(),
                    "Read,Grep,Glob,LS".to_string(),
                ],
                prompt: "hello prompt".to_string(),
            },
            Arc::new(Mutex::new(None)),
        );

        let mut saw_completed = false;
        for _ in 0..10 {
            if matches!(
                handle
                    .next_event_timeout(Duration::from_secs(1))
                    .expect("event"),
                Some(AgentEvent::AgentCompleted { .. })
            ) {
                saw_completed = true;
                break;
            }
        }
        let _ = fs::remove_file(script);
        assert!(saw_completed);
    }

    #[test]
    fn does_not_commit_session_after_provider_failure() {
        let script = mock_provider_script(
            "failure",
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-bad\",\"model\":\"qwen\"}'\nexit 2",
        );
        let committed = Arc::new(Mutex::new(Some("sess-prev".to_string())));
        let handle: AgentRunHandle = start_cancellable_qwen_process(
            "run-failure".to_string(),
            PreparedInvocation {
                program: script.display().to_string(),
                args: Vec::new(),
                prompt: String::new(),
            },
            Arc::clone(&committed),
        );

        let mut saw_failed = false;
        for _ in 0..10 {
            if matches!(
                handle
                    .next_event_timeout(Duration::from_secs(1))
                    .expect("event"),
                Some(AgentEvent::AgentFailed { .. })
            ) {
                saw_failed = true;
                break;
            }
        }
        let _ = fs::remove_file(script);
        assert!(saw_failed);
        assert_eq!(
            committed.lock().expect("committed session").as_deref(),
            Some("sess-prev")
        );
    }
}
