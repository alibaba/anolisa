#![forbid(unsafe_code)]
//! Installed local entrypoint for ACP v1 and the local Task control plane.
//!
//! Owner note: the first installed-control slice stays beside the existing ACP
//! command wiring so exit and presentation contracts remain centralized. Split
//! control commands into a sibling module before expanding this surface.

use std::fs::File;
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand, ValueEnum};
use cosh_gateway::daemon::{
    AppendTaskInput, CancelTask, GatewayDaemon, GatewayDaemonConfig, GatewayResult,
    LocalGatewayClient, ResolveApproval, RetryTask, SubmitTask,
};
use cosh_gateway::permission::{
    CancelPermissionPresenter, FilePermissionEvidenceSink, OncePermissionProxy,
    PermissionEvidenceContext, PermissionPresenter, TextPermissionPresenter,
};
use cosh_gateway::runtime::{
    AcpRuntimeProfileId, AcpRuntimeProfileRequest, AcpRuntimeProfileResolver, AcpSessionDriver,
    AcpSessionDriverConfig, AcpSessionEvent, AcpSessionObservation, AcpSessionTerminalKind,
    AcpV1ClientConfig, AcpV1Observation, AcpV1PermissionDecision, AcpV1PermissionOptionKind,
    AcpV1StopReason, InstalledBrokeredCoreRuntimePortFactory, LinuxSystemdContainmentVerifier,
    LocalOsActorResolver, ScheduledAgentRuntimeFactory, TrustedWorkspaceResolver,
    GATEWAY_BROKERED_CORE_RUNTIME_PROFILE,
};
use cosh_gateway::storage::{inspect_task_store, StoreInspectionOutcome};
use cosh_gateway_contracts::{
    capability::ApprovalDecision,
    common::{BoundedName, BoundedOpaque, BoundedText, IdempotencyKey, RuntimeSelector, TargetRef},
    ids::{
        ApprovalId, InputRequestId, InstallationId, RequestId, RunId, RuntimeInstanceId, TaskId,
    },
    runtime::{RuntimeInputResponse, RuntimeInputSelections},
};
use serde_json::{json, Value};
use thiserror::Error;

#[path = "cosh_gateway/serve.rs"]
mod serve;

use serve::{serve, ServeArgs};

const MAX_ACP_FRAME_BYTES: usize = 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EVENT_DEADLINE: Duration = Duration::from_secs(15);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

// Task submission is intentionally bound to a side-effect-free target. The
// target identity remains explicit in durable Task records without exposing
// target assembly to a local CLI caller.
const TASK_ONLY_TARGET_KIND: &str = "workspace";
const TASK_ONLY_TARGET_AUTHORITY: &str = "cosh";
const TASK_ONLY_TARGET_IDENTIFIER: &str = "task-only-v1";

const EXIT_INPUT: u8 = 10;
const EXIT_PROFILE: u8 = 11;
const EXIT_RUNTIME: u8 = 12;
const EXIT_AGENT: u8 = 13;
const EXIT_STORE_INSPECTION: u8 = 14;
const EXIT_CANCELLED: u8 = 130;

#[derive(Debug, Parser)]
#[command(
    name = "cosh-gateway",
    version,
    about = "Run ACP interoperability and brokered local Gateway tasks through COSH"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verify an installed adapter through initialize and session/new.
    Doctor(ProfileArgs),
    /// Run one text prompt read from stdin or an explicit file.
    Run(RunArgs),
    /// Run the brokered local Task control daemon.
    Serve(ServeArgs),
    /// Submit, inspect, follow, or cancel durable Tasks through the daemon.
    Task(TaskArgs),
    /// Run local read-only Gateway administration commands.
    Admin(AdminArgs),
}

#[derive(Debug, Clone, Args)]
struct AdminArgs {
    /// Presentation format for bounded local diagnostics.
    #[arg(long, value_enum, default_value_t = Output::Human)]
    output: Output,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Debug, Clone, Subcommand)]
enum AdminCommand {
    /// Inspect an existing Task store without migration or repair.
    Inspect(AdminInspectArgs),
}

#[derive(Debug, Clone, Args)]
struct AdminInspectArgs {
    /// Absolute path to an existing private Gateway SQLite database.
    #[arg(long, value_name = "PATH")]
    database: PathBuf,
}

#[derive(Debug, Clone, Args)]
struct TaskArgs {
    /// Absolute Unix socket path; defaults below the user runtime directory.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Presentation format for bounded daemon responses.
    #[arg(long, value_enum, default_value_t = Output::Human)]
    output: Output,
    #[command(subcommand)]
    command: TaskCommand,
}

#[derive(Debug, Clone, Subcommand)]
enum TaskCommand {
    /// Create one durable Task from stdin or a regular file.
    Submit(TaskSubmitArgs),
    /// Read the current durable Task projection.
    Get(TaskIdArgs),
    /// Read a bounded page of durable Task events.
    Events(TaskEventsArgs),
    /// Request cancellation of the active Task Run.
    Cancel(TaskCancelArgs),
    /// Resolve a pending Runtime or brokered approval.
    ResolveApproval(TaskResolveApprovalArgs),
    /// Append one exact response to a pending Runtime question.
    Append(TaskAppendArgs),
    /// Queue a replacement for one exact suspended Run.
    Retry(TaskRetryArgs),
}

