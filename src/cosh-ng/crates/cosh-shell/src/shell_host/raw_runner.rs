use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::libc;

use crate::input::InputClassifier;
use crate::raw_input::{
    spawn_raw_action_relay, spawn_raw_input_relay, MainPromptGate, RawInputEvent, RawInputMode,
    RawObserverAction, RawRelayAction, UserPtyInputGeneration,
};
use crate::types::ShellEvent;

use super::bootstrap::{start_bash_session, start_zsh_session, PtySession};
use super::io_loop::{read_until_streaming, wait_child_preserving_signal};
use super::lifecycle::{build_shell_host_output, push_shell_exited_event};
use super::model::{ShellHostConfig, ShellHostOutput};
use super::raw_relay::{read_raw_until_exit, DriverCompletion, RawActionWatchdog};

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
        None,
        config.slash_via_shell,
        |master, _, input_events, input_classifier, input_mode, input_generation, gate, routed| {
            spawn_raw_input_relay(
                input,
                master,
                input_events,
                input_classifier,
                input_mode,
                input_generation,
                gate,
                routed,
            )
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
        None,
        false,
        |master, _, input_events, input_classifier, input_mode, input_generation, gate, routed| {
            spawn_raw_input_relay(
                input,
                master,
                input_events,
                input_classifier,
                input_mode,
                input_generation,
                gate,
                routed,
            )
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
        Some(config.raw_action_watchdog),
        false,
        |master,
         child_pid,
         input_events,
         input_classifier,
         input_mode,
         input_generation,
         gate,
         routed| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
                input_generation,
                gate,
                routed,
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
        Some(config.raw_action_watchdog),
        config.slash_via_shell,
        |master,
         child_pid,
         input_events,
         input_classifier,
         input_mode,
         input_generation,
         gate,
         routed| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
                input_generation,
                gate,
                routed,
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
        Some(config.raw_action_watchdog),
        config.slash_via_shell,
        |master,
         child_pid,
         input_events,
         input_classifier,
         input_mode,
         input_generation,
         gate,
         routed| {
            spawn_raw_action_relay(
                actions,
                master,
                child_pid,
                input_events,
                input_classifier,
                input_mode,
                input_generation,
                gate,
                routed,
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
    action_watchdog: Option<Duration>,
    slash_via_shell: bool,
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
        UserPtyInputGeneration,
        MainPromptGate,
        bool,
    ) -> JoinHandle<io::Result<()>>,
{
    let mut session = start_session(config)?;

    read_until_streaming(
        &mut session.master,
        &mut session.child,
        &mut session.parser,
        &mut output,
        Duration::from_secs(5),
        |parser| {
            if config.native_mode {
                parser.precmd_count() >= 1
            } else {
                parser.prompt_count(config.prompt.as_bytes()) >= 1
            }
        },
    )?;

    let input_master = session.master.try_clone()?;
    let (input_event_sender, input_event_receiver) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let input_generation = UserPtyInputGeneration::default();
    // #1721 D16: prompt_ready raises the gate on the output side; submits
    // and preexec lower it, keeping CJK drafts off PS2/heredoc continuations.
    let main_prompt_gate = MainPromptGate::default();
    session
        .parser
        .set_main_prompt_gate(main_prompt_gate.clone());
    // Slash-via-shell routing (issue #1718) needs a markered native session
    // so the prompt gate can prove bash is at its prompt; everything else
    // keeps the Rust intercept path.
    let slash_route_enabled = slash_via_shell && config.native_mode;
    let driver_thread = spawn_driver(
        input_master,
        session.child.id(),
        input_event_sender,
        input_classifier,
        Arc::clone(&input_mode),
        input_generation.clone(),
        main_prompt_gate,
        slash_route_enabled,
    );
    let (driver_completion_sender, driver_completion_receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = driver_thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("raw input driver panicked")));
        let _ = driver_completion_sender.send(DriverCompletion {
            result,
            completed_at: Instant::now(),
        });
    });
    let watchdog = action_watchdog.map(RawActionWatchdog::new);
    let mut last_winsize = config.winsize;
    let relay_prompt = if config.native_mode {
        ""
    } else {
        &config.prompt
    };
    let eof_shutdown = read_raw_until_exit(
        &mut session.master,
        &session.terminal,
        &mut session.child,
        &mut session.parser,
        &mut output,
        &mut event_observer,
        &input_event_receiver,
        &driver_completion_receiver,
        &input_mode,
        &input_generation,
        &mut last_winsize,
        relay_prompt,
        &session.recovery_request_file,
        &session.handoff_request_file,
        watchdog.as_ref(),
    )?;
    let display_start = session.parser.display.len();
    session.parser.flush_pending();
    output.write_all(&session.parser.display[display_start..])?;
    output.flush()?;

    let exit_status = wait_child_preserving_signal(&mut session.child, eof_shutdown)?;
    push_shell_exited_event(&mut session.parser, config, exit_status)?;
    event_observer(&session.parser.events, &mut output)?;
    output.flush()?;
    build_shell_host_output(config, session.parser, exit_status)
}

pub fn run_raw_interactive_bash(config: &ShellHostConfig) -> io::Result<ShellHostOutput> {
    let _raw_mode = RawModeGuard::activate_stdin()?;
    reopen_stdout_blocking()?;
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
    reopen_stdout_blocking()?;
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
    reopen_stdout_blocking()?;
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
    reopen_stdout_blocking()?;
    run_raw_relay_zsh_with_output_control(
        config,
        std::io::stdin(),
        std::io::stdout(),
        event_observer,
    )
}

