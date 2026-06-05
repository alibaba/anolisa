use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::libc;
use nix::pty::openpty;

use crate::input::{InputClassifier, InputDecision};
use crate::journal::write_shell_events;
use crate::raw_input::{
    spawn_raw_action_relay, spawn_raw_input_relay, RawInputEvent, RawInputMode, RawObserverAction,
    RawRelayAction,
};
use crate::types::{ShellEvent, ShellEventKind};

mod adapter;
mod marker;
mod model;
mod osc;
mod raw_relay;
use adapter::{BashAdapter, ShellAdapter, ZshAdapter};
pub use model::{ScriptedInput, ShellHostConfig, ShellHostOutput};
use osc::{now_ms, OscParser};
use raw_relay::read_raw_until_exit;

struct PtySession {
    master: File,
    child: Child,
    parser: OscParser,
}

fn start_bash_session(config: &ShellHostConfig) -> io::Result<PtySession> {
    start_shell_session(config, &BashAdapter)
}

fn start_zsh_session(config: &ShellHostConfig) -> io::Result<PtySession> {
    start_shell_session(config, &ZshAdapter)
}

fn start_shell_session(
    config: &ShellHostConfig,
    adapter: &dyn ShellAdapter,
) -> io::Result<PtySession> {
    fs::create_dir_all(&config.work_dir)?;
    let output_ref_dir = config.work_dir.join("output-refs");
    fs::create_dir_all(&output_ref_dir)?;
    let rcfile = config.work_dir.join(adapter.marker_filename());
    fs::write(&rcfile, adapter.marker_script())?;

    let pty = openpty(Some(&config.winsize), None).map_err(nix_to_io)?;
    let master = unsafe { File::from_raw_fd(pty.master.into_raw_fd()) };
    set_nonblocking(master.as_raw_fd())?;

    let slave = unsafe { File::from_raw_fd(pty.slave.into_raw_fd()) };
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;

    let mut command = Command::new(adapter.executable(config));
    adapter.configure_command(&mut command, &rcfile, config);
    command
        .env("COSH_SESSION_ID", &config.session_id)
        .env("COSH_HISTFILE", config.work_dir.join("history"))
        .env("COSH_POC_PS1", &config.prompt)
        .env("BASH_SILENCE_DEPRECATION_WARNING", "1")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave));

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn()?;
    let mut parser = OscParser::new(config.session_id.clone(), output_ref_dir);
    push_shell_started_event(&mut parser, config);

    Ok(PtySession {
        master,
        child,
        parser,
    })
}

fn push_shell_started_event(parser: &mut OscParser, config: &ShellHostConfig) {
    parser.events.push(ShellEvent {
        kind: ShellEventKind::ShellStarted,
        session_id: config.session_id.clone(),
        command_id: None,
        command: None,
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string()),
        end_cwd: None,
        exit_code: None,
        started_at_ms: Some(now_ms()),
        ended_at_ms: None,
        duration_ms: None,
        terminal_output_ref: None,
        terminal_output_bytes: None,
        input: None,
        component: None,
        message: None,
    });
}

fn push_shell_exited_event(
    parser: &mut OscParser,
    config: &ShellHostConfig,
    exit_status: Option<i32>,
) -> io::Result<()> {
    parser.finish_current_on_exit(exit_status.unwrap_or(0))?;
    parser.events.push(ShellEvent {
        kind: ShellEventKind::ShellExited,
        session_id: config.session_id.clone(),
        command_id: None,
        command: None,
        cwd: None,
        end_cwd: None,
        exit_code: exit_status,
        started_at_ms: None,
        ended_at_ms: Some(now_ms()),
        duration_ms: None,
        terminal_output_ref: None,
        terminal_output_bytes: None,
        input: None,
        component: None,
        message: None,
    });
    Ok(())
}

fn finish_shell_host_output(
    config: &ShellHostConfig,
    mut parser: OscParser,
    exit_status: Option<i32>,
) -> io::Result<ShellHostOutput> {
    push_shell_exited_event(&mut parser, config, exit_status)?;
    build_shell_host_output(config, parser, exit_status)
}

fn build_shell_host_output(
    config: &ShellHostConfig,
    parser: OscParser,
    exit_status: Option<i32>,
) -> io::Result<ShellHostOutput> {
    let journal_path = config.work_dir.join("events.jsonl");
    write_shell_events(&journal_path, &parser.events)?;

    Ok(ShellHostOutput {
        events: parser.events,
        terminal_output: parser.clean,
        work_dir: config.work_dir.clone(),
        journal_path,
        exit_status,
    })
}

