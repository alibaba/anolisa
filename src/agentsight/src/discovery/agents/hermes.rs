//! Hermes agent matcher
//!
//! Hermes Agent (by Nous Research) is a self-improving AI agent that runs via Python.
//! This matcher identifies it by checking the process name and command line arguments.
//!
//! # Matching Logic
//!
//! Hermes can appear in multiple process forms:
//!
//! 1. **Main process (CLI)**: `comm` = `hermes`, started via Python console-scripts entry point.
//!    cmdline: `/usr/local/lib/hermes-agent/venv/bin/python3 /usr/local/lib/hermes-agent/venv/bin/hermes`
//!
//! 2. **Gateway subprocess**: `comm` = `python`, running `python -m hermes_cli.main gateway run`.
//!    cmdline: `/usr/local/lib/hermes-agent/venv/bin/python -m hermes_cli.main gateway run --replace`
//!
//! 3. **Script wrapper** (noisy, not matched): `comm` = `script`, wrapping the hermes binary.

use crate::discovery::agent::AgentInfo;
use crate::discovery::matcher::{match_name_with_version_suffix, AgentMatcher, ProcessContext};

/// Custom matcher for Hermes Agent
///
/// Matches by either:
/// - Process name is "hermes" (Python console-scripts entry point renames the process)
/// - Process name is "python" (or python3) with "hermes" in cmdline args (gateway subprocess)
pub struct HermesMatcher {
    info: AgentInfo,
}

impl HermesMatcher {
    pub fn new() -> Self {
        Self {
            info: AgentInfo::new(
                "Hermes",
                vec!["hermes", "python3", "python"],
                "Hermes - self-improving AI agent by Nous Research",
                "ai-assistant",
            ),
        }
    }
}

impl AgentMatcher for HermesMatcher {
    fn info(&self) -> &AgentInfo {
        &self.info
    }

    fn matches(&self, ctx: &ProcessContext) -> bool {
        let comm_lower = ctx.comm.to_lowercase();

        // Case 1: Direct "hermes" process (Python console-scripts entry point)
        // When installed via pip/uv, the entry point script renames the process to "hermes"
        if match_name_with_version_suffix(&comm_lower, "hermes") {
            return true;
        }

        // Case 2: Python process with "hermes" in cmdline (gateway subprocess)
        // e.g., python -m hermes_cli.main gateway run --replace
        let is_python = match_name_with_version_suffix(&comm_lower, "python3")
            || match_name_with_version_suffix(&comm_lower, "python");
        if is_python {
            // Check if cmdline contains hermes-related module path
            let has_hermes = ctx.cmdline_args.iter().any(|arg| {
                let arg_lower = arg.to_lowercase();
                arg_lower.contains("hermes")
            });
            if has_hermes {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hermes_direct_process() {
        // Main process: comm = "hermes" (console-scripts entry point)
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "hermes".to_string(),
            cmdline_args: vec![
                "/usr/local/lib/hermes-agent/venv/bin/python3".to_string(),
                "/usr/local/lib/hermes-agent/venv/bin/hermes".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(matcher.matches(&ctx));
    }

    #[test]
    fn test_hermes_gateway_subprocess() {
        // Gateway subprocess: comm = "python", cmdline contains hermes_cli
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "python".to_string(),
            cmdline_args: vec![
                "/usr/local/lib/hermes-agent/venv/bin/python".to_string(),
                "-m".to_string(),
                "hermes_cli.main".to_string(),
                "gateway".to_string(),
                "run".to_string(),
                "--replace".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(matcher.matches(&ctx));
    }

    #[test]
    fn test_hermes_python3_gateway() {
        // Alternative: python3 with hermes in args
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "python3".to_string(),
            cmdline_args: vec![
                "/usr/bin/python3".to_string(),
                "-m".to_string(),
                "hermes_cli.main".to_string(),
                "gateway".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(matcher.matches(&ctx));
    }

    #[test]
    fn test_hermes_python3_with_version() {
        // Python3 with version suffix
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "python3.11".to_string(),
            cmdline_args: vec![
                "/usr/bin/python3.11".to_string(),
                "/home/user/.local/bin/hermes".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(matcher.matches(&ctx));
    }

    #[test]
    fn test_hermes_development_mode() {
        // Development mode: python3 + hermes-agent path
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "python3".to_string(),
            cmdline_args: vec![
                "python3".to_string(),
                "/home/user/hermes-agent/scripts/run.py".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(matcher.matches(&ctx));
    }

    #[test]
    fn test_non_python_process_not_matched() {
        // Node process should not match even with "hermes" in args
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "node".to_string(),
            cmdline_args: vec!["node".to_string(), "/usr/local/bin/hermes".to_string()],
            exe_path: String::new(),
        };
        assert!(!matcher.matches(&ctx));
    }

    #[test]
    fn test_python_without_hermes_not_matched() {
        // Plain Python process without hermes should not match
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "python3".to_string(),
            cmdline_args: vec![
                "python3".to_string(),
                "manage.py".to_string(),
                "runserver".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(!matcher.matches(&ctx));
    }

    #[test]
    fn test_script_wrapper_not_matched() {
        // The "script" wrapper process should NOT match
        // (it's just a PTY wrapper, not the agent itself)
        let matcher = HermesMatcher::new();
        let ctx = ProcessContext {
            comm: "script".to_string(),
            cmdline_args: vec![
                "script".to_string(),
                "-qc".to_string(),
                "/usr/local/lib/hermes-agent/venv/bin/hermes".to_string(),
                "/dev/null".to_string(),
            ],
            exe_path: String::new(),
        };
        assert!(!matcher.matches(&ctx));
    }

    #[test]
    fn test_hermes_info() {
        let matcher = HermesMatcher::new();
        let info = matcher.info();
        assert_eq!(info.name, "Hermes");
        assert_eq!(info.category, "ai-assistant");
    }
}
