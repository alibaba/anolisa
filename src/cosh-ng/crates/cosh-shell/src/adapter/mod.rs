use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::types::{AgentEvent, AgentRequest};

mod claude;
mod claude_stream;
mod claude_stream_extract;
#[cfg(test)]
mod claude_stream_tests;
mod control_protocol;
#[cfg(test)]
mod control_protocol_tests;
mod cosh_core;
mod cosh_core_process;
mod cosh_core_registry;
#[cfg(test)]
mod cosh_core_registry_tests;
#[cfg(test)]
mod cosh_core_tests;
mod fake;
mod process;
mod prompt;
#[cfg(test)]
mod prompt_tests;
mod qwen;
mod qwen_stream;

pub use claude::ClaudeCodeAdapter;
use claude_stream::ClaudeStreamParser;
pub use control_protocol::*;
pub use cosh_core::{
    CoshCoreAdapter, SessionClearFailure, SessionClearInterruption, SessionClearPlan,
    SessionClearResult, SessionErrorInfo, SessionHealth, SessionList, SessionManagementClient,
    SessionRecovery, SessionRecoveryState, SessionRuntimeState, SessionSummary,
};
pub use fake::FakeAgentAdapter;
pub(crate) use process::{
    agent_event_is_provider_progress, record_cancellation_pending_session,
    run_provider_process_loop, spawn_provider_child, terminate_and_reap_process,
    terminate_process_group, ProviderLineProgress, ProviderPromptArgMode, ProviderRunOutcome,
    ProviderStdinMode,
};
pub use prompt::{
    prompt_from_request, prompt_from_request_with_evidence_access,
    prompt_from_request_with_evidence_policy, provider_prompt_contract,
};
pub use qwen::QwenCliAdapter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentBackendCapabilities {
    pub text_stream: bool,
    pub thinking_stream: bool,
    pub session_resume: bool,
    pub tool_intent: bool,
    pub user_question: bool,
    pub cancellable: bool,
    pub control_protocol: bool,
}

pub trait AgentAdapter {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> AgentBackendCapabilities {
        AgentBackendCapabilities::default()
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError>;

    fn run_stream(
        &self,
        request: &AgentRequest,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        for event in self.run(request)? {
            sink(event)?;
        }
        Ok(())
    }
}

pub struct AgentRunHandle {
    receiver: mpsc::Receiver<Result<AgentEvent, AdapterError>>,
    cancel: Arc<dyn Fn() + Send + Sync>,
    pub(crate) approval_sender: Option<mpsc::Sender<ApprovalResponse>>,
    pub(crate) auth_sender: Option<std::sync::mpsc::Sender<AuthResponse>>,
    control_capabilities: Arc<Mutex<ControlProtocolCapabilities>>,
    pending_provider_session: Option<Arc<Mutex<Option<String>>>>,
    cancellation_artifacts: ProviderCancellationArtifactStore,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderCancellationArtifactStore {
    inner: Arc<Mutex<Vec<ProviderCancellationArtifact>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCancellationArtifact {
    pub provider: &'static str,
    pub run_id: String,
    pub kind: ProviderCancellationArtifactKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderCancellationArtifactKind {
    StdoutLine,
    StderrTail,
    PendingSession,
}

impl ProviderCancellationArtifactStore {
    pub fn push(&self, artifact: ProviderCancellationArtifact) {
        if artifact.text.trim().is_empty() {
            return;
        }
        let Ok(mut artifacts) = self.inner.lock() else {
            return;
        };
        artifacts.push(artifact);
        const MAX_ARTIFACTS: usize = 32;
        if artifacts.len() > MAX_ARTIFACTS {
            let excess = artifacts.len() - MAX_ARTIFACTS;
            artifacts.drain(0..excess);
        }
    }

    pub fn snapshot(&self) -> Vec<ProviderCancellationArtifact> {
        self.inner
            .lock()
            .map(|artifacts| artifacts.clone())
            .unwrap_or_default()
    }
}

pub enum AgentRunPoll {
    Event(AgentEvent),
    Timeout,
    Finished,
}

impl AgentRunHandle {
    #[cfg(test)]
    pub(crate) fn test_with_approval_sender(
        approval_sender: mpsc::Sender<ApprovalResponse>,
    ) -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            receiver,
            cancel: Arc::new(|| {}),
            approval_sender: Some(approval_sender),
            auth_sender: None,
            control_capabilities: Arc::new(Mutex::new(ControlProtocolCapabilities::default())),
            pending_provider_session: None,
            cancellation_artifacts: ProviderCancellationArtifactStore::default(),
        }
    }

    pub fn cancel(&self) {
        (self.cancel)();
    }

    pub fn poll_event_timeout(&self, timeout: Duration) -> Result<AgentRunPoll, AdapterError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(Ok(event)) => Ok(AgentRunPoll::Event(event)),
            Ok(Err(err)) => Err(err),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(AgentRunPoll::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(AgentRunPoll::Finished),
        }
    }

