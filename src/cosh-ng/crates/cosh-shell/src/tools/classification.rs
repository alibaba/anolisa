#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderToolClass {
    Shell,
    ReadOnlyBuiltin,
    WriteBuiltin,
    OtherKnown,
    Unknown,
}

pub fn provider_tool_class(name: &str) -> ProviderToolClass {
    match name {
        "Bash"
        | "shell"
        | "run_shell_command"
        | "tool Bash"
        | "tool shell"
        | "tool run_shell_command" => ProviderToolClass::Shell,
        "Read"
        | "Grep"
        | "Glob"
        | "LS"
        | "read_file"
        | "grep_search"
        | "glob"
        | "list_directory"
        | "read_many_files"
        | "tool Read"
        | "tool Grep"
        | "tool Glob"
        | "tool LS"
        | "tool read_file"
        | "tool grep_search"
        | "tool glob"
        | "tool list_directory"
        | "tool read_many_files" => ProviderToolClass::ReadOnlyBuiltin,
        "Write" | "Edit" | "write_file" | "tool Write" | "tool Edit" | "tool write_file" => {
            ProviderToolClass::WriteBuiltin
        }
        "LSP" | "WebFetch" | "WebSearch" | "tool LSP" | "tool WebFetch" | "tool WebSearch" => {
            ProviderToolClass::OtherKnown
        }
        _ => ProviderToolClass::Unknown,
    }
}

pub fn is_shell_tool_name(name: &str) -> bool {
    provider_tool_class(name) == ProviderToolClass::Shell
}

pub fn is_readonly_builtin_tool_name(name: &str) -> bool {
    provider_tool_class(name) == ProviderToolClass::ReadOnlyBuiltin
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInteractionProfile {
    pub pty_requirement: PtyRequirement,
    pub output_stability: OutputStability,
    pub approval_risk: ApprovalRisk,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyRequirement {
    NotRequired,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStability {
    StableSnapshot,
    UnstableInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRisk {
    Medium,
    High,
}

impl ApprovalRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

pub fn classify_command_interaction(command: &str) -> CommandInteractionProfile {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let Some(program_index) = tokens.iter().position(|token| !is_env_assignment(token)) else {
        return stable_medium("empty command");
    };
    let program = basename(tokens[program_index]);
    let args = &tokens[program_index + 1..];
    let destructive = command_has_high_risk_shell_syntax(command)
        || command_contains_destructive_token(&tokens[program_index..]);

    if matches!(program, "sudo" | "su" | "passwd") {
        return interactive_profile(
            ApprovalRisk::High,
            "command commonly requires privilege or credential interaction",
        );
    }
    if matches!(program, "vim" | "vi" | "nvim" | "nano" | "emacs") {
        return interactive_profile(ApprovalRisk::High, "command opens an interactive editor");
    }
    if matches!(program, "ssh" | "scp" | "sftp") {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "command commonly requires an interactive terminal",
        );
    }
    if matches!(program, "less" | "more" | "man" | "htop") {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "command opens an interactive terminal UI",
        );
    }
    if program == "top" && !top_is_batch_snapshot(args) {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "command opens an interactive terminal UI",
        );
    }
    if matches!(program, "bash" | "sh" | "zsh" | "fish") && args.is_empty() {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "interactive shell without command",
        );
    }
    if matches!(program, "python" | "python3" | "node" | "irb" | "ruby") && !has_eval_arg(args) {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "language runtime without non-interactive command argument",
        );
    }
    if matches!(program, "docker" | "podman" | "kubectl") && has_tty_arg(args) {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "command explicitly requests an interactive tty",
        );
    }
    if tokens
        .iter()
        .any(|token| matches!(*token, "read" | "stty" | "$EDITOR" | "$VISUAL"))
    {
        return interactive_profile(
            if destructive {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "command contains an interactive terminal primitive",
        );
    }

    if destructive {
        CommandInteractionProfile {
            pty_requirement: PtyRequirement::NotRequired,
            output_stability: OutputStability::StableSnapshot,
            approval_risk: ApprovalRisk::High,
            reason: "command contains destructive or shell-sensitive syntax",
        }
    } else {
        stable_medium("stable non-interactive command")
    }
}

pub fn obvious_tty_command_reason(command: &str) -> Option<&'static str> {
    let profile = classify_command_interaction(command);
    (profile.pty_requirement != PtyRequirement::NotRequired).then_some(profile.reason)
}

fn basename(program: &str) -> &str {
    program
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(program)
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !name.bytes().next().unwrap_or_default().is_ascii_digit()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn has_eval_arg(args: &[&str]) -> bool {
    args.iter()
        .any(|arg| matches!(*arg, "-c" | "-e" | "--eval" | "--command"))
}

fn has_tty_arg(args: &[&str]) -> bool {
    args.iter().any(|arg| {
        matches!(
            *arg,
            "-it" | "-ti" | "-i" | "-t" | "--interactive" | "--tty"
        ) || arg.starts_with("--interactive=")
            || arg.starts_with("--tty=")
    })
}

fn top_is_batch_snapshot(args: &[&str]) -> bool {
    args.contains(&"-b") || args.contains(&"-l")
}

fn command_has_high_risk_shell_syntax(command: &str) -> bool {
    command
        .chars()
        .any(|ch| matches!(ch, ';' | '|' | '&' | '>' | '<' | '$' | '`'))
}

fn command_contains_destructive_token(tokens: &[&str]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        let token = basename(token);
        matches!(
            token,
            "rm" | "rmdir"
                | "mv"
                | "dd"
                | "kill"
                | "killall"
                | "pkill"
                | "chmod"
                | "chown"
                | "brew"
                | "apt"
                | "apt-get"
                | "dnf"
                | "yum"
                | "systemctl"
                | "launchctl"
        ) || (index > 0 && matches!(token, "delete" | "remove" | "uninstall"))
    })
}

