use std::path::PathBuf;

use nix::libc;
use nix::pty::Winsize;

use crate::input::InputClassifier;
use crate::types::ShellEvent;

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
        }
    }
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
