//! Agent process scanner
//!
//! This module provides functionality to scan the system for running AI agent processes
//! by examining /proc filesystem entries and handling process lifecycle events.
//! It also manages deny rules and domain rules for unified rule-based decisions.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::agent::{AgentInfo, DiscoveredAgent};
use super::matcher::{CmdlineGlobMatcher, ProcessContext, match_domain_glob};
use crate::config::{CmdlineRule, HttpsRule};

/// Scanner for discovering AI agent processes on the system
///
/// The scanner maintains allow matchers, deny matchers, and domain patterns.
/// It can scan the /proc filesystem to find running processes that match allow rules,
/// check deny rules, and match DNS domain events.
pub struct AgentScanner {
    /// Allow matchers (agent discovery)
    matchers: Vec<CmdlineGlobMatcher>,
    /// Deny matchers (blacklist)
    deny_matchers: Vec<CmdlineGlobMatcher>,
    /// Domain/DNS glob patterns
    domain_patterns: Vec<String>,
    /// Currently tracked agent processes: pid -> DiscoveredAgent
    tracked_agents: HashMap<u32, DiscoveredAgent>,
}

impl AgentScanner {
    /// Create a scanner from the full set of rules (recommended).
    ///
    /// Separates cmdline_rules into allow matchers and deny matchers,
    /// and stores domain patterns for DNS-based matching.
    pub fn from_rules(cmdline_rules: &[CmdlineRule], https_rules: &[HttpsRule]) -> Self {
        let matchers: Vec<CmdlineGlobMatcher> = cmdline_rules
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
            .collect();
        let deny_matchers: Vec<CmdlineGlobMatcher> = cmdline_rules
            .iter()
            .filter_map(CmdlineGlobMatcher::from_deny_rule)
            .collect();
        let domain_patterns: Vec<String> = https_rules.iter().map(|r| r.pattern.clone()).collect();
        Self {
            matchers,
            deny_matchers,
            domain_patterns,
            tracked_agents: HashMap::new(),
        }
    }

    /// Check if cmdline matches any deny rule.
    pub fn is_denied(&self, cmdline_args: &[String]) -> bool {
        let ctx = ProcessContext {
            comm: String::new(),
            cmdline_args: cmdline_args.to_vec(),
            exe_path: String::new(),
        };
        self.deny_matchers.iter().any(|m| m.matches(&ctx))
    }

    /// Check if a domain matches any domain rule.
    pub fn matches_domain(&self, domain: &str) -> bool {
        match_domain_glob(domain, &self.domain_patterns)
    }

    /// Whether any domain rules are configured (used to enable UDP DNS probe).
    pub fn has_domain_rules(&self) -> bool {
        !self.domain_patterns.is_empty()
    }

    /// Get a reference to the domain patterns (used by ConnectionScanner)
    pub fn domain_patterns(&self) -> &[String] {
        &self.domain_patterns
    }

    /// Handle DNS query event: check domain match + deny check.
    ///
    /// Returns `true` if the process should be attached (domain matches and
    /// the process cmdline is not denied).
    pub fn on_dns_event(&self, pid: u32, domain: &str) -> bool {
        if !self.matches_domain(domain) {
            return false;
        }
        let cmdline = read_cmdline(&format!("/proc/{pid}/cmdline"));
        // Fail-closed: if cmdline is empty (process already exited or unreadable),
        // do NOT attach — deny rules cannot be evaluated reliably.
        if cmdline.is_empty() {
            log::debug!("on_dns_event: pid={pid} cmdline empty (process exited?), skipping attach");
            return false;
        }
        !self.is_denied(&cmdline)
    }