pub fn run_scripted_bash(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
) -> io::Result<ShellHostOutput> {
    run_scripted_shell(config, inputs, start_bash_session)
}

pub fn run_scripted_zsh(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
) -> io::Result<ShellHostOutput> {
    run_scripted_shell(config, inputs, start_zsh_session)
}

fn run_scripted_shell(
    config: &ShellHostConfig,
    inputs: &[ScriptedInput],
    start_session: fn(&ShellHostConfig) -> io::Result<PtySession>,
) -> io::Result<ShellHostOutput> {
    let mut session = start_session(config)?;

    read_until(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        Duration::from_secs(5),
        |parser| parser.prompt_count(config.prompt.as_bytes()) >= 1,
    )?;

    for input in inputs {
        match input {
            ScriptedInput::Command(command) => {
                send_command_line(
                    &mut session.master,
                    &mut session.child,
                    &mut session.parser,
                    &config.prompt,
                    command,
                )?;
            }
            ScriptedInput::UserLine(input) => match config.input_classifier.classify(input) {
                InputDecision::SendToShell(command) => {
                    send_command_line(
                        &mut session.master,
                        &mut session.child,
                        &mut session.parser,
                        &config.prompt,
                        &command,
                    )?;
                }
                InputDecision::Intercept { input, reason } => {
                    session.parser.push_intercept_event(
                        &config.session_id,
                        input,
                        None,
                        reason.as_str(),
                    );
                }
            },
            ScriptedInput::Intercept { input, reason } => {
                session.parser.push_intercept_event(
                    &config.session_id,
                    input.clone(),
                    None,
                    reason,
                );
            }
        }
    }

    session.master.write_all(b"exit\n")?;
    session.master.flush()?;
    read_until(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        Duration::from_secs(5),
        |_| false,
    )?;
    session.parser.flush_pending();
    let exit_status = wait_child(&mut session.child)?;
    finish_shell_host_output(config, session.parser, exit_status)
}

pub fn run_streaming_line_bash<R, W>(
    config: &ShellHostConfig,
    mut input: R,
    mut output: W,
) -> io::Result<ShellHostOutput>
where
    R: BufRead,
    W: Write,
{
    let mut session = start_bash_session(config)?;

    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_secs(5),
        |parser| parser.prompt_count(config.prompt.as_bytes()) >= 1,
    )?;

    let mut line = String::new();
    loop {
        line.clear();
        let bytes = input.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }

        let user_line = line.trim_end_matches(['\r', '\n']).to_string();
        if user_line.is_empty() {
            continue;
        }

        match config.input_classifier.classify(&user_line) {
            InputDecision::SendToShell(command) => send_command_line_streaming(
                &mut session.master,
                &mut session.child,
                &mut session.parser,
                &mut output,
                &config.prompt,
                &command,
            )?,
            InputDecision::Intercept { input, reason } => {
                session.parser.push_intercept_event(
                    &config.session_id,
                    input,
                    None,
                    reason.as_str(),
                );
            }
        }
    }

    session.master.write_all(b"exit\n")?;
    session.master.flush()?;
    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_secs(5),
        |_| false,
    )?;
    let display_start = session.parser.display.len();
    session.parser.flush_pending();
    output.write_all(&session.parser.display[display_start..])?;
    output.flush()?;

    let exit_status = wait_child(&mut session.child)?;
    finish_shell_host_output(config, session.parser, exit_status)
}

pub fn run_raw_relay_bash<R, W>(
    config: &ShellHostConfig,
    input: R,
    mut output: W,
) -> io::Result<ShellHostOutput>
where
    R: Read + Send + 'static,
    W: Write,
{
    run_raw_relay_bash_with_observer(config, input, &mut output, |_, _| Ok(()))
}

