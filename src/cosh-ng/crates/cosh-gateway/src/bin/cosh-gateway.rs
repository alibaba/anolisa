#![forbid(unsafe_code)]
//! Installed local entrypoint for the narrow ACP v1 runtime profile.

use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
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
            Self::Runtime(_) | Self::Signal(_) => EXIT_RUNTIME,
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
            "permission_rejected" => eprintln!("ACP permission request rejected by default"),
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
    let driver = launch_driver(&args)?;
    initialize_session(&driver, reporter, &interrupted)?;
    driver
        .shutdown()
        .map_err(|error| CliError::Runtime(error.to_string()))?;
    wait_for_terminal(&driver, reporter, &interrupted)?;
    reporter.event("doctor_ok", json!({"profile": profile_name(args.profile)}))?;
    Ok(0)
}

fn run(args: RunArgs, reporter: &Reporter) -> Result<u8, CliError> {
    let prompt = read_prompt(args.prompt_file.as_ref())?;
    let interrupted = install_interrupt_handler()?;
    let driver = launch_driver(&args.profile)?;
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
                if let Some(exit) = handle_observation(&driver, reporter, observation)? {
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

fn launch_driver(args: &ProfileArgs) -> Result<AcpSessionDriver, CliError> {
    let request = AcpRuntimeProfileRequest::from_current_environment(
        args.profile.into(),
        args.adapter.clone(),
        &args.workspace,
    );
    let resolved = AcpRuntimeProfileResolver::resolve(request)
        .map_err(|error| CliError::Profile(error.to_string()))?;
    let config = AcpSessionDriverConfig::new(
        resolved.launch_spec(),
        AcpV1ClientConfig::new(
            "cosh-gateway",
            env!("CARGO_PKG_VERSION"),
            MAX_ACP_FRAME_BYTES,
        ),
        resolved.workspace(),
    );
    AcpSessionDriver::launch(config).map_err(|error| CliError::Runtime(error.to_string()))
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
                handle_observation(driver, reporter, observation)?;
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
            let options = request
                .options
                .iter()
                .filter_map(|option| match option.kind {
                    AcpV1PermissionOptionKind::AllowOnce => Some("allow_once"),
                    AcpV1PermissionOptionKind::RejectOnce => Some("reject_once"),
                    _ => None,
                })
                .collect::<Vec<_>>();
            reporter.event(
                "permission_rejected",
                json!({"request_id":request.request_id.to_string(), "options":options}),
            )?;
            driver
                .answer_permission(request.request_id, AcpV1PermissionDecision::Cancelled)
                .map_err(|error| CliError::Runtime(error.to_string()))?;
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
                handle_observation(driver, reporter, observation)?;
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
