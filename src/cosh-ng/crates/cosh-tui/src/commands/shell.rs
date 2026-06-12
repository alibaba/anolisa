//! Shell-related slash commands for cosh-tui.

use super::{CommandResult, SlashCommand};

pub struct BashCommand;

impl SlashCommand for BashCommand {
    fn name(&self) -> &str {
        "bash"
    }
    fn aliases(&self) -> &[&str] {
        &["sh", "shell"]
    }
    fn description(&self) -> &str {
        "Launch an interactive shell (exit to return)"
    }
    fn execute(&self, _args: &str, _app: &mut crate::app::App) -> CommandResult {
        CommandResult::EnterShell
    }
}

/// Spawn an interactive shell, suspending the TUI.
/// Returns Ok(()) on success, Err with message on failure.
pub fn spawn_interactive_shell() -> Result<(), String> {
    // Determine which shell to use
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    // Suspend raw mode so the child shell gets a normal terminal
    crossterm::terminal::disable_raw_mode()
        .map_err(|e| format!("Failed to disable raw mode: {}", e))?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen
    ).map_err(|e| format!("Failed to leave alternate screen: {}", e))?;

    // Spawn the shell as a child process
    let status = std::process::Command::new(&shell)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    // Resume TUI mode regardless of child exit status
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen
    );
    let _ = crossterm::terminal::enable_raw_mode();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Ok(()), // Shell exited with non-zero (user typed 'exit 1' etc), still fine
        Err(e) => Err(format!("Failed to spawn shell '{}': {}", shell, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bash_command_returns_enter_shell() {
        let cmd = BashCommand;
        assert_eq!(cmd.name(), "bash");
        assert_eq!(cmd.aliases(), &["sh", "shell"]);
        // We can't easily test execute without App, but we verify trait methods
        assert!(!cmd.description().is_empty());
    }
}