pub fn run_raw_relay_bash_with_observer<R, W, F>(
    config: &ShellHostConfig,
    input: R,
    output: W,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<()>,
{
    let mut event_observer = event_observer;
    run_raw_relay_bash_with_output_control(config, input, output, move |events, output| {
        event_observer(events, output)?;
        Ok(RawObserverAction::Continue)
    })
}

pub fn run_raw_relay_bash_with_output_control<R, W, F>(
    config: &ShellHostConfig,
    input: R,
    output: W,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    run_raw_relay_with_driver(
        config,
        start_bash_session,
        output,
        event_observer,
        config.input_classifier.clone(),
        |master, _, input_events, input_classifier, input_mode| {
            spawn_raw_input_relay(input, master, input_events, input_classifier, input_mode)
        },
    )
}

pub fn run_raw_relay_zsh_with_output_control<R, W, F>(
    config: &ShellHostConfig,
    input: R,
    output: W,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    R: Read + Send + 'static,
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    run_raw_relay_with_driver(
        config,
        start_zsh_session,
        output,
        event_observer,
        config.input_classifier.clone(),
        |master, _, input_events, input_classifier, input_mode| {
            spawn_raw_input_relay(input, master, input_events, input_classifier, input_mode)
        },
    )
}

pub fn run_raw_relay_bash_with_actions<W>(
    config: &ShellHostConfig,
    actions: Vec<RawRelayAction>,
    output: W,
) -> io::Result<ShellHostOutput>
where
    W: Write,
{
    run_raw_relay_bash_with_actions_observer(config, actions, output, |_, _| Ok(()))
}

pub fn run_raw_relay_zsh_with_actions<W>(
    config: &ShellHostConfig,
    actions: Vec<RawRelayAction>,
    output: W,
) -> io::Result<ShellHostOutput>
where
    W: Write,
{
    run_raw_relay_with_driver(
        config,
        start_zsh_session,
        output,
        |_, _| Ok(RawObserverAction::Continue),
        config.input_classifier.clone(),
        |master, child_pid, input_events, input_classifier, input_mode| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
            )
        },
    )
}

pub fn run_raw_relay_bash_with_actions_observer<W, F>(
    config: &ShellHostConfig,
    actions: Vec<RawRelayAction>,
    output: W,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<()>,
{
    let mut event_observer = event_observer;
    run_raw_relay_with_driver(
        config,
        start_bash_session,
        output,
        move |events, output| {
            event_observer(events, output)?;
            Ok(RawObserverAction::Continue)
        },
        config.input_classifier.clone(),
        |master, child_pid, input_events, input_classifier, input_mode| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
            )
        },
    )
}

pub fn run_raw_relay_bash_with_actions_output_control<W, F>(
    config: &ShellHostConfig,
    actions: Vec<RawRelayAction>,
    output: W,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    run_raw_relay_with_driver(
        config,
        start_bash_session,
        output,
        event_observer,
        config.input_classifier.clone(),
        |master, child_pid, input_events, input_classifier, input_mode| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
            )
        },
    )
}

fn run_raw_relay_with_driver<W, F, D>(
    config: &ShellHostConfig,
    start_session: fn(&ShellHostConfig) -> io::Result<PtySession>,
    mut output: W,
    mut event_observer: F,
    input_classifier: InputClassifier,
    spawn_driver: D,
) -> io::Result<ShellHostOutput>
where
    W: Write,
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
    D: FnOnce(
        File,
        u32,
        Sender<RawInputEvent>,
        InputClassifier,
        Arc<Mutex<RawInputMode>>,
    ) -> JoinHandle<io::Result<()>>,
{
    let mut session = start_session(config)?;

    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_secs(5),
        |parser| parser.prompt_count(config.prompt.as_bytes()) >= 1,
    )?;

    let input_master = session.master.try_clone()?;
    let (input_event_sender, input_event_receiver) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let _input_thread = spawn_driver(
        input_master,
        session.child.id(),
        input_event_sender,
        input_classifier,
        Arc::clone(&input_mode),
    );
    let mut last_winsize = config.winsize;
    read_raw_until_exit(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        &mut event_observer,
        &input_event_receiver,
        &input_mode,
        &mut last_winsize,
        &config.prompt,
    )?;
    let display_start = session.parser.display.len();
    session.parser.flush_pending();
    output.write_all(&session.parser.display[display_start..])?;
    output.flush()?;

    let exit_status = wait_child(&mut session.child)?;
    push_shell_exited_event(&mut session.parser, config, exit_status)?;
    event_observer(&session.parser.events, &mut output)?;
    output.flush()?;
    build_shell_host_output(config, session.parser, exit_status)
}