    /// Scan the system for running AI agent processes
    ///
    /// This method iterates over /proc/[pid]/ directories and attempts to match
    /// each process against the known agent list based on process name.
    /// Discovered agents are automatically added to `tracked_agents`.
    ///
    /// # Returns
    ///
    /// A vector of `DiscoveredAgent` instances representing the found agent processes.
    pub fn scan(&mut self) -> Vec<DiscoveredAgent> {
        let mut discovered = Vec::new();

        // Read /proc directory
        let proc_path = Path::new("/proc");
        let entries = match fs::read_dir(proc_path) {
            Ok(e) => e,
            Err(_) => return discovered,
        };

        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Only process numeric directory names (PIDs)
            let pid: u32 = match name_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Try to read process info and match against known agents
            if let Some(discovered_agent) = self.try_match_process(pid) {
                self.tracked_agents
                    .insert(discovered_agent.pid, discovered_agent.clone());
                discovered.push(discovered_agent);
            }
        }

        discovered
    }

    /// Handle process creation event
    ///
    /// Check if the new process matches a known agent and start tracking it.
    ///
    /// # Arguments
    /// * `pid` - Process ID
    /// * `bpf_comm` - Process command name (from BPF event, already updated at sys_exit_execve)
    ///
    /// # Returns
    ///
    /// `Some(DiscoveredAgent)` if the process is a known agent, `None` otherwise.
    pub fn on_process_create(&mut self, pid: u32, bpf_comm: &str) -> Option<&DiscoveredAgent> {
        // Use BPF comm as primary source (already updated at sys_exit_execve time).
        // Fallback to /proc/[pid]/comm only if BPF comm is empty or too short.
        let comm = if bpf_comm.len() >= 3 {
            bpf_comm.to_string()
        } else {
            // Fallback: read from /proc/[pid]/comm
            let comm_path = format!("/proc/{pid}/comm");
            fs::read_to_string(&comm_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| bpf_comm.to_string())
        };

        // Read full command line from /proc/[pid]/cmdline
        let cmdline_args = read_cmdline(&format!("/proc/{pid}/cmdline"));
        log::debug!("Process created: pid={pid}, comm='{comm}', cmdline={cmdline_args:?}");

        // Read executable path from /proc/[pid]/exe (symlink)
        let exe_path_str = format!("/proc/{pid}/exe");
        let exe = fs::read_link(&exe_path_str)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let ctx = ProcessContext {
            comm,
            cmdline_args: cmdline_args.clone(),
            exe_path: exe.clone(),
        };

        // Find the first matching agent
        let matched_info = self.find_match(&ctx)?;

        let discovered = DiscoveredAgent {
            agent_info: matched_info,
            pid,
            cmdline_args,
            exe_path: exe,
        };

        self.tracked_agents.insert(pid, discovered);
        self.tracked_agents.get(&pid)
    }

    /// Handle process exit event
    ///
    /// Remove the process from tracking if it was a known agent.
    pub fn on_process_exit(&mut self, pid: u32) -> Option<DiscoveredAgent> {
        log::debug!("Process exited: pid={pid}");
        self.tracked_agents.remove(&pid)
    }

    /// Check if a PID is currently being tracked
    pub fn is_tracked(&self, pid: u32) -> bool {
        self.tracked_agents.contains_key(&pid)
    }

    /// Get a tracked agent by PID
    pub fn get_tracked(&self, pid: u32) -> Option<&DiscoveredAgent> {
        self.tracked_agents.get(&pid)
    }

    /// Get all currently tracked agents
    pub fn tracked_agents(&self) -> &HashMap<u32, DiscoveredAgent> {
        &self.tracked_agents
    }

    /// Get list of tracked PIDs
    pub fn tracked_pids(&self) -> Vec<u32> {
        self.tracked_agents.keys().copied().collect()
    }

    /// Clear all tracked agents
    pub fn clear_tracked(&mut self) {
        self.tracked_agents.clear();
    }

    /// Attempt to match a process against known agents
    fn try_match_process(&self, pid: u32) -> Option<DiscoveredAgent> {
        let proc_dir = format!("/proc/{pid}");

        // Read process name from /proc/[pid]/comm
        let comm_path = format!("{proc_dir}/comm");
        let comm = fs::read_to_string(&comm_path).ok()?;
        let process_name = comm.trim().to_string();

        // Read full command line from /proc/[pid]/cmdline
        let cmdline_path = format!("{proc_dir}/cmdline");
        let cmdline_args = read_cmdline(&cmdline_path);

        // Read executable path from /proc/[pid]/exe (symlink)
        let exe_path = format!("{proc_dir}/exe");
        let exe = fs::read_link(&exe_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let ctx = ProcessContext {
            comm: process_name,
            cmdline_args: cmdline_args.clone(),
            exe_path: exe.clone(),
        };

        let matched_info = self.find_match(&ctx)?;

        Some(DiscoveredAgent {
            agent_info: matched_info,
            pid,
            cmdline_args,
            exe_path: exe,
        })
    }

    /// Find the first matching agent for a process context
    fn find_match(&self, ctx: &ProcessContext) -> Option<AgentInfo> {
        for matcher in &self.matchers {
            if matcher.matches(ctx) {
                return Some(matcher.info().clone());
            }
        }
        None
    }

    /// Get the number of registered agent matchers
    pub fn matcher_count(&self) -> usize {
        self.matchers.len()
    }
}

