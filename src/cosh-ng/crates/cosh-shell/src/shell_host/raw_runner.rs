use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use nix::libc;

use crate::input::InputClassifier;
use crate::raw_input::{
    spawn_raw_action_relay, spawn_raw_input_relay, RawInputEvent, RawInputMode, RawObserverAction,
    RawRelayAction,
};
use crate::types::ShellEvent;

use super::bootstrap::{start_bash_session, start_zsh_session, PtySession};
use super::io_loop::{read_until_streaming, wait_child};
use super::lifecycle::{build_shell_host_output, push_shell_exited_event};
use super::model::{ShellHostConfig, ShellHostOutput};
use super::raw_relay::read_raw_until_exit;

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
    let _input_thread = spawn_driver(
        input_master,
        session.child.id(),
        input_event_sender,
        input_classifier,
        Arc::clone(&input_mode),
    );
    let mut last_winsize = config.winsize;
    let relay_prompt = if config.native_mode {
        ""
    } else {
        &config.prompt
    };
    read_raw_until_exit(
        &mut session.master,
        &session.terminal,
        &mut session.child,
        &mut session.parser,
        &mut output,
        &mut event_observer,
        &input_event_receiver,
        &input_mode,
        &mut last_winsize,
        relay_prompt,
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

struct RawModeGuard {
    fd: i32,
    original: libc::termios,
    active: bool,
}

impl RawModeGuard {
    fn activate_stdin() -> io::Result<Option<Self>> {
        Self::activate_fd(0)
    }

    fn activate_fd(fd: i32) -> io::Result<Option<Self>> {
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

    fn termios_for_fd(fd: i32) -> libc::termios {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut termios) }, 0);
        termios
    }
}
