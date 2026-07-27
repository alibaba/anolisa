use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage configured MCP servers.
    Mcp(McpArgs),
}

#[derive(clap::Args, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommand,
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// List configured MCP servers without exposing credentials.
    List,
    /// Connect to a configured MCP server and display its discovered tools.
    Connect {
        /// Configured MCP server name.
        server: String,
    },
    /// Display a configured MCP server and its currently discovered tools.
    Inspect {
        /// Configured MCP server name.
        server: String,
    },
    /// Reconnect to an enabled MCP server and rediscover its tools.
    Refresh {
        /// Configured MCP server name.
        server: String,
    },
    /// Disable a configured MCP server until it is connected again.
    Disconnect {
        /// Configured MCP server name.
        server: String,
    },
    /// Authorize an MCP server in a browser.
    Login {
        /// Configured MCP server name.
        server: String,
        /// Print the authorization URL and accept a pasted callback URL.
        #[arg(long)]
        manual: bool,
    },
    /// Remove saved OAuth credentials for an MCP server.
    Logout {
        /// Configured MCP server name.
        server: String,
    },
}

#[derive(Parser, Debug)]
#[command(
    name = "cosh-core",
    version,
    about = "cosh core — agent core + interactive terminal"
)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Command>,
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

    /// Compact the resumed session's model context and exit
    #[arg(long)]
    pub compact: bool,

    /// Marks a `--compact` run as automatically triggered (idle-boundary
    /// recommendation) rather than a manual `/session compact`. Affects the
    /// reported trigger and enables revision preflight validation.
    ///
    /// Only valid with `--compact`, and must always carry both
    /// `--expect-generation` and `--expect-revision` (clap enforces this as a
    /// first fail-closed layer, before any provider work).
    #[arg(
        long,
        hide = true,
        requires = "compact",
        requires = "expect_generation",
        requires = "expect_revision"
    )]
    pub auto_compact: bool,

    /// Expected session generation for an automatic compaction. When set with
    /// `--expect-revision`, the compactor fails closed (no provider call) if
    /// the session moved since the recommendation was emitted.
    ///
    /// Only valid on an automatic compaction; requires `--auto-compact` (which
    /// in turn requires `--compact`), so it can never appear on a manual run.
    #[arg(long, value_name = "N", hide = true, requires = "auto_compact")]
    pub expect_generation: Option<u64>,

    /// Expected projection revision for an automatic compaction; paired with
    /// `--expect-generation` to bind the attempt to one exact context.
    ///
    /// Only valid on an automatic compaction; requires `--auto-compact` (which
    /// in turn requires `--compact`), so it can never appear on a manual run.
    #[arg(long, value_name = "N", hide = true, requires = "auto_compact")]
    pub expect_revision: Option<u64>,

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

    pub fn is_compact(&self) -> bool {
        self.compact
    }
}

#[cfg(test)]
mod compaction_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CliArgs, clap::Error> {
        let mut full = vec!["cosh-core"];
        full.extend_from_slice(args);
        CliArgs::try_parse_from(full)
    }

    #[test]
    fn hidden_compaction_flags_require_compact() {
        // The three hidden compaction flags are meaningless without --compact
        // and must be rejected at the clap layer, before any runtime work.
        assert!(parse(&[
            "--auto-compact",
            "--expect-generation",
            "1",
            "--expect-revision",
            "0"
        ])
        .is_err());
        assert!(parse(&["--expect-generation", "1"]).is_err());
        assert!(parse(&["--expect-revision", "0"]).is_err());
    }

    #[test]
    fn auto_compact_requires_both_expected_bounds() {
        assert!(parse(&["--compact", "--auto-compact"]).is_err());
        assert!(parse(&["--compact", "--auto-compact", "--expect-generation", "1"]).is_err());
        assert!(parse(&["--compact", "--auto-compact", "--expect-revision", "0"]).is_err());
    }

    #[test]
    fn expected_bounds_are_rejected_on_manual_compact() {
        // generation/revision may only accompany an automatic compaction.
        assert!(parse(&[
            "--compact",
            "--expect-generation",
            "1",
            "--expect-revision",
            "0"
        ])
        .is_err());
        assert!(parse(&["--compact", "--expect-generation", "1"]).is_err());
    }

    #[test]
    fn valid_manual_and_auto_combinations_parse() {
        assert!(parse(&["--compact"]).is_ok());
        assert!(parse(&[
            "--compact",
            "--auto-compact",
            "--expect-generation",
            "1",
            "--expect-revision",
            "0",
        ])
        .is_ok());
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

    #[test]
    fn parses_mcp_login_command() {
        let args =
            CliArgs::try_parse_from(["cosh-core", "mcp", "login", "remote", "--manual"]).unwrap();
        let Some(Command::Mcp(McpArgs {
            command: McpCommand::Login { server, manual },
        })) = args.command
        else {
            panic!("expected mcp login command");
        };
        assert_eq!(server, "remote");
        assert!(manual);
    }

    #[test]
    fn parses_mcp_lifecycle_command() {
        let args = CliArgs::try_parse_from(["cosh-core", "mcp", "refresh", "remote"]).unwrap();
        let Some(Command::Mcp(McpArgs {
            command: McpCommand::Refresh { server },
        })) = args.command
        else {
            panic!("expected MCP refresh command");
        };
        assert_eq!(server, "remote");
    }

    #[test]
    fn preserves_single_shot_prompt() {
        let args = CliArgs::try_parse_from(["cosh-core", "summarize this"]).unwrap();
        assert_eq!(args.prompt.as_deref(), Some("summarize this"));
        assert!(args.command.is_none());
    }
}
