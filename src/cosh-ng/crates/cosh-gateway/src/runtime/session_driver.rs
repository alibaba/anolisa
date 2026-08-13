//! Responsive single-owner ACP session orchestration over supervised stdio.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use thiserror::Error;

use super::{
    AcpV1BridgeError, AcpV1BridgeRead, AcpV1ClientConfig, AcpV1Observation,
    AcpV1PermissionDecision, AcpV1RequestId, AcpV1RuntimeBridge, ProcessTerminal,
    RuntimeLaunchSpec,
};

const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const COMMAND_CAPACITY: usize = 8;
const CONTROL_CAPACITY: usize = 1;
const EVENT_CAPACITY: usize = 32;
const MAX_TERMINAL_DETAIL_BYTES: usize = 4 * 1024;

/// Deadlines and immutable launch inputs for one local ACP session.
#[derive(Debug, Clone)]
pub struct AcpSessionDriverConfig {
    /// Direct supervised Agent launch specification.
    pub launch: RuntimeLaunchSpec,
    /// ACP client identity and frame bound.
    pub client: AcpV1ClientConfig,
    /// Canonical workspace bound to the single Agent session.
    pub workspace: PathBuf,
    /// Optional workspace roots passed only when the Agent advertises support.
    pub additional_directories: Vec<PathBuf>,
    /// Maximum wait for initialize and `session/new` responses.
    pub initialize_timeout: Duration,
    /// Maximum lifetime of one active prompt.
    pub prompt_timeout: Duration,
    /// TERM grace before KILL escalation during settlement.
    pub shutdown_grace: Duration,
    /// Maximum caller wait for actor acknowledgements.
    pub command_timeout: Duration,
}

impl AcpSessionDriverConfig {
    /// Builds a local single-session configuration with conservative deadlines.
    #[must_use]
    pub fn new(
        launch: RuntimeLaunchSpec,
        client: AcpV1ClientConfig,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        Self {
            launch,
            client,
            workspace: workspace.into(),
            additional_directories: Vec::new(),
            initialize_timeout: Duration::from_secs(10),
            prompt_timeout: Duration::from_secs(30 * 60),
            shutdown_grace: Duration::from_secs(2),
            command_timeout: Duration::from_secs(10),
        }
    }
}

/// One bounded event delivered by the ACP session actor.
#[derive(Debug)]
pub enum AcpSessionEvent {
    /// Validated protocol observation in wire order.
    Observation(AcpV1Observation),
    /// The sole terminal event for this driver generation.
    Terminal(AcpSessionTerminal),
}

/// Stable reason for the sole session-driver terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpSessionTerminalKind {
    /// Caller requested orderly shutdown.
    Shutdown,
    /// Independent control handle cancelled the active prompt.
    Cancelled,
    /// Protocol, transport, deadline, or actor coordination failed closed.
    Failed,
}

/// Final driver result emitted after the runtime child has been reaped.
#[derive(Debug)]
pub struct AcpSessionTerminal {
    /// Stable terminal classification.
    pub kind: AcpSessionTerminalKind,
    /// Bounded diagnostic without protocol payloads or secrets.
    pub detail: Option<String>,
    /// Reaped process terminal when cleanup returned it.
    pub process: Option<ProcessTerminal>,
}

