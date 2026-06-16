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
    let assessment = super::command_risk::assess_shell_command(
        command,
        super::command_risk::AssessmentPolicy::ask(
            super::command_risk::AssessmentSource::ProviderShellTool,
        ),
    );
    CommandInteractionProfile {
        pty_requirement: match assessment.interaction {
            super::command_risk::InteractionRequirement::None => PtyRequirement::NotRequired,
            super::command_risk::InteractionRequirement::TtyRequired
            | super::command_risk::InteractionRequirement::CredentialPromptLikely => {
                PtyRequirement::Required
            }
        },
        output_stability: match assessment.output_stability {
            super::command_risk::OutputStability::StableSnapshot
            | super::command_risk::OutputStability::PotentiallyLarge => {
                OutputStability::StableSnapshot
            }
            super::command_risk::OutputStability::Streaming
            | super::command_risk::OutputStability::UnstableInteractive => {
                OutputStability::UnstableInteractive
            }
        },
        approval_risk: match assessment.impact {
            super::command_risk::RiskImpact::High => ApprovalRisk::High,
            super::command_risk::RiskImpact::Low | super::command_risk::RiskImpact::Medium => {
                ApprovalRisk::Medium
            }
        },
        reason: assessment.primary_reason(),
    }
}

pub fn obvious_tty_command_reason(command: &str) -> Option<&'static str> {
    let profile = classify_command_interaction(command);
    (profile.pty_requirement != PtyRequirement::NotRequired).then_some(profile.reason)
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