fn interactive_profile(
    approval_risk: ApprovalRisk,
    reason: &'static str,
) -> CommandInteractionProfile {
    CommandInteractionProfile {
        pty_requirement: PtyRequirement::Required,
        output_stability: OutputStability::UnstableInteractive,
        approval_risk,
        reason,
    }
}

fn stable_medium(reason: &'static str) -> CommandInteractionProfile {
    CommandInteractionProfile {
        pty_requirement: PtyRequirement::NotRequired,
        output_stability: OutputStability::StableSnapshot,
        approval_risk: ApprovalRisk::Medium,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_shell_provider_aliases() {
        for name in [
            "Bash",
            "shell",
            "run_shell_command",
            "tool Bash",
            "tool shell",
            "tool run_shell_command",
        ] {
            assert_eq!(
                provider_tool_class(name),
                ProviderToolClass::Shell,
                "{name}"
            );
            assert!(is_shell_tool_name(name), "{name}");
        }
        assert!(!is_shell_tool_name("Read"));
    }

    #[test]
    fn classifies_readonly_provider_aliases() {
        for name in [
            "Read",
            "Grep",
            "Glob",
            "LS",
            "read_file",
            "grep_search",
            "glob",
            "list_directory",
            "read_many_files",
            "tool Read",
            "tool Grep",
            "tool Glob",
            "tool LS",
            "tool read_file",
            "tool grep_search",
            "tool glob",
            "tool list_directory",
            "tool read_many_files",
        ] {
            assert_eq!(
                provider_tool_class(name),
                ProviderToolClass::ReadOnlyBuiltin,
                "{name}"
            );
            assert!(is_readonly_builtin_tool_name(name), "{name}");
        }
        assert!(!is_readonly_builtin_tool_name("Bash"));
    }

    #[test]
    fn classifies_write_and_unknown_tools_without_shell_execution() {
        for name in ["Write", "Edit", "write_file", "tool Write", "tool Edit"] {
            assert_eq!(
                provider_tool_class(name),
                ProviderToolClass::WriteBuiltin,
                "{name}"
            );
            assert!(!is_shell_tool_name(name), "{name}");
        }
        assert_eq!(
            provider_tool_class("CustomTool"),
            ProviderToolClass::Unknown
        );
        assert!(!is_shell_tool_name("CustomTool"));
    }

    #[test]
    fn detects_obvious_tty_command_risk_conservatively() {
        for command in [
            "sudo id",
            "/usr/bin/ssh host",
            "vim Cargo.toml",
            "less README.md",
            "python",
            "docker exec -it container sh",
            "kubectl exec --tty pod -- sh",
            "LANG=C sudo id",
        ] {
            assert!(obvious_tty_command_reason(command).is_some(), "{command}");
        }

        for command in [
            "df -h",
            "git status --short",
            "python -c 'print(1)'",
            "node -e 'console.log(1)'",
            "docker ps",
            "kubectl get pods",
            "top -b -n1",
            "top -l 1 -stats pid,mem,command",
        ] {
            assert!(obvious_tty_command_reason(command).is_none(), "{command}");
        }
    }

    #[test]
    fn command_interaction_profile_decouples_pty_from_approval_risk() {
        for command in [
            "less README.md",
            "man ls",
            "top",
            "python",
            "node",
            "ssh host",
            "docker exec -it container sh",
            "kubectl exec --tty pod -- sh",
        ] {
            let profile = classify_command_interaction(command);
            assert_eq!(
                profile.pty_requirement,
                PtyRequirement::Required,
                "{command}"
            );
            assert_eq!(
                profile.output_stability,
                OutputStability::UnstableInteractive,
                "{command}"
            );
            assert_eq!(profile.approval_risk, ApprovalRisk::Medium, "{command}");
        }

        for command in ["vim Cargo.toml", "sudo id", "rm -rf target", "kill 1234"] {
            assert_eq!(
                classify_command_interaction(command).approval_risk,
                ApprovalRisk::High,
                "{command}"
            );
        }

        for command in ["df -h", "top -b -n1", "top -l 1 -stats pid,mem,command"] {
            let profile = classify_command_interaction(command);
            assert_eq!(
                profile.pty_requirement,
                PtyRequirement::NotRequired,
                "{command}"
            );
            assert_eq!(
                profile.output_stability,
                OutputStability::StableSnapshot,
                "{command}"
            );
            assert_eq!(profile.approval_risk, ApprovalRisk::Medium, "{command}");
        }
    }
}