/// Failure returned to a session-driver caller.
#[derive(Debug, Error)]
pub enum AcpSessionDriverError {
    /// Launching the supervised bridge failed before an actor was exposed.
    #[error(transparent)]
    Bridge(#[from] AcpV1BridgeError),
    /// A command was rejected in the current driver state.
    #[error("ACP session command {operation} is invalid while state is {state}")]
    InvalidState {
        /// Requested operation.
        operation: &'static str,
        /// Compact actor state name.
        state: &'static str,
    },
    /// A mandatory response did not arrive before its explicit deadline.
    #[error("ACP {operation} exceeded its deadline")]
    Deadline {
        /// Timed-out operation.
        operation: &'static str,
    },
    /// The actor or bounded queue is unavailable.
    #[error("ACP session actor is unavailable")]
    ActorUnavailable,
    /// The independent cancellation slot already contains a request.
    #[error("ACP cancellation is already pending")]
    CancellationPending,
    /// An event consumer failed to keep up with the bounded stream.
    #[error("ACP observation queue reached its bound")]
    ObservationBackpressure,
    /// Independent control cancelled a deadline-bound operation.
    #[error("ACP operation was cancelled")]
    Cancelled,
}

type Reply = SyncSender<Result<(), AcpSessionDriverError>>;

#[derive(Debug)]
enum DriverCommand {
    Initialize(Reply),
    OpenSession(Reply),
    Prompt {
        text: String,
        reply: Reply,
    },
    Permission {
        request_id: AcpV1RequestId,
        decision: AcpV1PermissionDecision,
        reply: Reply,
    },
    Shutdown(Reply),
}

/// Cloneable cancellation path that is independent from ordinary commands.
#[derive(Debug, Clone)]
pub struct AcpSessionControl {
    cancel: SyncSender<()>,
}

impl AcpSessionControl {
    /// Enqueues cancellation without waiting for Agent stdout or actor work.
    ///
    /// # Errors
    ///
    /// Returns when cancellation is already pending or the actor exited.
    pub fn cancel(&self) -> Result<(), AcpSessionDriverError> {
        match self.cancel.try_send(()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(())) => Err(AcpSessionDriverError::CancellationPending),
            Err(TrySendError::Disconnected(())) => Err(AcpSessionDriverError::ActorUnavailable),
        }
    }
}

/// Public handle for one actor-owned ACP connection and session.
#[derive(Debug)]
pub struct AcpSessionDriver {
    commands: SyncSender<DriverCommand>,
    events: Receiver<AcpSessionEvent>,
    terminal: Receiver<AcpSessionTerminal>,
    control: AcpSessionControl,
    actor: Option<JoinHandle<()>>,
    command_timeout: Duration,
}