    pub fn respond_approval(&self, response: ApprovalResponse) -> Result<(), AdapterError> {
        self.approval_sender
            .as_ref()
            .ok_or_else(|| AdapterError {
                message: "no approval channel (not in control protocol mode)".to_string(),
            })?
            .send(response)
            .map_err(|_| AdapterError {
                message: "approval channel closed".to_string(),
            })
    }

    pub fn respond_auth(&self, response: AuthResponse) -> Result<(), String> {
        self.auth_sender
            .as_ref()
            .ok_or_else(|| "no auth channel available".to_string())?
            .send(response)
            .map_err(|_| "auth channel closed".to_string())
    }

    pub fn control_capabilities(&self) -> ControlProtocolCapabilities {
        self.control_capabilities
            .lock()
            .map(|capabilities| *capabilities)
            .unwrap_or_default()
    }

    pub fn pending_provider_session_id(&self) -> Option<String> {
        self.pending_provider_session
            .as_ref()
            .and_then(|pending| pending.lock().ok().and_then(|guard| guard.clone()))
    }

    pub fn cancellation_artifact_store(&self) -> ProviderCancellationArtifactStore {
        self.cancellation_artifacts.clone()
    }

    pub fn next_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<AgentEvent>, AdapterError> {
        match self.poll_event_timeout(timeout)? {
            AgentRunPoll::Event(event) => Ok(Some(event)),
            AgentRunPoll::Timeout | AgentRunPoll::Finished => Ok(None),
        }
    }

