use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "cosh-tui", about = "cosh TUI — agent core + interactive terminal")]
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

    /// Increase stderr log verbosity
    #[arg(long)]
    pub verbose: bool,

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
}