#[derive(Debug, Clone, Args)]
struct TaskSubmitArgs {
    /// Read Task intent from this regular file; default is stdin.
    #[arg(long, value_name = "PATH")]
    intent_file: Option<PathBuf>,
    /// Caller-stable replay key; generate once and reuse after uncertain I/O.
    #[arg(long, value_name = "KEY")]
    idempotency_key: String,
    /// Runtime kind requested for the first Run. The production daemon admits
    /// only its configured brokered Core selector; other values are rejected
    /// at daemon admission and cannot launch an ACP session through this CLI.
    #[arg(long, default_value = "core")]
    runtime: String,
    /// Runtime profile requested for the first Run. The production daemon
    /// accepts only its configured brokered Core profile.
    #[arg(long, default_value = GATEWAY_BROKERED_CORE_RUNTIME_PROFILE)]
    runtime_profile: String,
}

#[derive(Debug, Clone, Args)]
struct TaskIdArgs {
    /// Canonical COSH Task ID.
    #[arg(value_name = "TASK_ID")]
    task_id: String,
}

#[derive(Debug, Clone, Args)]
struct TaskCancelArgs {
    /// Canonical COSH Task ID.
    #[arg(value_name = "TASK_ID")]
    task_id: String,
    /// Active Run being cancelled.
    #[arg(long, value_name = "RUN_ID")]
    run_id: String,
    /// Caller-stable replay key; generate once and reuse after uncertain I/O.
    #[arg(long, value_name = "KEY")]
    idempotency_key: String,
    /// Reject cancellation if the Task has advanced beyond this revision.
    #[arg(long)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Args)]
struct TaskRetryArgs {
    /// Canonical COSH Task ID.
    #[arg(value_name = "TASK_ID")]
    task_id: String,
    /// Exact suspended Run being replaced.
    #[arg(long, value_name = "RUN_ID")]
    previous_run_id: String,
    /// Caller-stable replay key; generate once and reuse after uncertain I/O.
    #[arg(long, value_name = "KEY")]
    idempotency_key: String,
    /// Reject retry if the Task has advanced beyond this revision.
    #[arg(long)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Args)]
struct TaskResolveApprovalArgs {
    /// Canonical approval identity from the Task event stream.
    #[arg(value_name = "APPROVAL_ID")]
    approval_id: String,
    /// Approve once or deny the pending operation.
    #[arg(long, value_enum)]
    decision: ApprovalChoice,
    /// Caller-stable replay key; generate once and reuse after uncertain I/O.
    #[arg(long, value_name = "KEY")]
    idempotency_key: String,
}

#[derive(Debug, Clone, Args)]
struct TaskAppendArgs {
    /// Canonical COSH Task ID.
    #[arg(value_name = "TASK_ID")]
    task_id: String,
    /// Exact input request identity from the Task event stream.
    #[arg(long, value_name = "INPUT_REQUEST_ID")]
    input_request_id: String,
    /// Read free-text input from this regular file; default is stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "selections")]
    input_file: Option<PathBuf>,
    /// Select one zero-based option index; repeat for multi-select questions.
    #[arg(long = "select", value_name = "INDEX")]
    selections: Vec<u16>,
    /// Caller-stable replay key; generate once and reuse after uncertain I/O.
    #[arg(long, value_name = "KEY")]
    idempotency_key: String,
    /// Reject append if the Task has advanced beyond this revision.
    #[arg(long)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Args)]
struct TaskEventsArgs {
    /// Canonical COSH Task ID.
    #[arg(value_name = "TASK_ID")]
    task_id: String,
    /// Last durable Task revision already observed.
    #[arg(long, default_value_t = 0)]
    after: u64,
    /// Maximum events returned in one bounded page.
    #[arg(long, default_value_t = 64, value_parser = clap::value_parser!(u16).range(1..=64))]
    limit: u16,
}

#[derive(Debug, Clone, Args)]
struct ProfileArgs {
    /// Fixed installed adapter profile.
    #[arg(long, value_enum, default_value_t = Profile::Codex)]
    profile: Profile,
    /// Absolute trusted adapter path; basename must match the profile.
    #[arg(long)]
    adapter: Option<PathBuf>,
    /// Existing workspace directory bound to the ACP session.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    /// Presentation format for stable COSH events and errors.
    #[arg(long, value_enum, default_value_t = Output::Human)]
    output: Output,
}

#[derive(Debug, Clone, Args)]
struct RunArgs {
    #[command(flatten)]
    profile: ProfileArgs,
    /// Read the prompt from this regular file; default is stdin.
    #[arg(long, value_name = "PATH")]
    prompt_file: Option<PathBuf>,
    /// Prompt on the local controlling terminal or deny every tool request.
    #[arg(long, value_enum, default_value_t = PermissionMode::Prompt)]
    permission: PermissionMode,
    /// Absolute private JSONL evidence path; defaults below the user state directory.
    #[arg(long, value_name = "PATH")]
    permission_evidence: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum Profile {
    #[default]
    Codex,
    ClaudeCode,
}

impl From<Profile> for AcpRuntimeProfileId {
    fn from(profile: Profile) -> Self {
        match profile {
            Profile::Codex => Self::Codex,
            Profile::ClaudeCode => Self::ClaudeCode,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum Output {
    #[default]
    Human,
    Jsonl,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum PermissionMode {
    /// Ask on `/dev/tty`; cancel when no controlling terminal is available.
    #[default]
    Prompt,
    /// Cancel every permission callback without presenting it.
    Deny,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ApprovalChoice {
    Approve,
    Deny,
}

impl From<ApprovalChoice> for ApprovalDecision {
    fn from(value: ApprovalChoice) -> Self {
        match value {
            ApprovalChoice::Approve => Self::Approve,
            ApprovalChoice::Deny => Self::Deny,
        }
    }
}

#[derive(Debug, Error)]
enum CliError {
    #[error("failed to resolve installed ACP profile: {0}")]
    Profile(String),
    #[error("failed to read prompt: {0}")]
    Input(#[source] io::Error),
    #[error("invalid control request: {0}")]
    InvalidInput(String),
    #[error("failed to read task intent: {0}")]
    IntentInput(#[source] io::Error),
    #[error("prompt path is not a regular file: {0}")]
    PromptNotRegular(PathBuf),
    #[error("prompt is empty")]
    EmptyPrompt,
    #[error("prompt exceeds the {MAX_PROMPT_BYTES}-byte limit")]
    PromptTooLarge,
    #[error("task intent path is not a regular file: {0}")]
    IntentNotRegular(PathBuf),
    #[error("task intent is empty")]
    EmptyIntent,
    #[error("task intent exceeds the {MAX_PROMPT_BYTES}-byte limit")]
    IntentTooLarge,
    #[error("failed to register interrupt handling: {0}")]
    Signal(#[source] io::Error),
    #[error("local permission handling failed: {0}")]
    Permission(String),
    #[error("ACP runtime failed: {0}")]
    Runtime(String),
    #[error("Gateway daemon request failed: {0}")]
    Daemon(String),
    #[error("Gateway Runtime containment failed: {0}")]
    Containment(String),
    #[error("Gateway store inspection failed: {0}")]
    StoreInspection(String),
    #[error("ACP Agent rejected or did not complete the prompt")]
    Agent,
    #[error("ACP operation was cancelled")]
    Cancelled,
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Input(_)
            | Self::InvalidInput(_)
            | Self::IntentInput(_)
            | Self::PromptNotRegular(_)
            | Self::EmptyPrompt
            | Self::PromptTooLarge
            | Self::IntentNotRegular(_)
            | Self::EmptyIntent
            | Self::IntentTooLarge => EXIT_INPUT,
            Self::Profile(_) => EXIT_PROFILE,
            Self::Runtime(_)
            | Self::Daemon(_)
            | Self::Containment(_)
            | Self::StoreInspection(_)
            | Self::Signal(_)
            | Self::Permission(_) => EXIT_RUNTIME,
            Self::Agent => EXIT_AGENT,
            Self::Cancelled => EXIT_CANCELLED,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Input(_) => "prompt_read_failed",
            Self::InvalidInput(_) => "invalid_request",
            Self::IntentInput(_) => "intent_read_failed",
            Self::PromptNotRegular(_) => "prompt_not_regular",
            Self::EmptyPrompt => "prompt_empty",
            Self::PromptTooLarge => "prompt_too_large",
            Self::IntentNotRegular(_) => "intent_not_regular",
            Self::EmptyIntent => "intent_empty",
            Self::IntentTooLarge => "intent_too_large",
            Self::Profile(_) => "profile_invalid",
            Self::Signal(_) => "signal_handler_failed",
            Self::Permission(_) => "permission_failed",
            Self::Runtime(_) => "runtime_failed",
            Self::Daemon(_) => "daemon_failed",
            Self::Containment(_) => "runtime_containment_unverified",
            Self::StoreInspection(_) => "store_inspection_failed",
            Self::Agent => "agent_incomplete",
            Self::Cancelled => "cancelled",
        }
    }
}

struct Reporter {
    output: Output,
}

impl Reporter {
    fn event(&self, event: &str, fields: Value) -> Result<(), CliError> {
        match self.output {
            Output::Jsonl => {
                let mut value = json!({"event": event});
                if let (Some(target), Some(source)) = (value.as_object_mut(), fields.as_object()) {
                    target.extend(source.clone());
                }
                println!("{value}");
            }
            Output::Human => self.human_event(event, &fields),
        }
        io::stdout()
            .flush()
            .map_err(|error| CliError::Runtime(error.to_string()))
    }

    fn human_event(&self, event: &str, fields: &Value) {
        match event {
            "initialized" => eprintln!("ACP v1 initialized"),
            "session_opened" => eprintln!("ACP session opened"),
            "session_update" => {
                if let Some(text) = fields.get("text").and_then(Value::as_str) {
                    print!("{}", terminal_safe(text));
                }
            }
            "permission_decided" => match fields.get("decision").and_then(Value::as_str) {
                Some("allow_once") => eprintln!("ACP permission allowed once"),
                Some("reject_once") => eprintln!("ACP permission rejected once"),
                _ => eprintln!("ACP permission request cancelled"),
            },
            "prompt_finished" => eprintln!("\nACP prompt finished"),
            "doctor_ok" => println!("ACP adapter is ready"),
            "terminal" => {}
            "daemon_ready" => eprintln!("COSH Gateway daemon is ready"),
            "task_submitted" => print_task_id(fields),
            "task" => println!("{}", human_json(fields)),
            "task_events" => println!("{}", human_json(fields)),
            "task_cancelled" => print_task_id(fields),
            "store_inspection" => println!("{}", human_json(fields)),
            _ => {}
        }
    }

    fn error(&self, error: &CliError) {
        match self.output {
            Output::Human => eprintln!("Error [{}]: {error}", error.code()),
            Output::Jsonl => println!(
                "{}",
                json!({"event":"error", "code":error.code(), "message":error.to_string()})
            ),
        }
    }
}

fn print_task_id(fields: &Value) {
    if let Some(task_id) = fields.get("task_id").and_then(Value::as_str) {
        println!("{task_id}");
    }
}

fn human_json(fields: &Value) -> String {
    serde_json::to_string_pretty(fields).unwrap_or_else(|_| "{}".to_owned())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = match &cli.command {
        Command::Doctor(args) => args.output,
        Command::Run(args) => args.profile.output,
        Command::Serve(args) => args.output,
        Command::Task(args) => args.output,
        Command::Admin(args) => args.output,
    };
    let reporter = Reporter { output };
    let result = match cli.command {
        Command::Doctor(args) => doctor(args, &reporter),
        Command::Run(args) => run(args, &reporter),
        Command::Serve(args) => serve(args, &reporter),
        Command::Task(args) => task(args, &reporter),
        Command::Admin(args) => admin(args, &reporter),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            reporter.error(&error);
            ExitCode::from(error.exit_code())
        }
    }
}

fn admin(args: AdminArgs, reporter: &Reporter) -> Result<u8, CliError> {
    match args.command {
        AdminCommand::Inspect(command) => {
            let report = inspect_task_store(command.database)
                .map_err(|error| CliError::StoreInspection(error.to_string()))?;
            let exit = if report.outcome == StoreInspectionOutcome::Healthy {
                0
            } else {
                EXIT_STORE_INSPECTION
            };
            reporter.event(
                "store_inspection",
                serde_json::to_value(report)
                    .map_err(|error| CliError::StoreInspection(error.to_string()))?,
            )?;
            Ok(exit)
        }
    }
}

fn doctor(args: ProfileArgs, reporter: &Reporter) -> Result<u8, CliError> {
    let interrupted = install_interrupt_handler()?;
    let (driver, _) = launch_driver(&args)?;
    initialize_session(&driver, reporter, &interrupted)?;
    driver
        .shutdown()
        .map_err(|error| CliError::Runtime(error.to_string()))?;
    wait_for_terminal(&driver, reporter, &interrupted)?;
    reporter.event("doctor_ok", json!({"profile": profile_name(args.profile)}))?;
    Ok(0)
}

fn run(args: RunArgs, reporter: &Reporter) -> Result<u8, CliError> {
    let evidence_path = permission_evidence_path(&args)?;
    let prompt = read_prompt(args.prompt_file.as_ref())?;
    let interrupted = install_interrupt_handler()?;
    let (driver, workspace) = launch_driver(&args.profile)?;
    let mut permissions = LocalPermissionHandler::new(&args, &workspace, evidence_path);
    initialize_session(&driver, reporter, &interrupted)?;
    driver
        .prompt(prompt)
        .map_err(|error| CliError::Runtime(error.to_string()))?;

    let mut cancel_sent = false;
    loop {
        if interrupted.load(Ordering::Relaxed) && !cancel_sent {
            driver
                .control()
                .cancel()
                .map_err(|error| CliError::Runtime(error.to_string()))?;
            cancel_sent = true;
        }
        match driver.receive_timeout(EVENT_POLL_INTERVAL) {
            Ok(AcpSessionEvent::Observation(observation)) => {
                if let Some(exit) =
                    handle_observation(&driver, reporter, observation, Some(&mut permissions))?
                {
                    driver
                        .shutdown()
                        .map_err(|error| CliError::Runtime(error.to_string()))?;
                    wait_for_terminal(&driver, reporter, &interrupted)?;
                    return Ok(exit);
                }
            }
            Ok(AcpSessionEvent::Terminal(terminal)) => {
                report_terminal(reporter, &terminal)?;
                return terminal_exit(terminal.kind).map(Ok)?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CliError::Runtime("ACP event channel closed".into()));
            }
        }
    }
}

fn task(args: TaskArgs, reporter: &Reporter) -> Result<u8, CliError> {
    let socket = daemon_socket_path(args.socket.as_ref())?;
    let client = LocalGatewayClient::new(socket);
    let result = match args.command {
        TaskCommand::Submit(command) => {
            let request = SubmitTask {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new(command.idempotency_key)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                intent: BoundedText::new(read_intent(command.intent_file.as_ref())?)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                target: task_only_target(),
                runtime: RuntimeSelector {
                    runtime: bounded_name(command.runtime)?,
                    profile: Some(bounded_name(command.runtime_profile)?),
                },
            };
            client.submit(request)
        }
        TaskCommand::Get(command) => client.get(RequestId::new(), parse_task(&command.task_id)?),
        TaskCommand::Events(command) => client.events(
            RequestId::new(),
            parse_task(&command.task_id)?,
            (command.after != 0).then_some(command.after),
            command.limit,
        ),
        TaskCommand::Cancel(command) => client.cancel(CancelTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            task_id: parse_task(&command.task_id)?,
            run_id: RunId::parse(&command.run_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            expected_revision: command.expected_revision,
        }),
        TaskCommand::Retry(command) => client.retry(RetryTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            task_id: parse_task(&command.task_id)?,
            previous_run_id: RunId::parse(&command.previous_run_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            expected_revision: command.expected_revision,
        }),
        TaskCommand::ResolveApproval(command) => client.resolve_approval(ResolveApproval {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            approval_id: ApprovalId::parse(&command.approval_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            decision: command.decision.into(),
        }),
        TaskCommand::Append(command) => {
            let response = if command.selections.is_empty() {
                RuntimeInputResponse::Text {
                    text: BoundedText::new(read_intent(command.input_file.as_ref())?)
                        .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                }
            } else {
                RuntimeInputResponse::Options {
                    selections: RuntimeInputSelections::new(command.selections)
                        .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                }
            };
            client.append_input(AppendTaskInput {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new(command.idempotency_key)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                task_id: parse_task(&command.task_id)?,
                input_request_id: InputRequestId::parse(&command.input_request_id)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                response,
                expected_revision: command.expected_revision,
            })
        }
    }
    .map_err(|error| CliError::Daemon(error.to_string()))?;
    report_gateway_result(reporter, result)?;
    Ok(0)
}

fn report_gateway_result(reporter: &Reporter, result: GatewayResult) -> Result<(), CliError> {
    match result {
        GatewayResult::Pong => reporter.event("daemon_pong", json!({})),
        GatewayResult::Task(task) => reporter.event(
            "task",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Events(events) => reporter.event(
            "task_events",
            serde_json::to_value(events).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Cancelled(task) => reporter.event(
            "task_cancelled",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Retried(task) => reporter.event(
            "task_retried",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::InputAppended(task) => reporter.event(
            "task_input_appended",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::ApprovalResolved(task) => reporter.event(
            "approval_resolved",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
    }
}

fn bounded_name(value: String) -> Result<BoundedName, CliError> {
    BoundedName::new(value).map_err(|error| CliError::InvalidInput(error.to_string()))
}

fn task_only_target() -> TargetRef {
    TargetRef {
        kind: BoundedName::new(TASK_ONLY_TARGET_KIND).unwrap_or_else(|_| unreachable!()),
        authority: BoundedName::new(TASK_ONLY_TARGET_AUTHORITY).unwrap_or_else(|_| unreachable!()),
        identifier: BoundedOpaque::new(TASK_ONLY_TARGET_IDENTIFIER)
            .unwrap_or_else(|_| unreachable!()),
    }
}

fn parse_task(value: &str) -> Result<TaskId, CliError> {
    TaskId::parse(value).map_err(|error| CliError::InvalidInput(error.to_string()))
}

fn daemon_socket_path(explicit: Option<&PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return require_absolute(path, "daemon socket");
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return require_absolute(&PathBuf::from(runtime), "XDG_RUNTIME_DIR")
            .map(|path| path.join("cosh/gateway.sock"));
    }
    Ok(PathBuf::from(format!(
        "/run/user/{}/cosh/gateway.sock",
        nix::unistd::Uid::effective().as_raw()
    )))
}

fn daemon_database_path(explicit: Option<&PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return require_absolute(path, "daemon database");
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return require_absolute(&PathBuf::from(state), "XDG_STATE_HOME")
            .map(|path| path.join("cosh/gateway/state.db"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Daemon("absolute HOME is required".to_owned()))?;
    require_absolute(&home, "HOME").map(|path| path.join(".local/state/cosh/gateway/state.db"))
}

fn require_absolute(path: &Path, label: &str) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(CliError::Daemon(format!("{label} path must be absolute")))
    }
}

fn launch_driver(args: &ProfileArgs) -> Result<(AcpSessionDriver, PathBuf), CliError> {
    let request = AcpRuntimeProfileRequest::from_current_environment(
        args.profile.into(),
        args.adapter.clone(),
        &args.workspace,
    );
    let resolved = AcpRuntimeProfileResolver::resolve(request)
        .map_err(|error| CliError::Profile(error.to_string()))?;
    let workspace = resolved.workspace().to_path_buf();
    let config = AcpSessionDriverConfig::new(
        resolved.launch_spec(),
        AcpV1ClientConfig::new(
            "cosh-gateway",
            env!("CARGO_PKG_VERSION"),
            MAX_ACP_FRAME_BYTES,
        ),
        resolved.workspace(),
    );
    let driver =
        AcpSessionDriver::launch(config).map_err(|error| CliError::Runtime(error.to_string()))?;
    Ok((driver, workspace))
}

fn initialize_session(
    driver: &AcpSessionDriver,
    reporter: &Reporter,
    interrupted: &AtomicBool,
) -> Result<(), CliError> {
    check_interrupted(driver, interrupted)?;
    driver
        .initialize()
        .map_err(|error| CliError::Runtime(error.to_string()))?;
    wait_for_observation(driver, reporter, interrupted, |observation| {
        matches!(observation, AcpV1Observation::Initialized { .. })
    })?;
    check_interrupted(driver, interrupted)?;
    driver
        .open_session()
        .map_err(|error| CliError::Runtime(error.to_string()))?;
    wait_for_observation(driver, reporter, interrupted, |observation| {
        matches!(observation, AcpV1Observation::SessionOpened { .. })
    })
}

fn wait_for_observation(
    driver: &AcpSessionDriver,
    reporter: &Reporter,
    interrupted: &AtomicBool,
    expected: impl Fn(&AcpV1Observation) -> bool,
) -> Result<(), CliError> {
    let deadline = std::time::Instant::now() + EVENT_DEADLINE;
    loop {
        check_interrupted(driver, interrupted)?;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(CliError::Runtime(
                "ACP event delivery deadline exceeded".into(),
            ));
        }
        match driver.receive_timeout(remaining.min(EVENT_POLL_INTERVAL)) {
            Ok(AcpSessionEvent::Observation(observation)) => {
                let matched = expected(&observation.observation);
                handle_observation(driver, reporter, observation, None)?;
                if matched {
                    return Ok(());
                }
            }
            Ok(AcpSessionEvent::Terminal(terminal)) => {
                report_terminal(reporter, &terminal)?;
                return terminal_exit(terminal.kind).map(|_| ());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CliError::Runtime("ACP event channel closed".into()));
            }
        }
    }
}

fn handle_observation(
    driver: &AcpSessionDriver,
    reporter: &Reporter,
    observation: AcpSessionObservation,
    permissions: Option<&mut LocalPermissionHandler>,
) -> Result<Option<u8>, CliError> {
    let sequence = observation.sequence;
    let report = |event, fields| reporter.event(event, with_observation_sequence(sequence, fields));
    match observation.observation {
        AcpV1Observation::Initialized { agent_info, .. } => {
            report(
                "initialized",
                json!({"agent": agent_info.map(|info| json!({
                    "name": info.name, "version": info.version
                }))}),
            )?;
        }
        AcpV1Observation::SessionOpened { session_id } => {
            report("session_opened", json!({"session_id":session_id}))?;
        }
        AcpV1Observation::SessionUpdate { session_id, update } => {
            let text = update
                .get("content")
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str);
            if let Some(text) = text {
                report(
                    "session_update",
                    json!({"session_id":session_id, "text":text}),
                )?;
            } else {
                report(
                    "session_diagnostic",
                    json!({"session_id":session_id, "kind":"non_text_update"}),
                )?;
            }
        }
        AcpV1Observation::PermissionRequested(request) => {
            let request_id = request.request_id.clone();
            let resolved = permissions
                .ok_or_else(|| CliError::Permission("permission UI is unavailable".into()))
                .and_then(|handler| handler.resolve(&request));
            let (decision, decision_name) = match resolved {
                Ok(value) => value,
                Err(error) => {
                    let _ =
                        driver.answer_permission(request_id, AcpV1PermissionDecision::Cancelled);
                    return Err(error);
                }
            };
            driver
                .answer_permission(request_id, decision)
                .map_err(|error| CliError::Runtime(error.to_string()))?;
            report("permission_decided", json!({"decision":decision_name}))?;
        }
        AcpV1Observation::PromptFinished {
            session_id,
            stop_reason,
        } => {
            report(
                "prompt_finished",
                json!({"session_id":session_id, "stop_reason":stop_reason_name(stop_reason)}),
            )?;
            return Ok(Some(if stop_reason == AcpV1StopReason::EndTurn {
                0
            } else {
                EXIT_AGENT
            }));
        }
        AcpV1Observation::RequestFailed {
            request,
            code,
            message,
        } => {
            report(
                "request_failed",
                json!({"request":format!("{request:?}"), "code":code, "message":message}),
            )?;
            return Err(CliError::Agent);
        }
        AcpV1Observation::UnsupportedClientRequest { request_id, method } => {
            report(
                "unsupported_request",
                json!({"request_id":request_id.to_string(), "method":method}),
            )?;
        }
        AcpV1Observation::UnsupportedNotification { method } => {
            report("unsupported_notification", json!({"method":method}))?;
        }
        AcpV1Observation::TransportClosed => {
            return Err(CliError::Runtime("ACP transport closed".into()));
        }
    }
    Ok(None)
}

fn with_observation_sequence(sequence: u64, mut fields: Value) -> Value {
    if let Some(fields) = fields.as_object_mut() {
        fields.insert("sequence".to_owned(), Value::from(sequence));
    }
    fields
}

struct LocalPermissionHandler {
    mode: PermissionMode,
    profile: &'static str,
    workspace: Vec<u8>,
    evidence_path: PathBuf,
    evidence: Option<FilePermissionEvidenceSink>,
}

impl LocalPermissionHandler {
    fn new(args: &RunArgs, workspace: &Path, evidence_path: PathBuf) -> Self {
        #[cfg(unix)]
        use std::os::unix::ffi::OsStrExt;

        #[cfg(unix)]
        let workspace = workspace.as_os_str().as_bytes().to_vec();
        #[cfg(not(unix))]
        let workspace = workspace.to_string_lossy().as_bytes().to_vec();
        Self {
            mode: args.permission,
            profile: profile_name(args.profile.profile),
            workspace,
            evidence_path,
            evidence: None,
        }
    }

    fn resolve(
        &mut self,
        request: &cosh_gateway::runtime::AcpV1PermissionRequest,
    ) -> Result<(AcpV1PermissionDecision, &'static str), CliError> {
        let occurred_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CliError::Permission("system clock precedes the Unix epoch".into()))?
            .as_millis()
            .try_into()
            .map_err(|_| CliError::Permission("system clock is out of range".into()))?;
        let context = PermissionEvidenceContext {
            profile: self.profile,
            canonical_workspace: &self.workspace,
            actor_uid: nix::unistd::Uid::effective().as_raw(),
            occurred_at_ms,
        };
        if self.evidence.is_none() {
            self.evidence = Some(
                FilePermissionEvidenceSink::open_in_private_state(&self.evidence_path)
                    .map_err(|error| CliError::Permission(error.to_string()))?,
            );
        }
        let evidence = self
            .evidence
            .as_mut()
            .ok_or_else(|| CliError::Permission("permission evidence is unavailable".into()))?;
        let decision = match self.mode {
            PermissionMode::Deny => {
                resolve_permission(CancelPermissionPresenter, evidence, context, request)?
            }
            PermissionMode::Prompt => match local_terminal_presenter() {
                Some(presenter) => resolve_permission(presenter, evidence, context, request)?,
                None => resolve_permission(CancelPermissionPresenter, evidence, context, request)?,
            },
        };
        let name = match &decision {
            AcpV1PermissionDecision::Cancelled => "cancelled",
            AcpV1PermissionDecision::Selected { option_id } => request
                .options
                .iter()
                .find(|option| &option.option_id == option_id)
                .map_or("cancelled", |option| match option.kind {
                    AcpV1PermissionOptionKind::AllowOnce => "allow_once",
                    AcpV1PermissionOptionKind::RejectOnce => "reject_once",
                    _ => "cancelled",
                }),
        };
        Ok((decision, name))
    }
}

fn resolve_permission<P: PermissionPresenter>(
    presenter: P,
    evidence: &mut FilePermissionEvidenceSink,
    context: PermissionEvidenceContext<'_>,
    request: &cosh_gateway::runtime::AcpV1PermissionRequest,
) -> Result<AcpV1PermissionDecision, CliError> {
    let mut proxy = OncePermissionProxy::new(presenter, evidence);
    proxy
        .resolve(context, request)
        .map_err(|error| CliError::Permission(error.to_string()))
}

fn local_terminal_presenter() -> Option<TextPermissionPresenter<BufReader<File>, File>> {
    let terminal = File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    if !terminal.is_terminal() {
        return None;
    }
    let input = terminal.try_clone().ok()?;
    Some(TextPermissionPresenter::new(
        BufReader::new(input),
        terminal,
    ))
}

fn permission_evidence_path(args: &RunArgs) -> Result<PathBuf, CliError> {
    if let Some(path) = &args.permission_evidence {
        return if path.is_absolute() {
            Ok(path.clone())
        } else {
            Err(CliError::Permission(
                "permission evidence path must be absolute".into(),
            ))
        };
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        let state = PathBuf::from(state);
        if !state.is_absolute() {
            return Err(CliError::Permission(
                "XDG_STATE_HOME must be absolute".into(),
            ));
        }
        return Ok(state.join("cosh/gateway/permission-evidence.jsonl"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CliError::Permission("absolute HOME is required".into()))?;
    Ok(home.join(".local/state/cosh/gateway/permission-evidence.jsonl"))
}

fn wait_for_terminal(
    driver: &AcpSessionDriver,
    reporter: &Reporter,
    interrupted: &AtomicBool,
) -> Result<(), CliError> {
    let deadline = std::time::Instant::now() + SHUTDOWN_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(CliError::Runtime(
                "ACP shutdown event deadline exceeded".into(),
            ));
        }
        match driver.receive_timeout(remaining.min(EVENT_POLL_INTERVAL)) {
            Ok(AcpSessionEvent::Observation(observation)) => {
                handle_observation(driver, reporter, observation, None)?;
            }
            Ok(AcpSessionEvent::Terminal(terminal)) => {
                report_terminal(reporter, &terminal)?;
                return terminal_exit(terminal.kind).map(|_| ());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if interrupted.load(Ordering::Relaxed) {
                    let _ = driver.control().cancel();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CliError::Runtime("ACP terminal channel closed".into()));
            }
        }
    }
}

fn report_terminal(
    reporter: &Reporter,
    terminal: &cosh_gateway::runtime::AcpSessionTerminal,
) -> Result<(), CliError> {
    reporter.event(
        "terminal",
        json!({
            "kind":format!("{:?}", terminal.kind).to_ascii_lowercase(),
            "detail":terminal.detail,
        }),
    )
}

fn terminal_exit(kind: AcpSessionTerminalKind) -> Result<u8, CliError> {
    match kind {
        AcpSessionTerminalKind::Shutdown => Ok(0),
        AcpSessionTerminalKind::Cancelled => Err(CliError::Cancelled),
        AcpSessionTerminalKind::Failed => Err(CliError::Runtime("ACP session failed".into())),
    }
}

fn check_interrupted(driver: &AcpSessionDriver, interrupted: &AtomicBool) -> Result<(), CliError> {
    if interrupted.load(Ordering::Relaxed) {
        let _ = driver.control().cancel();
        Err(CliError::Cancelled)
    } else {
        Ok(())
    }
}

fn install_interrupt_handler() -> Result<Arc<AtomicBool>, CliError> {
    let interrupted = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&interrupted))
        .map_err(CliError::Signal)?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&interrupted))
        .map_err(CliError::Signal)?;
    Ok(interrupted)
}

fn read_prompt(path: Option<&PathBuf>) -> Result<String, CliError> {
    let mut input: Box<dyn Read> = match path {
        Some(path) => {
            let file = File::open(path).map_err(CliError::Input)?;
            if !file.metadata().map_err(CliError::Input)?.is_file() {
                return Err(CliError::PromptNotRegular(path.clone()));
            }
            Box::new(file)
        }
        None => {
            if io::stdin().is_terminal() {
                eprintln!("Enter prompt, then press Ctrl-D:");
            }
            Box::new(io::stdin())
        }
    };
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((MAX_PROMPT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(CliError::Input)?;
    if bytes.len() > MAX_PROMPT_BYTES {
        return Err(CliError::PromptTooLarge);
    }
    let prompt = String::from_utf8(bytes)
        .map_err(|error| CliError::Input(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    if prompt.trim().is_empty() {
        return Err(CliError::EmptyPrompt);
    }
    Ok(prompt)
}

fn read_intent(path: Option<&PathBuf>) -> Result<String, CliError> {
    let mut input: Box<dyn Read> = match path {
        Some(path) => {
            #[cfg(unix)]
            use std::os::unix::fs::OpenOptionsExt;

            let mut options = File::options();
            options.read(true);
            #[cfg(unix)]
            options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
            let file = options.open(path).map_err(CliError::IntentInput)?;
            if !file.metadata().map_err(CliError::IntentInput)?.is_file() {
                return Err(CliError::IntentNotRegular(path.clone()));
            }
            Box::new(file)
        }
        None => {
            if io::stdin().is_terminal() {
                eprintln!("Enter Task intent, then press Ctrl-D:");
            }
            Box::new(io::stdin())
        }
    };
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((MAX_PROMPT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(CliError::IntentInput)?;
    if bytes.len() > MAX_PROMPT_BYTES {
        return Err(CliError::IntentTooLarge);
    }
    let intent = String::from_utf8(bytes).map_err(|error| {
        CliError::IntentInput(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    if intent.trim().is_empty() {
        return Err(CliError::EmptyIntent);
    }
    Ok(intent)
}

fn terminal_safe(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\n' | '\t' => vec![character],
            character if character.is_control() => character.escape_default().collect(),
            character => vec![character],
        })
        .collect()
}

fn profile_name(profile: Profile) -> &'static str {
    match profile {
        Profile::Codex => "codex",
        Profile::ClaudeCode => "claude-code",
    }
}

fn stop_reason_name(reason: AcpV1StopReason) -> &'static str {
    match reason {
        AcpV1StopReason::EndTurn => "end_turn",
        AcpV1StopReason::MaxTokens => "max_tokens",
        AcpV1StopReason::MaxTurnRequests => "max_turn_requests",
        AcpV1StopReason::Refusal => "refusal",
        AcpV1StopReason::Cancelled => "cancelled",
        AcpV1StopReason::Unsupported => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_text_escapes_control_sequences() {
        assert_eq!(terminal_safe("ok\u{1b}[2J\rnext"), "ok\\u{1b}[2J\\rnext");
    }

    #[test]
    fn json_observation_fields_include_driver_sequence() {
        assert_eq!(
            with_observation_sequence(7, json!({"text": "chunk"})),
            json!({"sequence": 7, "text": "chunk"})
        );
    }

    #[test]
    fn cli_does_not_accept_prompt_as_an_argument() {
        assert!(Cli::try_parse_from(["cosh-gateway", "run", "secret prompt"]).is_err());
    }

    #[test]
    fn task_submit_does_not_accept_intent_as_an_argument() {
        assert!(Cli::try_parse_from(["cosh-gateway", "task", "submit", "private intent"]).is_err());
    }

    #[test]
    fn task_event_page_is_bounded_by_clap() {
        assert!(Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "events",
            "tsk_00000000-0000-0000-0000-000000000000",
            "--limit",
            "65",
        ])
        .is_err());
    }

    #[test]
    fn task_submit_defaults_to_brokered_core_and_fixed_task_only_target() {
        let defaults = Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "submit",
            "--idempotency-key",
            "stable-submit-key",
        ])
        .unwrap();
        let Command::Task(TaskArgs {
            command: TaskCommand::Submit(defaults),
            ..
        }) = defaults.command
        else {
            panic!("expected task submit command");
        };
        assert_eq!(defaults.runtime, "core");
        assert_eq!(
            defaults.runtime_profile,
            GATEWAY_BROKERED_CORE_RUNTIME_PROFILE
        );
        assert_eq!(task_only_target().kind.as_str(), TASK_ONLY_TARGET_KIND);
        assert_eq!(
            task_only_target().authority.as_str(),
            TASK_ONLY_TARGET_AUTHORITY
        );
        assert_eq!(
            task_only_target().identifier.as_str(),
            TASK_ONLY_TARGET_IDENTIFIER
        );

        let explicit = Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "submit",
            "--idempotency-key",
            "explicit-acp-key",
            "--runtime",
            "acp",
            "--runtime-profile",
            "codex",
        ])
        .unwrap();
        let Command::Task(TaskArgs {
            command: TaskCommand::Submit(explicit),
            ..
        }) = explicit.command
        else {
            panic!("expected explicit task submit command");
        };
        assert_eq!(explicit.runtime, "acp");
        assert_eq!(explicit.runtime_profile, "codex");
    }

    #[test]
    fn task_submit_rejects_hand_assembled_target_flags() {
        for removed in [
            vec!["--target-kind", "workspace"],
            vec!["--target-authority", "ws-ckpt"],
            vec!["--target", "checkpoint-create-v1"],
        ] {
            let parsed = Cli::try_parse_from(
                [
                    "cosh-gateway",
                    "task",
                    "submit",
                    "--idempotency-key",
                    "fixed-target-key",
                ]
                .into_iter()
                .chain(removed.iter().copied()),
            );
            assert!(
                parsed.is_err(),
                "removed target flags must not parse: {removed:?}"
            );
        }
    }

    #[test]
    fn task_approval_decision_needs_no_internal_ledger_revision() {
        let cli = Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "resolve-approval",
            "apr_00000000-0000-0000-0000-000000000000",
            "--decision",
            "approve",
            "--idempotency-key",
            "stable-approval-key",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Task(TaskArgs {
                command: TaskCommand::ResolveApproval(TaskResolveApprovalArgs {
                    decision: ApprovalChoice::Approve,
                    ..
                }),
                ..
            })
        ));
        assert!(Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "resolve-approval",
            "apr_00000000-0000-0000-0000-000000000000",
            "--decision",
            "approve",
            "--idempotency-key",
            "stable-approval-key",
            "--expected-revision",
            "1",
        ])
        .is_err());
    }