/// Re-open stdout on a fresh, blocking file description when needed.
///
/// On Linux terminals (and SSH sessions), stdin and stdout often share the
/// same underlying open file description. `RawModeGuard` sets `O_NONBLOCK` on
/// stdin (fd 0), which therefore also makes stdout (fd 1) non-blocking.
/// Subsequent writes to stdout can then return `EAGAIN` / `EWOULDBLOCK`.
///
/// This function opens `/dev/tty` to obtain a new, independent file
/// description that is not marked `O_NONBLOCK`, and uses `dup2` to replace
/// fd 1 with it. The termios configuration is per-device, so the new fd
/// inherits the raw-mode settings already applied by `RawModeGuard`.
///
/// If stdout is not a terminal, or if `/dev/tty` cannot be opened, the
/// function logs a warning and returns success so that the shell can still
/// start. In that case the caller retains the original (possibly
/// non-blocking) stdout behavior.
fn reopen_stdout_blocking() -> io::Result<()> {
    if unsafe { libc::isatty(libc::STDOUT_FILENO) } == 0 {
        // stdout is not a terminal, so the stdin/stdout shared file
        // description problem does not apply here.
        return Ok(());
    }
    let tty = match OpenOptions::new().write(true).open("/dev/tty") {
        Ok(tty) => tty,
        Err(err) => {
            eprintln!(
                "cosh-shell: warning: cannot reopen stdout as blocking: {err}; \
                 EAGAIN risk remains"
            );
            return Ok(());
        }
    };
    let tty_fd = tty.as_raw_fd();
    if unsafe { libc::dup2(tty_fd, libc::STDOUT_FILENO) } < 0 {
        let err = io::Error::last_os_error();
        eprintln!(
            "cosh-shell: warning: cannot reopen stdout as blocking: {err}; \
             EAGAIN risk remains"
        );
    }
    // `tty` is dropped here, but the duplicated fd remains alive because fd 1
    // now refers to the same open file description.
    Ok(())
}

struct RawModeGuard {
    fd: i32,
    original_termios: Option<libc::termios>,
    original_flags: i32,
    active: bool,
}

impl RawModeGuard {
    /// #1932 F4: modifyOtherKeys level 1 makes the terminal report
    /// modifier-carrying editing keys (Shift+Enter -> `CSI 27;2;13~`)
    /// that already sit on the soft-newline whitelist, with zero terminal
    /// configuration. Level 1 leaves every conventionally-encoded key
    /// (Esc, Alt+letter, Ctrl+letter) untouched, and terminals without
    /// the feature ignore the sequence entirely. The enable is written on
    /// the relay's ordered stdout path; this guard only owns the
    /// withdrawal so the tty never keeps the mode after exit.
    const MODIFY_OTHER_KEYS_DISABLE: &'static [u8] = b"\x1b[>4;0m";

    fn activate_stdin() -> io::Result<Option<Self>> {
        Self::activate_fd(0)
    }

    fn activate_fd(fd: i32) -> io::Result<Option<Self>> {
        let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if original_flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let original_termios = if unsafe { libc::isatty(fd) } == 1 {
            let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
            if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
                let error = io::Error::last_os_error();
                unsafe {
                    libc::fcntl(fd, libc::F_SETFL, original_flags);
                }
                return Err(error);
            }

            let mut raw = original;
            unsafe {
                libc::cfmakeraw(&mut raw);
            }
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0 {
                let error = io::Error::last_os_error();
                unsafe {
                    libc::fcntl(fd, libc::F_SETFL, original_flags);
                }
                return Err(error);
            }
            Some(original)
        } else {
            None
        };

        Ok(Some(Self {
            fd,
            original_termios,
            original_flags,
            active: true,
        }))
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            if let Some(original) = &self.original_termios {
                // Withdraw the keyboard negotiation before handing the tty
                // back (#1932 F4); paired with the enable in activate_fd.
                unsafe {
                    libc::write(
                        self.fd,
                        Self::MODIFY_OTHER_KEYS_DISABLE.as_ptr().cast(),
                        Self::MODIFY_OTHER_KEYS_DISABLE.len(),
                    );
                    libc::tcsetattr(self.fd, libc::TCSANOW, original);
                }
            }
            unsafe {
                libc::fcntl(self.fd, libc::F_SETFL, self.original_flags);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn raw_mode_guard_restores_echo_and_canonical_mode() {
        let pty = nix::pty::openpty(None, None).expect("open pty");
        let fd = pty.slave.as_raw_fd();
        let original = termios_for_fd(fd);

        {
            let _guard = RawModeGuard::activate_fd(fd)
                .expect("activate raw mode")
                .expect("pty is tty");
            let raw = termios_for_fd(fd);
            assert_eq!(raw.c_lflag & libc::ECHO, 0);
            assert_eq!(raw.c_lflag & libc::ICANON, 0);
        }

        let restored = termios_for_fd(fd);
        assert_eq!(restored.c_lflag & libc::ECHO, original.c_lflag & libc::ECHO);
        assert_eq!(
            restored.c_lflag & libc::ICANON,
            original.c_lflag & libc::ICANON
        );
    }

    #[test]
    fn raw_mode_guard_restores_nonblocking_flag_for_pipe_input() {
        let pipe = nix::unistd::pipe().expect("open pipe");
        let fd = pipe.0.as_raw_fd();
        let original = unsafe { libc::fcntl(fd, libc::F_GETFL) };

        {
            let _guard = RawModeGuard::activate_fd(fd)
                .expect("activate input mode")
                .expect("pipe guard");
            let active = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert_ne!(active & libc::O_NONBLOCK, 0);
        }

        let restored = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(restored & libc::O_NONBLOCK, original & libc::O_NONBLOCK);
    }

    fn termios_for_fd(fd: i32) -> libc::termios {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut termios) }, 0);
        termios
    }
}
