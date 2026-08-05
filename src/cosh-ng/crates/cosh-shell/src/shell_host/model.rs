use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nix::libc;
use nix::pty::Winsize;

use crate::input::InputClassifier;
use crate::types::{ShellEnvironmentSnapshot, ShellEvent};

use super::raw_relay::interactive_sentinel::InputWaitStatus;

#[derive(Clone)]
pub(super) struct ShellEnvironmentObserver(
    Arc<dyn Fn(ShellEnvironmentSnapshot) + Send + Sync + 'static>,
);

impl std::fmt::Debug for ShellEnvironmentObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ShellEnvironmentObserver")
    }
}

#[derive(Clone)]
pub(super) struct ShellHistoryFileObserver(Arc<dyn Fn(PathBuf) + Send + Sync + 'static>);

impl std::fmt::Debug for ShellHistoryFileObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ShellHistoryFileObserver")
    }
}

impl ShellHistoryFileObserver {
    pub(super) fn new<F>(observer: F) -> Self
    where
        F: Fn(PathBuf) + Send + Sync + 'static,
    {
        Self(Arc::new(observer))
    }

    pub(super) fn observe(&self, path: PathBuf) {
        (self.0)(path);
    }
}

impl ShellEnvironmentObserver {
    pub(super) fn new<F>(observer: F) -> Self
    where
        F: Fn(ShellEnvironmentSnapshot) + Send + Sync + 'static,
    {
        Self(Arc::new(observer))
    }

    pub(super) fn observe(&self, snapshot: ShellEnvironmentSnapshot) {
        (self.0)(snapshot);
    }
}

#[derive(Debug, Clone)]
pub struct ShellHostConfig {
    pub session_id: String,
    pub work_dir: PathBuf,
    pub bash_path: String,
    pub zsh_path: String,
    pub prompt: String,
    pub winsize: Winsize,
    pub input_classifier: InputClassifier,
    pub native_mode: bool,
    pub login_shell: bool,
    /// Routes exact slash-control submissions through bash so they enter
    /// native history (issue #1718). Defaults from `COSH_SLASH_VIA_SHELL`
    /// (on unless "0"); disabling restores the pre-#1718 Rust intercept
    /// path end to end. Only bash runners consult it; zsh has no extdebug
    /// return-suppression equivalent and always keeps the Rust path.
    pub slash_via_shell: bool,
    pub env_overrides: Vec<(String, String)>,
    pub raw_action_watchdog: Duration,
    /// #2161: shared input-wait episode clock. The relay's interactive
    /// sentinel marks/clears it; the runtime controller reads it to drive
    /// the `shell.input_wait_timeout_secs` interrupt. Clone the handle
    /// before handing the config to the runner.
    pub(crate) input_wait_status: InputWaitStatus,
    /// #2025/#2161: language for the relay-rendered input-wait hint card.
    pub(crate) hint_language: crate::config::Language,
    /// #2161: mirrors `shell.input_wait_timeout_secs` so the hint card can
    /// forecast the auto-interrupt (0 = disabled, no forecast line).
    pub(crate) input_wait_timeout_secs: u64,
    pub(super) shell_environment_observer: Option<ShellEnvironmentObserver>,
    pub(super) shell_history_file_observer: Option<ShellHistoryFileObserver>,
}

impl ShellHostConfig {
    pub fn new(session_id: impl Into<String>, work_dir: impl Into<PathBuf>) -> Self {
        let winsize = current_terminal_winsize().unwrap_or_else(default_winsize);
        Self {
            session_id: session_id.into(),
            work_dir: work_dir.into(),
            bash_path: "bash".to_string(),
            zsh_path: "zsh".to_string(),
            prompt: "cosh-osc$ ".to_string(),
            winsize,
            input_classifier: InputClassifier::default(),
            native_mode: true,
            login_shell: false,
            slash_via_shell: slash_via_shell_default(),
            env_overrides: Vec::new(),
            raw_action_watchdog: Duration::from_secs(120),
            input_wait_status: InputWaitStatus::default(),
            hint_language: crate::config::Language::default(),
            input_wait_timeout_secs: 120,
            shell_environment_observer: None,
            shell_history_file_observer: None,
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_overrides.push((key.into(), value.into()));
        self
    }

    pub fn with_ai_enabled(mut self, enabled: bool) -> Self {
        self.input_classifier = self.input_classifier.with_ai_enabled(enabled);
        self
    }

    pub(crate) fn set_shell_environment_observer<F>(&mut self, observer: F)
    where
        F: Fn(ShellEnvironmentSnapshot) + Send + Sync + 'static,
    {
        self.shell_environment_observer = Some(ShellEnvironmentObserver::new(observer));
    }

    pub(crate) fn clear_shell_environment_observer(&mut self) {
        self.shell_environment_observer = None;
    }

    pub(crate) fn set_shell_history_file_observer<F>(&mut self, observer: F)
    where
        F: Fn(PathBuf) + Send + Sync + 'static,
    {
        self.shell_history_file_observer = Some(ShellHistoryFileObserver::new(observer));
    }

    pub(crate) fn clear_shell_history_file_observer(&mut self) {
        self.shell_history_file_observer = None;
    }
}

/// `COSH_SLASH_VIA_SHELL` gates the shell routing of exact slash
/// submissions; any value other than "0" (including unset) keeps it on.
fn slash_via_shell_default() -> bool {
    std::env::var("COSH_SLASH_VIA_SHELL")
        .map(|value| value != "0")
        .unwrap_or(true)
}

fn default_winsize() -> Winsize {
    Winsize {
        ws_row: 40,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

pub(super) fn current_terminal_winsize() -> Option<Winsize> {
    [libc::STDOUT_FILENO, libc::STDIN_FILENO, libc::STDERR_FILENO]
        .into_iter()
        .filter_map(read_fd_winsize)
        .find(|winsize| winsize.ws_row > 0 && winsize.ws_col > 0)
}

fn read_fd_winsize(fd: i32) -> Option<Winsize> {
    let mut winsize = default_winsize();
    let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as libc::c_ulong, &mut winsize) };
    if result == 0 {
        Some(winsize)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptedInput {
    Command(String),
    UserLine(String),
    Intercept { input: String, reason: String },
}

impl ScriptedInput {
    pub fn command(command: impl Into<String>) -> Self {
        Self::Command(command.into())
    }

    pub fn user_line(input: impl Into<String>) -> Self {
        Self::UserLine(input.into())
    }

    pub fn intercept(input: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Intercept {
            input: input.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug)]
pub struct ShellHostOutput {
    pub events: Vec<ShellEvent>,
    pub terminal_output: Vec<u8>,
    pub work_dir: PathBuf,
    pub journal_path: PathBuf,
    pub exit_status: Option<i32>,
}