pub fn run_raw_interactive_bash(config: &ShellHostConfig) -> io::Result<ShellHostOutput> {
    let _raw_mode = RawModeGuard::activate_stdin()?;
    run_raw_relay_bash(config, std::io::stdin(), std::io::stdout())
}

pub fn run_raw_interactive_bash_with_observer<F>(
    config: &ShellHostConfig,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    F: FnMut(&[ShellEvent], &mut std::io::Stdout) -> io::Result<()>,
{
    let _raw_mode = RawModeGuard::activate_stdin()?;
    run_raw_relay_bash_with_observer(config, std::io::stdin(), std::io::stdout(), event_observer)
}

pub fn run_raw_interactive_bash_with_output_control<F>(
    config: &ShellHostConfig,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    F: FnMut(&[ShellEvent], &mut std::io::Stdout) -> io::Result<RawObserverAction>,
{
    let _raw_mode = RawModeGuard::activate_stdin()?;
    run_raw_relay_bash_with_output_control(
        config,
        std::io::stdin(),
        std::io::stdout(),
        event_observer,
    )
}

pub fn run_raw_interactive_zsh_with_output_control<F>(
    config: &ShellHostConfig,
    event_observer: F,
) -> io::Result<ShellHostOutput>
where
    F: FnMut(&[ShellEvent], &mut std::io::Stdout) -> io::Result<RawObserverAction>,
{
    let _raw_mode = RawModeGuard::activate_stdin()?;
    run_raw_relay_zsh_with_output_control(
        config,
        std::io::stdin(),
        std::io::stdout(),
        event_observer,
    )
}

fn send_command_line(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    prompt: &str,
    command: &str,
) -> io::Result<()> {
    let target_prompts = parser.prompt_count(prompt.as_bytes()) + 1;
    master.write_all(command.as_bytes())?;
    master.write_all(b"\n")?;
    master.flush()?;
    read_until(master, child, parser, Duration::from_secs(5), |parser| {
        parser.prompt_count(prompt.as_bytes()) >= target_prompts
    })?;
    Ok(())
}

fn send_command_line_streaming<W: Write>(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    output: &mut W,
    prompt: &str,
    command: &str,
) -> io::Result<()> {
    let target_prompts = parser.prompt_count(prompt.as_bytes()) + 1;
    master.write_all(command.as_bytes())?;
    master.write_all(b"\n")?;
    master.flush()?;
    read_until_streaming(
        master,
        child,
        parser,
        output,
        Duration::from_secs(5),
        |parser| parser.prompt_count(prompt.as_bytes()) >= target_prompts,
    )?;
    Ok(())
}

fn read_until(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    timeout: Duration,
    condition: impl Fn(&OscParser) -> bool,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 8192];

    while Instant::now() < deadline {
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    if condition(parser) {
                        return Ok(true);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if child.try_wait()?.is_some() => return Ok(condition(parser)),
                Err(err) => return Err(err),
            }
        }

        if child.try_wait()?.is_some() {
            return Ok(condition(parser));
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(condition(parser))
}

fn read_until_streaming<W: Write>(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    output: &mut W,
    timeout: Duration,
    condition: impl Fn(&OscParser) -> bool,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 8192];
    let mut display_start = parser.display.len();

    while Instant::now() < deadline {
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    if parser.display.len() > display_start {
                        output.write_all(&parser.display[display_start..])?;
                        output.flush()?;
                        display_start = parser.display.len();
                    }
                    if condition(parser) {
                        return Ok(true);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if child.try_wait()?.is_some() => return Ok(condition(parser)),
                Err(err) => return Err(err),
            }
        }

        if child.try_wait()?.is_some() {
            return Ok(condition(parser));
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(condition(parser))
}

struct RawModeGuard {
    fd: i32,
    original: libc::termios,
    active: bool,
}

impl RawModeGuard {
    fn activate_stdin() -> io::Result<Option<Self>> {
        let fd = 0;
        if unsafe { libc::isatty(fd) } != 1 {
            return Ok(None);
        }

        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Some(Self {
            fd,
            original,
            active: true,
        }))
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
            }
        }
    }
}

fn wait_child(child: &mut Child) -> io::Result<Option<i32>> {
    match child.try_wait()? {
        Some(status) => Ok(status.code()),
        None => Ok(child.wait()?.code()),
    }
}

fn set_nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn nix_to_io(err: nix::Error) -> io::Error {
    io::Error::other(err)
}
