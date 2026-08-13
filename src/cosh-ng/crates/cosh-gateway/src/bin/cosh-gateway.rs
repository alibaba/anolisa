#![forbid(unsafe_code)]
//! Installed local entrypoint for the narrow ACP v1 runtime profile.

use std::fs::File;
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand, ValueEnum};
use cosh_gateway::permission::{
    CancelPermissionPresenter, FilePermissionEvidenceSink, OncePermissionProxy,
    PermissionEvidenceContext, PermissionPresenter, TextPermissionPresenter,
};
use cosh_gateway::runtime::{
    AcpRuntimeProfileId, AcpRuntimeProfileRequest, AcpRuntimeProfileResolver, AcpSessionDriver,
    AcpSessionDriverConfig, AcpSessionEvent, AcpSessionTerminalKind, AcpV1ClientConfig,
    AcpV1Observation, AcpV1PermissionDecision, AcpV1PermissionOptionKind, AcpV1StopReason,
};
use serde_json::{json, Value};
use thiserror::Error;

const MAX_ACP_FRAME_BYTES: usize = 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EVENT_DEADLINE: Duration = Duration::from_secs(15);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

const EXIT_INPUT: u8 = 10;
const EXIT_PROFILE: u8 = 11;
const EXIT_RUNTIME: u8 = 12;
const EXIT_AGENT: u8 = 13;
const EXIT_CANCELLED: u8 = 130;

#[derive(Debug, Parser)]
#[command(
    name = "cosh-gateway",
    version,
    about = "Run installed ACP v1 Agent adapters through COSH"
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

#[derive(Debug, Error)]
enum CliError {
    #[error("failed to resolve installed ACP profile: {0}")]
    Profile(String),
    #[error("failed to read prompt: {0}")]
    Input(#[source] io::Error),
    #[error("prompt path is not a regular file: {0}")]
    PromptNotRegular(PathBuf),
    #[error("prompt is empty")]
    EmptyPrompt,
    #[error("prompt exceeds the {MAX_PROMPT_BYTES}-byte limit")]
    PromptTooLarge,
    #[error("failed to register interrupt handling: {0}")]
    Signal(#[source] io::Error),
    #[error("local permission handling failed: {0}")]
    Permission(String),
    #[error("ACP runtime failed: {0}")]
    Runtime(String),
    #[error("ACP Agent rejected or did not complete the prompt")]
    Agent,
    #[error("ACP operation was cancelled")]
    Cancelled,
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Input(_)
            | Self::PromptNotRegular(_)
            | Self::EmptyPrompt
            | Self::PromptTooLarge => EXIT_INPUT,
            Self::Profile(_) => EXIT_PROFILE,
            Self::Runtime(_) | Self::Signal(_) | Self::Permission(_) => EXIT_RUNTIME,
            Self::Agent => EXIT_AGENT,
            Self::Cancelled => EXIT_CANCELLED,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Input(_) => "prompt_read_failed",
            Self::PromptNotRegular(_) => "prompt_not_regular",
            Self::EmptyPrompt => "prompt_empty",
            Self::PromptTooLarge => "prompt_too_large",
            Self::Profile(_) => "profile_invalid",
            Self::Signal(_) => "signal_handler_failed",
            Self::Permission(_) => "permission_failed",
            Self::Runtime(_) => "runtime_failed",
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = match &cli.command {
        Command::Doctor(args) => args.output,
        Command::Run(args) => args.profile.output,
    };
    let reporter = Reporter { output };
    let result = match cli.command {
        Command::Doctor(args) => doctor(args, &reporter),
        Command::Run(args) => run(args, &reporter),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            reporter.error(&error);
            ExitCode::from(error.exit_code())
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
                let matched = expected(&observation);
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
    observation: AcpV1Observation,
    permissions: Option<&mut LocalPermissionHandler>,
) -> Result<Option<u8>, CliError> {
    match observation {
        AcpV1Observation::Initialized { agent_info, .. } => {
            reporter.event(
                "initialized",
                json!({"agent": agent_info.map(|info| json!({
                    "name": info.name, "version": info.version
                }))}),
            )?;
        }
        AcpV1Observation::SessionOpened { session_id } => {
            reporter.event("session_opened", json!({"session_id":session_id}))?;
        }
        AcpV1Observation::SessionUpdate { session_id, update } => {
            let text = update
                .get("content")
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str);
            if let Some(text) = text {
                reporter.event(
                    "session_update",
                    json!({"session_id":session_id, "text":text}),
                )?;
            } else {
                reporter.event(
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
            reporter.event("permission_decided", json!({"decision":decision_name}))?;
        }
        AcpV1Observation::PromptFinished {
            session_id,
            stop_reason,
        } => {
            reporter.event(
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
            reporter.event(
                "request_failed",
                json!({"request":format!("{request:?}"), "code":code, "message":message}),
            )?;
            return Err(CliError::Agent);
        }
        AcpV1Observation::UnsupportedClientRequest { request_id, method } => {
            reporter.event(
                "unsupported_request",
                json!({"request_id":request_id.to_string(), "method":method}),
            )?;
        }
        AcpV1Observation::UnsupportedNotification { method } => {
            reporter.event("unsupported_notification", json!({"method":method}))?;
        }
        AcpV1Observation::TransportClosed => {
            return Err(CliError::Runtime("ACP transport closed".into()));
        }
    }
    Ok(None)
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
    fn cli_does_not_accept_prompt_as_an_argument() {
        assert!(Cli::try_parse_from(["cosh-gateway", "run", "secret prompt"]).is_err());
    }
}
