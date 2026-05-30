//! Discover subcommand - scan for running AI agents
//!
//! This module provides the `discover` subcommand which scans the system
//! for running AI agent processes.

use agentsight::{AgentScanner, CmdlineGlobMatcher};
use structopt::StructOpt;

/// Discover subcommand for finding AI agents running on the system
#[derive(Debug, StructOpt, Clone)]
pub struct DiscoverCommand {
    /// Show detailed output including executable path
    #[structopt(short, long)]
    pub verbose: bool,

    /// List all known agents without scanning
    #[structopt(long)]
    pub list_known: bool,
}

impl DiscoverCommand {
    pub fn execute(&self) {
        if self.list_known {
            self.list_known_agents();
            return;
        }

        self.scan_agents();
    }

    /// List all known agents that can be detected
    fn list_known_agents(&self) {
        let rules = agentsight::default_cmdline_rules();
        let scanner = AgentScanner::from_rules(&rules, &[]);
        let count = scanner.matcher_count();

        println!("Known AI Agents ({count} total):");
        println!("{}", "=".repeat(60));
        println!();

        // Use CmdlineGlobMatcher to list agent info
        for matcher in agentsight::default_cmdline_rules()
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
        {
            let agent = matcher.info();
            println!("  {} ({})", agent.name, agent.category);
            println!("    Process names: {}", agent.process_names.join(", "));
            println!("    {}", agent.description);
            println!();
        }
    }

    /// Scan the system for running AI agents
    fn scan_agents(&self) {
        let mut scanner = AgentScanner::from_rules(&agentsight::default_cmdline_rules(), &[]);
        let agents = scanner.scan();

        if agents.is_empty() {
            println!("No AI agents found running on this system.");
            println!();
            println!("Tip: Use --list-known to see all detectable agents.");
            return;
        }

        println!("Discovered AI Agents ({} found):", agents.len());
        println!("{}", "=".repeat(60));
        println!();

        for agent in &agents {
            println!("  {} [PID: {}]", agent.agent_info.name, agent.pid);
            println!("    Category: {}", agent.agent_info.category);

            // Truncate long command lines
            let cmdline_str = agent.cmdline_args.join(" ");
            let cmdline = if cmdline_str.len() > 80 && !self.verbose {
                format!("{}...", &cmdline_str[..77])
            } else {
                cmdline_str
            };
            println!("    Command:  {cmdline}");

            if self.verbose && !agent.exe_path.is_empty() {
                println!("    Executable: {}", agent.exe_path);
            }

            println!();
        }

        println!("Total: {} agent(s) found", agents.len());
    }
}
