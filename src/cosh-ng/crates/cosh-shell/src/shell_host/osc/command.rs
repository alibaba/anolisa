// Owner: shell_host. CurrentCommand state and the prompt display-window
// helpers, carved out of osc.rs (#2196 review: the file had crossed the
// 1000-line growth bar); `OscParser` keeps thin delegation over this data.
use super::OscParser;
use crate::types::{CommandOrigin, ShellCommandAuditIdentity};

/// A foreground command tracked between its preexec and precmd markers.
#[derive(Debug)]
pub(super) struct CurrentCommand {
    pub(super) id: String,
    pub(super) command: String,
    pub(super) cwd: String,
    pub(super) origin: CommandOrigin,
    pub(super) audit_identity: Option<ShellCommandAuditIdentity>,
    pub(super) started_at_ms: u64,
    pub(super) output_start: usize,
    pub(super) attempt_generation: Option<u64>,
    pub(super) shell_environment_generation: Option<u64>,
}

impl OscParser {
    /// #2025: origin of the command currently tracked between preexec and
    /// precmd, used by the interactive sentinel's trigger gate.
    pub(crate) fn active_command_origin(&self) -> Option<CommandOrigin> {
        self.current.as_ref().map(|current| current.origin)
    }

    /// True while a marker-tracked foreground command is running (between
    /// its preexec and precmd markers).
    pub(crate) fn has_active_foreground_command(&self) -> bool {
        self.current.is_some()
    }

    pub(crate) fn last_prompt_display(&self) -> &[u8] {
        let Some(start) = self.last_prompt_display_start else {
            return &[];
        };
        if start >= self.display.len() {
            return &[];
        }
        &self.display[start..]
    }

    /// True after the shell's post-hook marker is followed by visible prompt
    /// bytes, excluding output produced by user prompt hooks.
    pub(crate) fn has_prompt_painted_since_ready(&self) -> bool {
        self.prompt_ready_display_start
            .is_some_and(|start| start < self.display.len())
    }
}
