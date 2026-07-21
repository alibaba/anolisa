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
