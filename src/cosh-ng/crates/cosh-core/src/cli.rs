use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cosh-core",
    about = "cosh core — agent core + interactive terminal"
)]
pub struct CliArgs {
    /// Force headless JSONL mode (otherwise auto-detected via TTY)
    #[arg(long)]
    pub headless: bool,

    /// Override the active model from config.toml
    #[arg(long)]
    pub model: Option<String>,

    /// Override approval mode (trust|auto|balanced|strict)
    #[arg(long, value_name = "MODE")]
    pub approval_mode: Option<String>,

    /// Comma-separated list of auto-approved tools
    #[arg(long, value_name = "TOOLS")]
    pub allowed_tools: Option<String>,

    /// Comma-separated tools exposed to the model (default|empty|names)
    #[arg(long, value_name = "TOOLS")]
    pub tools: Option<String>,

    /// Disable project config, hooks, skills, and extensions
    #[arg(long)]
    pub bare: bool,

    /// Resume an existing session
    #[arg(long, value_name = "SESSION_ID")]
    pub resume: Option<String>,

    /// Override the workspace scope used for session persistence
    #[arg(long, value_name = "PATH", hide = true)]
    pub workspace: Option<String>,

    /// Run one provider-free session management request from stdin
    #[arg(long, hide = true)]
    pub session_control: bool,

    /// Increase stderr log verbosity
    #[arg(long)]
    pub verbose: bool,

    /// Registry-only mode: respond to one registry_request then exit
    #[arg(long)]
    pub registry: bool,

    /// Enable cosh-shell backed terminal output evidence tool
    #[arg(long)]
    pub enable_shell_evidence_tool: bool,

    // Compatibility flags — accepted but ignored
    #[arg(long, value_name = "FMT", hide = true)]
    pub output_format: Option<String>,

    #[arg(long, value_name = "FMT", hide = true)]
    pub input_format: Option<String>,

    #[arg(long, hide = true)]
    pub include_partial_messages: bool,

    /// Single-shot prompt (headless mode: send one user message then exit)
    pub prompt: Option<String>,
}

impl CliArgs {
    pub fn is_headless(&self) -> bool {
        self.headless || !atty::is(atty::Stream::Stdin)
    }

    pub fn is_registry(&self) -> bool {
        self.registry
    }

    pub fn is_session_control(&self) -> bool {
        self.session_control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_and_bare_are_generic_headless_flags() {
        let args = CliArgs::try_parse_from(["cosh-core", "--headless", "--bare", "--tools", ""])
            .expect("parse analyzer isolation flags");

        assert!(args.headless);
        assert!(args.bare);
        assert_eq!(args.tools.as_deref(), Some(""));
        assert!(args.allowed_tools.is_none());
    }

    #[test]
    fn tools_default_is_distinct_from_empty() {
        let default_args = CliArgs::try_parse_from(["cosh-core", "--tools", "default"])
            .expect("parse default tools");
        let empty_args =
            CliArgs::try_parse_from(["cosh-core", "--tools", ""]).expect("parse empty tools");

        assert_eq!(default_args.tools.as_deref(), Some("default"));
        assert_eq!(empty_args.tools.as_deref(), Some(""));
    }
}