    #[test]
    fn task_append_parses_exact_input_identity_and_bounded_options() {
        let cli = Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "append",
            "tsk_00000000-0000-0000-0000-000000000000",
            "--input-request-id",
            "inp_00000000-0000-0000-0000-000000000001",
            "--select",
            "0",
            "--select",
            "2",
            "--idempotency-key",
            "stable-input-key",
            "--expected-revision",
            "5",
        ])
        .unwrap();
        let Command::Task(TaskArgs {
            command: TaskCommand::Append(append),
            ..
        }) = cli.command
        else {
            panic!("expected task append command");
        };
        assert_eq!(
            append.input_request_id,
            "inp_00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(append.selections, vec![0, 2]);
        assert_eq!(append.expected_revision, Some(5));

        assert!(Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "append",
            "tsk_00000000-0000-0000-0000-000000000000",
            "--input-request-id",
            "inp_00000000-0000-0000-0000-000000000001",
            "--input-file",
            "/tmp/input",
            "--select",
            "0",
            "--idempotency-key",
            "conflicting-input-source",
        ])
        .is_err());
    }

    #[test]
    fn task_retry_requires_exact_previous_run_and_stable_key() {
        let cli = Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "retry",
            "tsk_00000000-0000-0000-0000-000000000000",
            "--previous-run-id",
            "run_00000000-0000-0000-0000-000000000001",
            "--idempotency-key",
            "stable-retry-key",
            "--expected-revision",
            "4",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Task(TaskArgs {
                command: TaskCommand::Retry(TaskRetryArgs {
                    expected_revision: Some(4),
                    ..
                }),
                ..
            })
        ));
        assert!(Cli::try_parse_from([
            "cosh-gateway",
            "task",
            "retry",
            "tsk_00000000-0000-0000-0000-000000000000",
            "--idempotency-key",
            "stable-retry-key",
        ])
        .is_err());
    }
}
