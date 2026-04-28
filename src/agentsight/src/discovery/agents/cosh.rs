//! Cosh agent matcher
//!
//! Cosh (OS Copilot) is a shell terminal agent that runs via Node.js.
//! This matcher identifies it by checking if the process is node with
//! a Cosh-related path in its command line arguments.

use crate::discovery::agent::AgentInfo;
use crate::discovery::matcher::{AgentMatcher, ProcessContext, match_name_with_version_suffix};

/// Custom matcher for Cosh (OS Copilot)
///
/// Matches by: comm is "node" (or node-XX) and cmdline contains Cosh paths.
/// Variant comm names like "cosh"/"os-copilot" are handled by
/// `normalize_agent_name()` as a fallback in `resolve_agent_name_from_comm`.
pub struct CoshMatcher {
    info: AgentInfo,
}

impl CoshMatcher {
    pub fn new() -> Self {
        Self {
            info: AgentInfo::new(
                "Cosh",
                vec!["node"],
                "Cosh - OS Copilot, shell terminal AI assistant",
                "shell-assistant",
            ),
        }
    }
}

impl AgentMatcher for CoshMatcher {
    fn info(&self) -> &AgentInfo {
        &self.info
    }

    fn matches(&self, ctx: &ProcessContext) -> bool {
        let comm_lower = ctx.comm.to_lowercase();

        // Match: node runtime with Cosh-related paths in cmdline args
        let is_node = match_name_with_version_suffix(&comm_lower, "node");
        let has_cosh_path = ctx.cmdline_args.iter().any(|arg| {
            arg == "/usr/bin/co"
                || arg == "/usr/bin/cosh"
                || arg == "/usr/bin/copilot"
                || arg == "/usr/local/lib/copilot-shell/cli.js"
                || arg == "/usr/lib/copilot-shell/cli.js"
        });

        is_node && has_cosh_path
    }
}
