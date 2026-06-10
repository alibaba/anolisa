use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::types::{AgentEvent, AgentRequest};

mod claude;
mod claude_stream;
#[cfg(test)]
mod claude_stream_tests;
mod control_protocol;
mod cosh_tui;
mod fake;
mod prompt;
mod qwen;
mod qwen_stream;

pub use claude::ClaudeCodeAdapter;
use claude_stream::ClaudeStreamParser;
pub use control_protocol::*;
pub use cosh_tui::CoshTuiAdapter;
pub use fake::FakeAgentAdapter;
pub use prompt::{prompt_from_request, provider_prompt_contract};
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
}

pub enum AgentRunPoll {
    Event(AgentEvent),
    Timeout,
    Finished,
}

impl AgentRunHandle {
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

    pub fn next_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<AgentEvent>, AdapterError> {
        match self.poll_event_timeout(timeout)? {
            AgentRunPoll::Event(event) => Ok(Some(event)),
            AgentRunPoll::Timeout | AgentRunPoll::Finished => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Fake,
    ClaudeCode,
    QwenCli,
    CoshTui,
}

impl AdapterKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fake" => Some(Self::Fake),
            "claude" | "claude-code" => Some(Self::ClaudeCode),
            "qwen" | "qwen-cli" => Some(Self::QwenCli),
            "cosh-tui" | "tui" => Some(Self::CoshTui),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AdapterInstance {
    Fake(FakeAgentAdapter),
    ClaudeCode(ClaudeCodeAdapter),
    QwenCli(QwenCliAdapter),
    CoshTui(CoshTuiAdapter),
}

impl AgentAdapter for AdapterInstance {
    fn name(&self) -> &'static str {
        match self {
            Self::Fake(adapter) => adapter.name(),
            Self::ClaudeCode(adapter) => adapter.name(),
            Self::QwenCli(adapter) => adapter.name(),
            Self::CoshTui(adapter) => adapter.name(),
        }
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        match self {
            Self::Fake(adapter) => adapter.capabilities(),
            Self::ClaudeCode(adapter) => adapter.capabilities(),
            Self::QwenCli(adapter) => adapter.capabilities(),
            Self::CoshTui(adapter) => adapter.capabilities(),
        }
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError> {
        match self {
            Self::Fake(adapter) => adapter.run(request),
            Self::ClaudeCode(adapter) => adapter.run(request),
            Self::QwenCli(adapter) => adapter.run(request),
            Self::CoshTui(adapter) => adapter.run(request),
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
            Self::CoshTui(adapter) => adapter.run_stream(request, sink),
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
            Self::CoshTui(adapter) => adapter.start_cancellable(request, mode),
            _ => start_threaded_adapter_run(self.clone(), request),
        }
    }
}

pub fn adapter_for_kind(kind: AdapterKind) -> AdapterInstance {
    match kind {
        AdapterKind::Fake => AdapterInstance::Fake(FakeAgentAdapter),
        AdapterKind::ClaudeCode => AdapterInstance::ClaudeCode(ClaudeCodeAdapter::default()),
        AdapterKind::QwenCli => AdapterInstance::QwenCli(QwenCliAdapter::default()),
        AdapterKind::CoshTui => AdapterInstance::CoshTui(CoshTuiAdapter::default()),
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

pub(super) fn first_token(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .filter(|token| !token.is_empty())
        .unwrap_or("command")
        .to_string()
}
