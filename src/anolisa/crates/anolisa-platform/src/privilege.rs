//! Privilege escalation helpers (sudo/polkit).

/// Check if the current process has root privileges.
pub fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

/// Escalate privileges via sudo for the given command.
pub fn escalate_command(cmd: &str, args: &[&str]) -> std::process::Command {
    if is_root() {
        let mut command = std::process::Command::new(cmd);
        command.args(args);
        command
    } else {
        let mut command = std::process::Command::new("sudo");
        command.arg(cmd).args(args);
        command
    }
}