impl AcpSessionDriver {
    /// Launches the Agent and starts the sole bridge owner thread.
    ///
    /// # Errors
    ///
    /// Returns bridge launch or actor thread creation failures.
    pub fn launch(config: AcpSessionDriverConfig) -> Result<Self, AcpSessionDriverError> {
        let bridge = AcpV1RuntimeBridge::launch(&config.launch, config.client.clone())?;
        let (command_sender, command_receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (cancel_sender, cancel_receiver) = mpsc::sync_channel(CONTROL_CAPACITY);
        let (event_sender, event_receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let (terminal_sender, terminal_receiver) = mpsc::sync_channel(1);
        let command_timeout = config.command_timeout;
        let actor = thread::Builder::new()
            .name("cosh-acp-session".to_owned())
            .spawn(move || {
                run_actor(
                    bridge,
                    config,
                    command_receiver,
                    cancel_receiver,
                    event_sender,
                    terminal_sender,
                )
            })
            .map_err(|_| AcpSessionDriverError::ActorUnavailable)?;
        Ok(Self {
            commands: command_sender,
            events: event_receiver,
            terminal: terminal_receiver,
            control: AcpSessionControl {
                cancel: cancel_sender,
            },
            actor: Some(actor),
            command_timeout,
        })
    }

    /// Returns an independent cancellation handle.
    #[must_use]
    pub fn control(&self) -> AcpSessionControl {
        self.control.clone()
    }

    /// Negotiates ACP wire version 1 within the initialization deadline.
    pub fn initialize(&self) -> Result<(), AcpSessionDriverError> {
        self.request(DriverCommand::Initialize)
    }

    /// Opens the single configured canonical workspace session.
    pub fn open_session(&self) -> Result<(), AcpSessionDriverError> {
        self.request(DriverCommand::OpenSession)
    }

    /// Starts the only active text prompt.
    pub fn prompt(&self, text: impl Into<String>) -> Result<(), AcpSessionDriverError> {
        let text = text.into();
        self.request(move |reply| DriverCommand::Prompt { text, reply })
    }

    /// Answers one correlated permission callback exactly once.
    pub fn answer_permission(
        &self,
        request_id: AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<(), AcpSessionDriverError> {
        self.request(move |reply| DriverCommand::Permission {
            request_id,
            decision,
            reply,
        })
    }

    /// Receives one event before `timeout` expires.
    pub fn receive_timeout(&self, timeout: Duration) -> Result<AcpSessionEvent, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            match self
                .events
                .recv_timeout(remaining.min(CONTROL_POLL_INTERVAL))
            {
                Ok(event) => return Ok(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return self
                        .terminal
                        .recv_timeout(remaining)
                        .map(AcpSessionEvent::Terminal);
                }
            }
        }
    }

    /// Requests orderly process settlement.
    pub fn shutdown(&self) -> Result<(), AcpSessionDriverError> {
        self.request(DriverCommand::Shutdown)
    }

    fn request<F>(&self, build: F) -> Result<(), AcpSessionDriverError>
    where
        F: FnOnce(Reply) -> DriverCommand,
    {
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        let deadline = Instant::now() + self.command_timeout;
        let mut command = build(reply_sender);
        loop {
            match self.commands.try_send(command) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    command = returned;
                    if Instant::now() >= deadline {
                        let _ = self.control.cancel();
                        return Err(AcpSessionDriverError::Deadline {
                            operation: "command queue",
                        });
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err(AcpSessionDriverError::ActorUnavailable);
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match reply_receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(_) => {
                let _ = self.control.cancel();
                Err(AcpSessionDriverError::Deadline {
                    operation: "command acknowledgement",
                })
            }
        }
    }
}

impl Drop for AcpSessionDriver {
    fn drop(&mut self) {
        let _ = self.control.cancel();
        if let Some(actor) = self.actor.take() {
            let _ = actor.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActorState {
    Created,
    Initialized,
    SessionOpen,
    PromptActive,
    Terminal,
}

impl ActorState {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Initialized => "initialized",
            Self::SessionOpen => "session-open",
            Self::PromptActive => "prompt-active",
            Self::Terminal => "terminal",
        }
    }
}

fn run_actor(
    mut bridge: AcpV1RuntimeBridge,
    config: AcpSessionDriverConfig,
    commands: Receiver<DriverCommand>,
    cancel: Receiver<()>,
    events: SyncSender<AcpSessionEvent>,
    terminal: SyncSender<AcpSessionTerminal>,
) {
    let mut state = ActorState::Created;
    let mut prompt_deadline = None;
    loop {
        match cancel.try_recv() {
            Ok(()) => {
                settle_cancel(&mut bridge, &config, &terminal, state);
                break;
            }
            Err(TryRecvError::Disconnected | TryRecvError::Empty) => {}
        }

        match commands.try_recv() {
            Ok(command) => {
                if handle_command(
                    command,
                    &mut bridge,
                    &config,
                    &events,
                    &terminal,
                    &cancel,
                    &mut state,
                    &mut prompt_deadline,
                ) {
                    break;
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => {
                settle_cancel(&mut bridge, &config, &terminal, state);
                break;
            }
            Err(TryRecvError::Empty) => {}
        }

        if state != ActorState::PromptActive {
            thread::sleep(CONTROL_POLL_INTERVAL);
            continue;
        }
        if prompt_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            fail_terminal(
                &mut bridge,
                &config,
                &terminal,
                AcpSessionDriverError::Deadline {
                    operation: "prompt",
                },
            );
            break;
        }
        match bridge.read_observation_timeout(CONTROL_POLL_INTERVAL) {
            Ok(AcpV1BridgeRead::TimedOut) => {}
            Ok(AcpV1BridgeRead::Observation(observation)) => {
                if let Err(error) = settle_unsupported(&mut bridge, &observation) {
                    fail_terminal(&mut bridge, &config, &terminal, error);
                    break;
                }
                let finished = matches!(
                    observation,
                    AcpV1Observation::PromptFinished { .. }
                        | AcpV1Observation::RequestFailed { .. }
                );
                if emit_observation(&events, observation).is_err() {
                    fail_terminal(
                        &mut bridge,
                        &config,
                        &terminal,
                        AcpSessionDriverError::ObservationBackpressure,
                    );
                    break;
                }
                if finished {
                    state = ActorState::SessionOpen;
                    prompt_deadline = None;
                }
            }
            Err(error) => {
                fail_terminal(&mut bridge, &config, &terminal, error.into());
                break;
            }
        }
    }
}

fn handle_command(
    command: DriverCommand,
    bridge: &mut AcpV1RuntimeBridge,
    config: &AcpSessionDriverConfig,
    events: &SyncSender<AcpSessionEvent>,
    terminal_events: &SyncSender<AcpSessionTerminal>,
    cancel: &Receiver<()>,
    state: &mut ActorState,
    prompt_deadline: &mut Option<Instant>,
) -> bool {
    let (reply, result, terminal) = match command {
        DriverCommand::Initialize(reply) => {
            let result = require_state(*state, ActorState::Created, "initialize").and_then(|()| {
                bridge.send_initialize()?;
                wait_for(
                    bridge,
                    events,
                    cancel,
                    config.initialize_timeout,
                    "initialize",
                    |observation| matches!(observation, AcpV1Observation::Initialized { .. }),
                )?;
                *state = ActorState::Initialized;
                Ok(())
            });
            (reply, result, false)
        }
        DriverCommand::OpenSession(reply) => {
            let result =
                require_state(*state, ActorState::Initialized, "open_session").and_then(|()| {
                    bridge.send_new_session(
                        config.workspace.clone(),
                        config.additional_directories.clone(),
                    )?;
                    wait_for(
                        bridge,
                        events,
                        cancel,
                        config.initialize_timeout,
                        "session/new",
                        |observation| matches!(observation, AcpV1Observation::SessionOpened { .. }),
                    )?;
                    *state = ActorState::SessionOpen;
                    Ok(())
                });
            (reply, result, false)
        }
        DriverCommand::Prompt { text, reply } => {
            let result = require_state(*state, ActorState::SessionOpen, "prompt").and_then(|()| {
                bridge.send_prompt(text)?;
                *state = ActorState::PromptActive;
                *prompt_deadline = Some(Instant::now() + config.prompt_timeout);
                Ok(())
            });
            (reply, result, false)
        }
        DriverCommand::Permission {
            request_id,
            decision,
            reply,
        } => {
            let result = require_state(*state, ActorState::PromptActive, "answer_permission")
                .and_then(|()| {
                    bridge.send_permission_decision(&request_id, decision)?;
                    Ok(())
                });
            (reply, result, false)
        }
        DriverCommand::Shutdown(reply) => {
            let result = settle(
                bridge,
                config,
                terminal_events,
                AcpSessionTerminalKind::Shutdown,
                None,
            );
            *state = ActorState::Terminal;
            (reply, result, true)
        }
    };
    let fatal = result.as_ref().err().and_then(|error| match error {
        AcpSessionDriverError::InvalidState { .. } => None,
        AcpSessionDriverError::Cancelled => {
            Some((AcpSessionTerminalKind::Cancelled, error.to_string()))
        }
        error => Some((AcpSessionTerminalKind::Failed, error.to_string())),
    });
    let _ = reply.send(result);
    if terminal {
        return true;
    }
    if let Some((kind, detail)) = fatal {
        let _ = settle(bridge, config, terminal_events, kind, Some(detail));
        *state = ActorState::Terminal;
        return true;
    }
    false
}

fn require_state(
    actual: ActorState,
    expected: ActorState,
    operation: &'static str,
) -> Result<(), AcpSessionDriverError> {
    if actual == expected {
        Ok(())
    } else {
        Err(AcpSessionDriverError::InvalidState {
            operation,
            state: actual.name(),
        })
    }
}

fn wait_for(
    bridge: &mut AcpV1RuntimeBridge,
    events: &SyncSender<AcpSessionEvent>,
    cancel: &Receiver<()>,
    timeout: Duration,
    operation: &'static str,
    expected: impl Fn(&AcpV1Observation) -> bool,
) -> Result<(), AcpSessionDriverError> {
    let deadline = Instant::now() + timeout;
    loop {
        match cancel.try_recv() {
            Ok(()) => return Err(AcpSessionDriverError::Cancelled),
            Err(TryRecvError::Disconnected | TryRecvError::Empty) => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AcpSessionDriverError::Deadline { operation });
        }
        match bridge.read_observation_timeout(remaining.min(CONTROL_POLL_INTERVAL))? {
            AcpV1BridgeRead::TimedOut => {}
            AcpV1BridgeRead::Observation(observation) => {
                settle_unsupported(bridge, &observation)?;
                let matched = expected(&observation);
                emit_observation(events, observation)?;
                if matched {
                    return Ok(());
                }
            }
        }
    }
}

fn settle_unsupported(
    bridge: &mut AcpV1RuntimeBridge,
    observation: &AcpV1Observation,
) -> Result<(), AcpSessionDriverError> {
    if let AcpV1Observation::UnsupportedClientRequest { request_id, .. } = observation {
        bridge.reject_unsupported_request(request_id)?;
    }
    Ok(())
}

fn emit_observation(
    events: &SyncSender<AcpSessionEvent>,
    observation: AcpV1Observation,
) -> Result<(), AcpSessionDriverError> {
    events
        .try_send(AcpSessionEvent::Observation(observation))
        .map_err(|_| AcpSessionDriverError::ObservationBackpressure)
}

fn settle_cancel(
    bridge: &mut AcpV1RuntimeBridge,
    config: &AcpSessionDriverConfig,
    terminal: &SyncSender<AcpSessionTerminal>,
    state: ActorState,
) {
    let detail = if state == ActorState::PromptActive {
        bridge.send_cancel().err().map(|error| error.to_string())
    } else {
        None
    };
    let _ = settle(
        bridge,
        config,
        terminal,
        AcpSessionTerminalKind::Cancelled,
        detail,
    );
}

fn fail_terminal(
    bridge: &mut AcpV1RuntimeBridge,
    config: &AcpSessionDriverConfig,
    terminal: &SyncSender<AcpSessionTerminal>,
    error: AcpSessionDriverError,
) {
    let _ = settle(
        bridge,
        config,
        terminal,
        AcpSessionTerminalKind::Failed,
        Some(error.to_string()),
    );
}

fn settle(
    bridge: &mut AcpV1RuntimeBridge,
    config: &AcpSessionDriverConfig,
    terminal_events: &SyncSender<AcpSessionTerminal>,
    kind: AcpSessionTerminalKind,
    detail: Option<String>,
) -> Result<(), AcpSessionDriverError> {
    let shutdown = bridge.shutdown(config.shutdown_grace);
    let (process, cleanup_error) = match &shutdown {
        Ok(process) => (process.clone(), None),
        Err(error) => (
            bridge.poll_terminal().ok().flatten(),
            Some(error.to_string()),
        ),
    };
    let detail = match (detail, cleanup_error) {
        (Some(detail), Some(cleanup)) => Some(format!("{detail}; cleanup failed: {cleanup}")),
        (Some(detail), None) => Some(detail),
        (None, Some(cleanup)) => Some(format!("cleanup failed: {cleanup}")),
        (None, None) => None,
    }
    .map(|detail| bounded_detail(&detail));
    // The dedicated one-shot slot reserves terminal delivery even when a
    // consumer stopped draining the bounded observation stream.
    let _ = terminal_events.try_send(AcpSessionTerminal {
        kind,
        detail,
        process,
    });
    match shutdown {
        Ok(_) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn bounded_detail(detail: &str) -> String {
    if detail.len() <= MAX_TERMINAL_DETAIL_BYTES {
        return detail.to_owned();
    }
    let mut end = MAX_TERMINAL_DETAIL_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

#[cfg(test)]
#[path = "session_driver/tests.rs"]
mod tests;