    pub fn drain_cancelled_in_background(self, timeout: Duration) {
        thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let poll_timeout = (deadline - now).min(Duration::from_millis(100));
                match self.poll_event_timeout(poll_timeout) {
                    Ok(AgentRunPoll::Event(_)) | Ok(AgentRunPoll::Timeout) => {}
                    Ok(AgentRunPoll::Finished) | Err(_) => break,
                }
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Fake,
    ClaudeCode,
    QwenCli,
    CoshCore,
}

impl AdapterKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fake" => Some(Self::Fake),
            "claude" | "claude-code" => Some(Self::ClaudeCode),
            "co" | "qwen" | "qwen-cli" => Some(Self::QwenCli),
            "cosh-core" | "core" => Some(Self::CoshCore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AdapterInstance {
    Fake(FakeAgentAdapter),
    ClaudeCode(ClaudeCodeAdapter),
    QwenCli(QwenCliAdapter),
    CoshCore(CoshCoreAdapter),
}

impl AgentAdapter for AdapterInstance {
    fn name(&self) -> &'static str {
        match self {
            Self::Fake(adapter) => adapter.name(),
            Self::ClaudeCode(adapter) => adapter.name(),
            Self::QwenCli(adapter) => adapter.name(),
            Self::CoshCore(adapter) => adapter.name(),
        }
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        match self {
            Self::Fake(adapter) => adapter.capabilities(),
            Self::ClaudeCode(adapter) => adapter.capabilities(),
            Self::QwenCli(adapter) => adapter.capabilities(),
            Self::CoshCore(adapter) => adapter.capabilities(),
        }
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError> {
        match self {
            Self::Fake(adapter) => adapter.run(request),
            Self::ClaudeCode(adapter) => adapter.run(request),
            Self::QwenCli(adapter) => adapter.run(request),
            Self::CoshCore(adapter) => adapter.run(request),
        }
    }

    fn run_stream(
        &self,
        request: &AgentRequest,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        match self {
            Self::Fake(adapter) => adapter.run_stream(request, sink),
            Self::ClaudeCode(adapter) => adapter.run_stream(request, sink),
            Self::QwenCli(adapter) => adapter.run_stream(request, sink),
            Self::CoshCore(adapter) => adapter.run_stream(request, sink),
        }
    }
}

impl AdapterInstance {
    pub fn start_cancellable(
        &self,
        request: AgentRequest,
        mode: crate::types::CoshApprovalMode,
    ) -> AgentRunHandle {
        match self {
            Self::ClaudeCode(adapter) => adapter.start_cancellable(request, mode),
            Self::QwenCli(adapter) => adapter.start_cancellable(request, mode),
            Self::CoshCore(adapter) => adapter.start_cancellable(request, mode),
            _ => start_threaded_adapter_run(self.clone(), request),
        }
    }

    pub fn committed_session_id(&self) -> Option<String> {
        match self {
            Self::ClaudeCode(adapter) => adapter.session_id.lock().ok().and_then(|id| id.clone()),
            Self::QwenCli(adapter) => adapter.session_id.lock().ok().and_then(|id| id.clone()),
            Self::CoshCore(adapter) => adapter.committed_session_id(),
            Self::Fake(_) => None,
        }
    }

    pub fn provider_invocation(&self) -> Option<String> {
        match self {
            Self::ClaudeCode(adapter) => Some(adapter.program.clone()),
            Self::QwenCli(adapter) => Some(adapter.program.clone()),
            Self::CoshCore(adapter) => Some(adapter.program.clone()),
            Self::Fake(_) => None,
        }
    }
}

pub(super) fn commit_pending_session(
    committed: &Arc<Mutex<Option<String>>>,
    pending: &Arc<Mutex<Option<String>>>,
) {
    let Some(pending_id) = pending.lock().ok().and_then(|id| id.clone()) else {
        return;
    };
    if let Ok(mut committed_id) = committed.lock() {
        *committed_id = Some(pending_id);
    }
}

pub(super) fn commit_provider_session_if_completed(
    outcome: &ProviderRunOutcome,
    completed: bool,
    failed: bool,
    committed: &Arc<Mutex<Option<String>>>,
    pending: &Arc<Mutex<Option<String>>>,
) {
    if matches!(outcome, ProviderRunOutcome::Exited { status, .. } if status.success())
        && completed
        && !failed
    {
        commit_pending_session(committed, pending);
    }
}

pub fn adapter_for_kind(kind: AdapterKind) -> AdapterInstance {
    match kind {
        AdapterKind::Fake => AdapterInstance::Fake(FakeAgentAdapter),
        AdapterKind::ClaudeCode => AdapterInstance::ClaudeCode(ClaudeCodeAdapter::default()),
        AdapterKind::QwenCli => AdapterInstance::QwenCli(QwenCliAdapter::default()),
        AdapterKind::CoshCore => AdapterInstance::CoshCore(CoshCoreAdapter::default()),
    }
}

fn start_threaded_adapter_run(adapter: AdapterInstance, request: AgentRequest) -> AgentRunHandle {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&cancelled);
    let cancel = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
    });

    thread::spawn(move || {
        let mut sent_cancelled = false;
        let result = adapter.run_stream(&request, &mut |event| {
            if cancelled.load(Ordering::SeqCst) {
                sent_cancelled = true;
                let _ = sender.send(Ok(AgentEvent::AgentCancelled {
                    run_id: request.id.clone(),
                    reason: "user requested cancellation".to_string(),
                }));
                return Err(AdapterError {
                    message: "agent run cancelled".to_string(),
                });
            }
            sender.send(Ok(event)).map_err(|_| AdapterError {
                message: "agent event receiver dropped".to_string(),
            })
        });
        if let Err(err) = result {
            if !sent_cancelled {
                let _ = sender.send(Err(err));
            }
        }
    });

    AgentRunHandle {
        receiver,
        cancel,
        approval_sender: None,
        auth_sender: None,
        control_capabilities: Arc::new(Mutex::new(ControlProtocolCapabilities::default())),
        pending_provider_session: None,
        cancellation_artifacts: ProviderCancellationArtifactStore::default(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub prompt: String,
}

impl PreparedInvocation {
    pub fn argv_preview(&self) -> Vec<String> {
        let mut argv = vec![self.program.clone()];
        argv.extend(self.args.clone());
        argv.push("<prompt>".to_string());
        argv
    }
}