/// Read and parse cmdline file
///
/// The cmdline file contains arguments separated by null bytes.
/// Returns a vector of command line arguments.
pub fn read_cmdline(path: &str) -> Vec<String> {
    match fs::read(path) {
        Ok(data) => {
            // Split by null bytes and collect non-empty strings
            data.split(|&b| b == 0)
                .filter_map(|slice| {
                    if slice.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(slice).into_owned())
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_creation() {
        let scanner = AgentScanner::from_rules(&crate::config::default_cmdline_rules(), &[]);
        assert!(scanner.matcher_count() > 0);
    }

    #[test]
    fn test_process_lifecycle() {
        let mut scanner = AgentScanner::from_rules(&crate::config::default_cmdline_rules(), &[]);

        // Initially no tracked agents
        assert!(scanner.tracked_pids().is_empty());

        // Simulate process exit for non-tracked PID
        let result = scanner.on_process_exit(99999);
        assert!(result.is_none());

        // Check is_tracked
        assert!(!scanner.is_tracked(99999));
    }

    #[test]
    fn test_is_denied() {
        let rules = vec![CmdlineRule {
            patterns: vec!["*spam*".to_string()],
            agent_name: None,
            allow: false,
        }];
        let scanner = AgentScanner::from_rules(&rules, &[]);

        assert!(scanner.is_denied(&["spam-process".to_string()]));
        assert!(!scanner.is_denied(&["good-process".to_string()]));
    }

    #[test]
    fn test_matches_domain() {
        let https_rules = vec![
            HttpsRule {
                pattern: "*.openai.com".to_string(),
            },
            HttpsRule {
                pattern: "*.anthropic.com".to_string(),
            },
        ];
        let scanner = AgentScanner::from_rules(&[], &https_rules);

        assert!(scanner.matches_domain("api.openai.com"));
        assert!(scanner.matches_domain("api.anthropic.com"));
        assert!(!scanner.matches_domain("example.com"));
        assert!(scanner.has_domain_rules());
    }

    #[test]
    fn test_has_no_domain_rules() {
        let scanner = AgentScanner::from_rules(&crate::config::default_cmdline_rules(), &[]);
        assert!(!scanner.has_domain_rules());
    }

    #[test]
    fn test_from_rules_separates_allow_and_deny() {
        let rules = vec![
            CmdlineRule {
                patterns: vec!["node".to_string(), "*claude*".to_string()],
                agent_name: Some("Claude".to_string()),
                allow: true,
            },
            CmdlineRule {
                patterns: vec!["*deny-me*".to_string()],
                agent_name: None,
                allow: false,
            },
        ];
        let scanner = AgentScanner::from_rules(&rules, &[]);

        // One allow matcher
        assert_eq!(scanner.matcher_count(), 1);
        // Deny works
        assert!(scanner.is_denied(&["deny-me-process".to_string()]));
        assert!(!scanner.is_denied(&["node".to_string(), "/path/claude-code".to_string()]));
    }
}
