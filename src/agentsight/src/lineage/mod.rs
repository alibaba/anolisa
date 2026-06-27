//! Blood lineage tree for AI agent process tracking
//!
//! Maintains a userspace mirror of the BPF lineage_tree map, enriched with
//! process type classification (Agent / SubAgent / Tool / Skill).

use std::collections::HashMap;

use serde::Serialize;

/// Process type classification for lineage tree nodes.
///
/// `Skill` is forward-declared for the follow-up scoring task — `classify()`
/// does NOT currently produce it (today's classifier covers Agent / SubAgent /
/// Tool only). The variant exists in `from_u32`/`as_u32` so a future BPF-side
/// scorer can emit it without breaking userspace decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessType {
    Unknown,
    Agent,
    SubAgent,
    Tool,
    /// Forward-declared. Not produced by the current `classify()`.
    Skill,
}

impl ProcessType {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Agent,
            2 => Self::SubAgent,
            3 => Self::Tool,
            4 => Self::Skill,
            _ => Self::Unknown,
        }
    }

    pub fn as_u32(&self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::Agent => 1,
            Self::SubAgent => 2,
            Self::Tool => 3,
            Self::Skill => 4,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Agent => "agent",
            Self::SubAgent => "sub_agent",
            Self::Tool => "tool",
            Self::Skill => "skill",
        }
    }
}

/// Flags on a lineage node (mirrors LINEAGE_FLAG_* from BPF)
pub const LINEAGE_FLAG_AGENT_MODE: u32 = 1 << 0;

/// A single node in the lineage tree
#[derive(Debug, Clone, Serialize)]
pub struct LineageNode {
    pub pid: u32,
    pub ppid: u32,
    pub process_type: ProcessType,
    pub flags: u32,
    pub create_time_ns: u64,
    pub comm: String,
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<u32>,
}

impl LineageNode {
    pub fn has_agent_mode(&self) -> bool {
        self.flags & LINEAGE_FLAG_AGENT_MODE != 0
    }
}

/// Userspace lineage tree — mirrors BPF lineage_tree map with classification
#[derive(Default)]
pub struct LineageTree {
    nodes: HashMap<u32, LineageNode>,
}

impl LineageTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or update a node. Automatically maintains parent→child links.
    ///
    /// Also handles PID re-parenting: if the pid already exists under a
    /// different parent (e.g. PID reuse where the old Exit was missed under
    /// ringbuf pressure), the stale child entry is removed from the old
    /// parent's children list before inserting under the new parent.
    pub fn insert(&mut self, mut node: LineageNode) {
        let pid = node.pid;
        let ppid = node.ppid;

        // PID-reuse / re-parent: detach from old parent first.
        if let Some(old_node) = self.nodes.get(&pid) {
            let old_ppid = old_node.ppid;
            if old_ppid != ppid {
                if let Some(old_parent) = self.nodes.get_mut(&old_ppid) {
                    old_parent.children.retain(|&c| c != pid);
                }
            }
        }

        // Add this pid as a child of its (possibly new) parent.
        if let Some(parent) = self.nodes.get_mut(&ppid) {
            if !parent.children.contains(&pid) {
                parent.children.push(pid);
            }
        }

        if let Some(old) = self.nodes.get(&pid) {
            node.children = old.children.clone();
        }

        self.nodes.insert(pid, node);
    }

    /// Remove a node, reparent its children to grandparent, and clean up links.
    pub fn remove(&mut self, pid: u32) -> Option<LineageNode> {
        let node = self.nodes.remove(&pid)?;

        // Reparent children to the removed node's parent (mirrors kernel subreaper)
        for &child_pid in &node.children {
            if let Some(child) = self.nodes.get_mut(&child_pid) {
                child.ppid = node.ppid;
            }
        }

        // Update parent's children list: remove this node, add its children
        if let Some(parent) = self.nodes.get_mut(&node.ppid) {
            parent.children.retain(|&c| c != pid);
            for child_pid in &node.children {
                if !parent.children.contains(child_pid) {
                    parent.children.push(*child_pid);
                }
            }
        }

        Some(node)
    }

    /// Get a reference to a node
    pub fn get(&self, pid: u32) -> Option<&LineageNode> {
        self.nodes.get(&pid)
    }

    /// Get a mutable reference to a node
    pub fn get_mut(&mut self, pid: u32) -> Option<&mut LineageNode> {
        self.nodes.get_mut(&pid)
    }

    /// Insert a process (if not already present) and classify it in one step.
    /// Returns the classified ProcessType.
    ///
    /// This is the primary entry point for unified.rs glue code: it combines
    /// node creation + insertion + classification into a single pure-function
    /// call on the tree, enabling full unit-test coverage without BPF.
    pub fn insert_and_classify(
        &mut self,
        pid: u32,
        ppid: u32,
        comm: &str,
        agent_name: Option<String>,
        has_agent_mode: bool,
        matches_agent: bool,
    ) -> ProcessType {
        if self.get(pid).is_none() {
            let node = LineageNode {
                pid,
                ppid,
                process_type: ProcessType::Unknown,
                flags: if has_agent_mode {
                    LINEAGE_FLAG_AGENT_MODE
                } else {
                    0
                },
                create_time_ns: 0,
                comm: comm.to_string(),
                agent_name,
                children: vec![],
            };
            self.insert(node);
        }
        self.classify(pid, has_agent_mode, matches_agent);
        self.get(pid)
            .map(|n| n.process_type)
            .unwrap_or(ProcessType::Unknown)
    }

    /// Classify a newly added process based on its ancestry and environment.
    ///
    /// Rules (evaluated in order):
    /// 1. Parent is Agent/SubAgent → child inherits lineage:
    ///    - matches agent pattern → SubAgent
    ///    - otherwise → Tool
    /// 2. No tracked parent + matches agent pattern → Agent (cmdline rule match)
    /// 3. No tracked parent + AGENT_MODE=1 in env → Agent (reserved for future
    ///    agent frameworks that self-declare; currently no component sets this)
    /// 4. Otherwise → Unknown
    ///
    /// Note: child processes inherit AGENT_MODE=1 via environment, but that
    /// does NOT make them Agents — only top-level processes (without a tracked
    /// parent) are classified as Agent via AGENT_MODE. Cmdline pattern matching
    /// takes precedence for root classification.
    pub fn classify(&mut self, pid: u32, has_agent_mode_env: bool, matches_agent_pattern: bool) {
        let ppid = match self.nodes.get(&pid) {
            Some(n) => n.ppid,
            None => return,
        };

        let parent_in_tree = self.nodes.get(&ppid);
        let parent_type = parent_in_tree
            .map(|p| p.process_type)
            .unwrap_or(ProcessType::Unknown);

        let process_type = match parent_type {
            ProcessType::Agent | ProcessType::SubAgent => {
                if matches_agent_pattern {
                    ProcessType::SubAgent
                } else {
                    ProcessType::Tool
                }
            }
            _ => {
                if matches_agent_pattern || has_agent_mode_env {
                    ProcessType::Agent
                } else {
                    ProcessType::Unknown
                }
            }
        };

        if let Some(node) = self.nodes.get_mut(&pid) {
            node.process_type = process_type;
        }
    }

    /// Record a process exec: classify it and stamp its create time.
    ///
    /// Derives the classification inputs the way the live event pipeline does:
    /// - `has_agent_mode` is read from any pre-existing node (AGENT_MODE is
    ///   inherited via env and preserved across re-exec), defaulting to false.
    /// - `matches_agent` is true when discovery resolved an `agent_name` AND the
    ///   process is not already an AGENT_MODE process (cmdline match wins for
    ///   roots; an AGENT_MODE child stays a Tool — see `classify`).
    ///
    /// Returns the resulting `ProcessType`. This is the pure tree-side of
    /// `unified::update_lineage_from_proc`, extracted so it can be unit-tested
    /// without the BPF event pipeline.
    pub fn record_exec(
        &mut self,
        pid: u32,
        ppid: u32,
        comm: &str,
        agent_name: Option<String>,
        create_time_ns: u64,
    ) -> ProcessType {
        let has_agent_mode = self.get(pid).map(|n| n.has_agent_mode()).unwrap_or(false);
        let matches_agent = agent_name.is_some() && !has_agent_mode;
        let ptype =
            self.insert_and_classify(pid, ppid, comm, agent_name, has_agent_mode, matches_agent);
        if let Some(node) = self.get_mut(pid) {
            node.create_time_ns = create_time_ns;
        }
        ptype
    }

    /// Snapshot of direct child PIDs at exit time.
    ///
    /// Not a liveness check — returns whichever children are in the tree when
    /// called, regardless of whether they are still running.
    pub fn children_at_exit(&self, pid: u32) -> Vec<u32> {
        self.get(pid)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    }

    /// Walk the ppid chain from `pid` upward to find the root Agent ancestor.
    /// Returns `None` if `pid` is itself an Agent, is absent, or has no Agent
    /// ancestor in the tree.
    pub fn root_agent_ancestor(&self, pid: u32) -> Option<&LineageNode> {
        let node = self.nodes.get(&pid)?;
        if node.process_type == ProcessType::Agent {
            return None;
        }
        let mut current_ppid = node.ppid;
        for _ in 0..64 {
            let parent = self.nodes.get(&current_ppid)?;
            if parent.process_type == ProcessType::Agent {
                return Some(parent);
            }
            current_ppid = parent.ppid;
        }
        None
    }

    /// Get the full subtree rooted at `pid` as a serializable structure
    pub fn subtree(&self, pid: u32) -> Option<LineageSubtree> {
        self.subtree_inner(pid, 0)
    }

    fn subtree_inner(&self, pid: u32, depth: u32) -> Option<LineageSubtree> {
        if depth > 64 {
            return None;
        }
        let node = self.nodes.get(&pid)?;
        let children = node
            .children
            .iter()
            .filter_map(|&cpid| self.subtree_inner(cpid, depth + 1))
            .collect();
        Some(LineageSubtree {
            pid: node.pid,
            ppid: node.ppid,
            process_type: node.process_type,
            flags: node.flags,
            create_time_ns: node.create_time_ns,
            comm: node.comm.clone(),
            agent_name: node.agent_name.clone(),
            children,
        })
    }

    /// Return all root nodes (nodes whose ppid is not in the tree)
    pub fn roots(&self) -> Vec<u32> {
        self.nodes
            .values()
            .filter(|n| !self.nodes.contains_key(&n.ppid))
            .map(|n| n.pid)
            .collect()
    }

    /// Snapshot the entire tree as a flat list.
    pub fn snapshot(&self) -> Vec<&LineageNode> {
        self.nodes.values().collect()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Recursive subtree for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct LineageSubtree {
    pub pid: u32,
    pub ppid: u32,
    pub process_type: ProcessType,
    pub flags: u32,
    pub create_time_ns: u64,
    pub comm: String,
    pub agent_name: Option<String>,
    pub children: Vec<LineageSubtree>,
}

// ─── Exit classification (crash detection) ───────────────────────────────────

/// Kernel wait-status decoded into a crash classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitClassification {
    /// Fatal signal (SIGSEGV, SIGKILL, SIGABRT, SIGBUS, etc.)
    SignalCrash { signal: u8, coredump: bool },
    /// Graceful stop (SIGTERM, SIGINT, SIGHUP)
    GracefulStop { signal: u8 },
    /// Benign signal (SIGPIPE)
    BenignSignal { signal: u8 },
    /// Clean exit (exit(0))
    NormalExit,
    /// Non-zero exit status (exit(N), N != 0)
    AbnormalExit { status: u8 },
}

impl ExitClassification {
    pub fn is_crash(&self) -> bool {
        matches!(self, Self::SignalCrash { .. } | Self::AbnormalExit { .. })
    }
}

/// Classify a raw kernel exit_code into a crash category.
///
/// exit_code layout (wait(2) encoding):
///   bits [6:0] = terminating signal (0 if normal exit)
///   bit  [7]   = core dump produced
///   bits [15:8] = exit status (only meaningful when signal == 0)
pub fn classify_exit(exit_code: i32) -> ExitClassification {
    let signal = (exit_code & 0x7f) as u8;
    let coredump = (exit_code >> 7) & 1 != 0;
    let status = ((exit_code >> 8) & 0xff) as u8;

    if signal == 0 {
        if status == 0 {
            ExitClassification::NormalExit
        } else {
            ExitClassification::AbnormalExit { status }
        }
    } else {
        match signal {
            // SIGTERM(15), SIGINT(2), SIGHUP(1): graceful stop
            15 | 2 | 1 => ExitClassification::GracefulStop { signal },
            // SIGPIPE(13): benign (broken pipe)
            13 => ExitClassification::BenignSignal { signal },
            // Everything else: crash (SIGSEGV=11, SIGKILL=9, SIGABRT=6, SIGBUS=7, ...)
            _ => ExitClassification::SignalCrash { signal, coredump },
        }
    }
}

/// Map a signal number to its conventional name.
pub fn signal_name(signal: u8) -> &'static str {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        7 => "SIGBUS",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "SIG?",
    }
}

/// Format a one-line crash cause from a raw exit_code.
pub fn format_crash_cause(exit_code: i32) -> String {
    let classification = classify_exit(exit_code);
    match classification {
        ExitClassification::SignalCrash { signal, coredump } => {
            format!(
                "{} (signal {signal}, coredump: {coredump})",
                signal_name(signal)
            )
        }
        ExitClassification::AbnormalExit { status } => format!("exit({status})"),
        ExitClassification::NormalExit => "exit(0)".to_string(),
        ExitClassification::GracefulStop { signal } => {
            format!("{} (graceful)", signal_name(signal))
        }
        ExitClassification::BenignSignal { signal } => {
            format!("{} (benign)", signal_name(signal))
        }
    }
}

/// Build a structured crash report string from the detail JSON of an agent_crash event.
///
/// Expects the detail schema written by `record_agent_crash_interruptions`:
/// `signal` / `exit_code` / `core_dump` are the decoded wait(2) fields.
pub fn format_crash_report(detail: &serde_json::Value) -> String {
    let mut out = String::new();
    out.push_str("\n  --- Crash Report ---\n");
    let signal = detail.get("signal").and_then(|v| v.as_u64()).unwrap_or(0);
    let exit_status = detail
        .get("exit_code")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let coredump = detail
        .get("core_dump")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let oom = detail.get("oom").and_then(|v| v.as_bool()).unwrap_or(false);

    if signal > 0 {
        let sig_name = signal_name(signal as u8);
        out.push_str(&format!(
            "  Cause:        {sig_name} (signal {signal}, coredump: {coredump})\n"
        ));
    } else {
        out.push_str(&format!("  Cause:        exit({exit_status})\n"));
    }
    out.push_str(&format!(
        "  OOM Killed:   {}\n",
        if oom { "yes" } else { "no" }
    ));
    if let Some(ptype) = detail.get("process_type").and_then(|v| v.as_str()) {
        out.push_str(&format!("  Process Type: {ptype}\n"));
    }
    if let Some(blast) = detail.get("blast_radius").and_then(|v| v.as_str()) {
        out.push_str(&format!("  Blast Radius: {blast}\n"));
    }

    if let Some(calls) = detail.get("call_ids").and_then(|v| v.as_array()) {
        if !calls.is_empty() {
            out.push_str(&format!("  Pending LLM Calls ({}):\n", calls.len()));
            for c in calls {
                if let Some(s) = c.as_str() {
                    out.push_str(&format!("    - {s}\n"));
                }
            }
        }
    }

    if let Some(children) = detail.get("children_at_exit").and_then(|v| v.as_array()) {
        if !children.is_empty() {
            let pids: Vec<String> = children
                .iter()
                .filter_map(|v| v.as_u64().map(|p| p.to_string()))
                .collect();
            out.push_str(&format!("  Children at Exit: [{}]\n", pids.join(", ")));
        }
    }

    if let Some(tree) = detail.get("process_tree") {
        out.push_str("\n  Process Tree at Crash:\n");
        render_tree_to_string(tree, "    ", true, &mut out);
    }
    out
}

/// Render a process tree JSON node into a string with tree connectors.
pub fn render_tree_to_string(
    node: &serde_json::Value,
    prefix: &str,
    is_last: bool,
    out: &mut String,
) {
    let pid = node.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
    let comm = node.get("comm").and_then(|v| v.as_str()).unwrap_or("?");
    let ptype = node
        .get("process_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let connector = if is_last { "`-- " } else { "|-- " };
    out.push_str(&format!("{prefix}{connector}{comm} [{pid}] ({ptype})\n"));
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "|   " });
        for (i, child) in children.iter().enumerate() {
            render_tree_to_string(child, &child_prefix, i == children.len() - 1, out);
        }
    }
}

/// Format token usage by process type as JSON string.
pub fn format_token_by_type_json(rows: &[(String, i64, i64, i64, i64)]) -> String {
    let data: Vec<_> = rows
        .iter()
        .map(|(ptype, count, input, output, total)| {
            serde_json::json!({
                "process_type": ptype,
                "call_count": count,
                "input_tokens": input,
                "output_tokens": output,
                "total_tokens": total,
            })
        })
        .collect();
    serde_json::to_string_pretty(&data).unwrap_or_default()
}

/// Format token usage by process type as a table string.
pub fn format_token_by_type_table(rows: &[(String, i64, i64, i64, i64)], hours: u64) -> String {
    use crate::format_tokens_with_commas;
    let mut out = format!("Token Usage by Process Type (last {hours}h)\n\n");
    out.push_str(&format!(
        "{:<12} {:>8} {:>12} {:>12} {:>12}\n",
        "TYPE", "CALLS", "INPUT", "OUTPUT", "TOTAL"
    ));
    out.push_str(&format!("{}\n", "-".repeat(60)));
    let mut grand_total = 0i64;
    for (ptype, count, input, output, total) in rows {
        out.push_str(&format!(
            "{:<12} {:>8} {:>12} {:>12} {:>12}\n",
            ptype,
            count,
            format_tokens_with_commas(*input as u64),
            format_tokens_with_commas(*output as u64),
            format_tokens_with_commas(*total as u64),
        ));
        grand_total += total;
    }
    out.push_str(&format!("{}\n", "-".repeat(60)));
    out.push_str(&format!(
        "{:<12} {:>8} {:>12} {:>12} {:>12}\n",
        "TOTAL",
        "",
        "",
        "",
        format_tokens_with_commas(grand_total as u64)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(pid: u32, ppid: u32, ptype: ProcessType) -> LineageNode {
        LineageNode {
            pid,
            ppid,
            process_type: ptype,
            flags: 0,
            create_time_ns: 0,
            comm: format!("proc-{pid}"),
            agent_name: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn test_insert_and_parent_link() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));

        let parent = tree.get(100).unwrap();
        assert_eq!(parent.children, vec![200]);
    }

    #[test]
    fn test_remove_cleans_parent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.remove(200);

        let parent = tree.get(100).unwrap();
        assert!(parent.children.is_empty());
    }

    #[test]
    fn test_classify_agent_mode() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Unknown));
        tree.classify(100, true, false);
        assert_eq!(tree.get(100).unwrap().process_type, ProcessType::Agent);
    }

    #[test]
    fn test_classify_tool_under_agent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Unknown));
        tree.classify(200, false, false);
        assert_eq!(tree.get(200).unwrap().process_type, ProcessType::Tool);
    }

    #[test]
    fn test_classify_subagent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Unknown));
        tree.classify(200, false, true);
        assert_eq!(tree.get(200).unwrap().process_type, ProcessType::SubAgent);
    }

    #[test]
    fn test_roots() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.insert(make_node(300, 2, ProcessType::Agent));

        let roots = tree.roots();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&100));
        assert!(roots.contains(&300));
    }

    /// PID-reuse / re-parent: a pid that re-execs (or whose Exit was dropped)
    /// under a different parent must NOT leave a phantom child entry on the
    /// old parent. Discriminating: without the cleanup branch in insert(), the
    /// final assertion below fails — old parent still lists pid 200.
    #[test]
    fn test_insert_cleans_old_parent_on_reparent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(300, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        // Re-insert pid=200 under a different parent (e.g. PID reuse).
        tree.insert(make_node(200, 300, ProcessType::Tool));

        assert!(
            tree.get(100).unwrap().children.is_empty(),
            "old parent 100 must not retain a phantom child 200"
        );
        assert_eq!(tree.get(300).unwrap().children, vec![200]);
        assert_eq!(tree.get(200).unwrap().ppid, 300);
    }

    /// AGENT_MODE precedence pin (1/2): when the parent is already an Agent,
    /// a child with has_agent_mode_env=true must classify as Tool — NOT
    /// promote itself to Agent. The env var is inherited through fork; only
    /// top-level processes (no tracked parent) are eligible for Agent.
    /// Discriminating: reordering the match arms in classify() so that
    /// AGENT_MODE wins over parent_type would flip this case to Agent.
    #[test]
    fn test_classify_agent_mode_inherited_under_agent_stays_tool() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Unknown));
        tree.classify(
            200, /* has_agent_mode_env */ true, /* matches_agent */ false,
        );
        assert_eq!(tree.get(200).unwrap().process_type, ProcessType::Tool);
    }

    /// AGENT_MODE precedence pin (2/2): same rule but parent is SubAgent.
    /// Inherited AGENT_MODE under a SubAgent is still a Tool.
    #[test]
    fn test_classify_agent_mode_inherited_under_subagent_stays_tool() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::SubAgent));
        tree.insert(make_node(300, 200, ProcessType::Unknown));
        tree.classify(
            300, /* has_agent_mode_env */ true, /* matches_agent */ false,
        );
        assert_eq!(tree.get(300).unwrap().process_type, ProcessType::Tool);
    }

    /// `ProcessType::Skill` is forward-declared. Document the current
    /// behaviour: classify() never produces Skill regardless of inputs.
    /// Tightens the contract so a future scorer change is explicit.
    #[test]
    fn test_classify_never_produces_skill_today() {
        // Try a variety of parent types + flags; none should yield Skill.
        for parent_type in [
            ProcessType::Unknown,
            ProcessType::Agent,
            ProcessType::SubAgent,
            ProcessType::Tool,
            ProcessType::Skill,
        ] {
            let mut tree = LineageTree::new();
            tree.insert(make_node(100, 1, parent_type));
            tree.insert(make_node(200, 100, ProcessType::Unknown));
            for has_env in [false, true] {
                for matches in [false, true] {
                    tree.classify(200, has_env, matches);
                    assert_ne!(
                        tree.get(200).unwrap().process_type,
                        ProcessType::Skill,
                        "classify() must not produce Skill (forward-declared)",
                    );
                }
            }
        }
    }

    #[test]
    fn test_classify_cmdline_match_root_becomes_agent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Unknown));
        // cmdline-matched root with no AGENT_MODE — should still become Agent
        tree.classify(
            100, /* has_agent_mode_env */ false, /* matches_agent */ true,
        );
        assert_eq!(
            tree.get(100).unwrap().process_type,
            ProcessType::Agent,
            "cmdline-matched root process must be classified as Agent"
        );
    }

    #[test]
    fn test_reinsert_preserves_children() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.insert(make_node(300, 200, ProcessType::Tool));

        assert_eq!(tree.get(200).unwrap().children, vec![300]);

        // Re-insert 200 (simulates re-execve or procmon+proctrace double-insert)
        tree.insert(make_node(200, 100, ProcessType::Tool));

        assert_eq!(
            tree.get(200).unwrap().children,
            vec![300],
            "re-insert must preserve accumulated children",
        );
    }

    #[test]
    fn test_lineage_node_fields_and_flags() {
        let node = LineageNode {
            pid: 1,
            ppid: 0,
            process_type: ProcessType::Agent,
            flags: LINEAGE_FLAG_AGENT_MODE,
            create_time_ns: 12345,
            comm: "test".to_string(),
            agent_name: Some("TestAgent".to_string()),
            children: vec![2, 3],
        };
        assert!(node.has_agent_mode());
        assert_eq!(node.pid, 1);
        assert_eq!(node.ppid, 0);
        assert_eq!(node.comm, "test");
        assert_eq!(node.agent_name, Some("TestAgent".to_string()));
        assert_eq!(node.children, vec![2, 3]);

        // without flag
        let node2 = LineageNode {
            pid: 2,
            ppid: 1,
            process_type: ProcessType::Tool,
            flags: 0,
            create_time_ns: 0,
            comm: "bash".to_string(),
            agent_name: None,
            children: vec![],
        };
        assert!(!node2.has_agent_mode());
    }

    #[test]
    fn test_snapshot() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.insert(make_node(300, 100, ProcessType::SubAgent));

        let snap = tree.snapshot();
        assert_eq!(snap.len(), 3);
    }

    #[test]
    fn test_subtree() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.insert(make_node(300, 100, ProcessType::SubAgent));
        tree.insert(make_node(400, 200, ProcessType::Tool));

        // subtree from root
        let sub = tree.subtree(100);
        assert!(sub.is_some());
        let sub = sub.unwrap();
        assert_eq!(sub.pid, 100);
        assert_eq!(sub.children.len(), 2);

        // subtree from leaf
        let leaf = tree.subtree(400);
        assert!(leaf.is_some());
        assert_eq!(leaf.unwrap().children.len(), 0);

        // subtree from non-existent
        let none = tree.subtree(999);
        assert!(none.is_none());
    }

    #[test]
    fn test_remove_reparents_children_to_grandparent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(1, 0, ProcessType::Agent));
        tree.insert(make_node(2, 1, ProcessType::Tool));
        tree.insert(make_node(3, 2, ProcessType::Tool));
        tree.insert(make_node(4, 2, ProcessType::Tool));

        // Remove middle node (pid=2), children should reparent to pid=1
        let removed = tree.remove(2);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().pid, 2);

        // Children 3,4 should now have ppid=1
        assert_eq!(tree.get(3).unwrap().ppid, 1);
        assert_eq!(tree.get(4).unwrap().ppid, 1);

        // Parent 1 should have children 3,4
        let parent = tree.get(1).unwrap();
        assert!(parent.children.contains(&3));
        assert!(parent.children.contains(&4));
        assert!(!parent.children.contains(&2));
    }

    #[test]
    fn test_remove_reparent_deduplicates_existing_child_link() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(1, 0, ProcessType::Agent));
        tree.insert(make_node(2, 1, ProcessType::Tool));
        tree.insert(make_node(3, 2, ProcessType::Tool));
        // Simulate an out-of-order / duplicate parent edge that can exist after
        // missed exits or PID reuse. Removing 2 should not duplicate child 3.
        tree.get_mut(1).unwrap().children.push(3);

        tree.remove(2);

        let children = &tree.get(1).unwrap().children;
        assert_eq!(
            children.iter().filter(|&&pid| pid == 3).count(),
            1,
            "reparent must not duplicate an existing child edge"
        );
        assert_eq!(tree.get(3).unwrap().ppid, 1);
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut tree = LineageTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);

        tree.insert(make_node(100, 1, ProcessType::Unknown));
        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);

        tree.remove(100);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_classify_nonexistent_pid_is_noop() {
        let mut tree = LineageTree::new();
        // Should not panic, just return early
        tree.classify(999, true, true);
        assert!(tree.get(999).is_none());
    }

    // --- insert_and_classify tests ---

    #[test]
    fn test_insert_and_classify_root_agent() {
        let mut tree = LineageTree::new();
        let pt = tree.insert_and_classify(100, 1, "cosh", Some("Cosh".to_string()), false, true);
        assert_eq!(pt, ProcessType::Agent);
        assert_eq!(tree.get(100).unwrap().comm, "cosh");
        assert_eq!(tree.get(100).unwrap().agent_name, Some("Cosh".to_string()));
    }

    #[test]
    fn test_insert_and_classify_tool_under_agent() {
        let mut tree = LineageTree::new();
        tree.insert_and_classify(100, 1, "cosh", Some("Cosh".to_string()), false, true);
        let pt = tree.insert_and_classify(200, 100, "bash", None, false, false);
        assert_eq!(pt, ProcessType::Tool);
    }

    #[test]
    fn test_insert_and_classify_with_agent_mode() {
        let mut tree = LineageTree::new();
        let pt = tree.insert_and_classify(
            100,
            1,
            "python",
            Some("agent-mode-python".to_string()),
            true,
            false,
        );
        assert_eq!(pt, ProcessType::Agent);
        assert!(tree.get(100).unwrap().has_agent_mode());
    }

    #[test]
    fn test_insert_and_classify_skips_existing() {
        let mut tree = LineageTree::new();
        tree.insert_and_classify(100, 1, "cosh", Some("Cosh".to_string()), false, true);
        // Second call should not overwrite
        let pt = tree.insert_and_classify(100, 1, "different", None, false, false);
        // Should re-classify based on current state (no parent in tree -> Unknown)
        // but original comm is preserved
        assert_eq!(tree.get(100).unwrap().comm, "cosh");
        // Re-classify: no tracked parent, matches_agent=false, has_agent_mode=false -> Unknown
        assert_eq!(pt, ProcessType::Unknown);
    }

    #[test]
    fn test_insert_and_classify_subagent_under_agent() {
        let mut tree = LineageTree::new();
        tree.insert_and_classify(100, 1, "cosh", Some("Cosh".to_string()), false, true);
        let pt =
            tree.insert_and_classify(200, 100, "sub-agent", Some("Sub".to_string()), false, true);
        assert_eq!(pt, ProcessType::SubAgent);
    }

    #[test]
    fn test_record_exec_classifies_root_agent_and_stamps_time() {
        let mut tree = LineageTree::new();
        let pt = tree.record_exec(100, 1, "cosh", Some("Cosh".to_string()), 42);
        assert_eq!(pt, ProcessType::Agent);
        assert_eq!(tree.get(100).unwrap().create_time_ns, 42);
    }

    #[test]
    fn test_record_exec_tool_child_under_agent() {
        let mut tree = LineageTree::new();
        tree.record_exec(100, 1, "cosh", Some("Cosh".to_string()), 1);
        // Child with no agent_name under an Agent classifies as Tool.
        let pt = tree.record_exec(200, 100, "bash", None, 2);
        assert_eq!(pt, ProcessType::Tool);
    }

    /// AGENT_MODE precedence pin: `record_exec` must derive `matches_agent` as
    /// false when the node already carries AGENT_MODE, so an inherited-env child
    /// with an agent_name still classifies as Tool — not promote to SubAgent.
    /// Discriminating: dropping the `!has_agent_mode` term in `record_exec`
    /// would flip this to SubAgent.
    #[test]
    fn test_record_exec_agent_mode_node_stays_tool() {
        let mut tree = LineageTree::new();
        tree.record_exec(100, 1, "cosh", Some("Cosh".to_string()), 1);
        // Seed an AGENT_MODE child node under the agent.
        tree.insert(LineageNode {
            pid: 200,
            ppid: 100,
            process_type: ProcessType::Unknown,
            flags: LINEAGE_FLAG_AGENT_MODE,
            create_time_ns: 0,
            comm: "child".to_string(),
            agent_name: None,
            children: vec![],
        });
        // Re-exec the AGENT_MODE node with an agent_name: matches_agent must be
        // suppressed by has_agent_mode, so it stays Tool (not SubAgent).
        let pt = tree.record_exec(200, 100, "child", Some("X".to_string()), 5);
        assert_eq!(pt, ProcessType::Tool);
    }

    #[test]
    fn test_children_at_exit_returns_children() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        tree.insert(make_node(300, 100, ProcessType::Tool));
        let mut orphans = tree.children_at_exit(100);
        orphans.sort();
        assert_eq!(orphans, vec![200, 300]);
    }

    #[test]
    fn test_children_at_exit_empty_for_leaf_and_missing() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        assert!(tree.children_at_exit(100).is_empty());
        assert!(tree.children_at_exit(999).is_empty());
    }

    // ─── classify_exit tests ─────────────────────────────────────────

    #[test]
    fn test_classify_exit_normal() {
        assert_eq!(classify_exit(0), ExitClassification::NormalExit);
    }

    #[test]
    fn test_classify_exit_abnormal_status() {
        assert_eq!(
            classify_exit(1 << 8),
            ExitClassification::AbnormalExit { status: 1 }
        );
        assert_eq!(
            classify_exit(2 << 8),
            ExitClassification::AbnormalExit { status: 2 }
        );
    }

    #[test]
    fn test_classify_exit_sigsegv() {
        let code = 11; // SIGSEGV, no coredump
        assert_eq!(
            classify_exit(code),
            ExitClassification::SignalCrash {
                signal: 11,
                coredump: false
            }
        );
    }

    #[test]
    fn test_classify_exit_sigsegv_with_coredump() {
        let code = 11 | 0x80; // SIGSEGV + coredump bit
        assert_eq!(
            classify_exit(code),
            ExitClassification::SignalCrash {
                signal: 11,
                coredump: true
            }
        );
    }

    #[test]
    fn test_classify_exit_sigkill() {
        assert_eq!(
            classify_exit(9),
            ExitClassification::SignalCrash {
                signal: 9,
                coredump: false
            }
        );
    }

    #[test]
    fn test_classify_exit_sigterm_graceful() {
        assert_eq!(
            classify_exit(15),
            ExitClassification::GracefulStop { signal: 15 }
        );
    }

    #[test]
    fn test_classify_exit_sigint_graceful() {
        assert_eq!(
            classify_exit(2),
            ExitClassification::GracefulStop { signal: 2 }
        );
    }

    #[test]
    fn test_classify_exit_sigpipe_benign() {
        assert_eq!(
            classify_exit(13),
            ExitClassification::BenignSignal { signal: 13 }
        );
    }

    #[test]
    fn test_classify_exit_sigabrt() {
        assert_eq!(
            classify_exit(6),
            ExitClassification::SignalCrash {
                signal: 6,
                coredump: false
            }
        );
    }

    #[test]
    fn test_is_crash() {
        assert!(
            ExitClassification::SignalCrash {
                signal: 11,
                coredump: false
            }
            .is_crash()
        );
        assert!(ExitClassification::AbnormalExit { status: 1 }.is_crash());
        assert!(!ExitClassification::NormalExit.is_crash());
        assert!(!ExitClassification::GracefulStop { signal: 15 }.is_crash());
        assert!(!ExitClassification::BenignSignal { signal: 13 }.is_crash());
    }

    #[test]
    fn test_classify_exit_sighup_graceful() {
        assert_eq!(
            classify_exit(1),
            ExitClassification::GracefulStop { signal: 1 }
        );
    }

    #[test]
    fn test_classify_exit_sigbus() {
        assert_eq!(
            classify_exit(7),
            ExitClassification::SignalCrash {
                signal: 7,
                coredump: false
            }
        );
    }

    #[test]
    fn test_classify_exit_max_status() {
        assert_eq!(
            classify_exit(255 << 8),
            ExitClassification::AbnormalExit { status: 255 }
        );
    }

    #[test]
    fn test_classify_exit_coredump_bit_without_signal() {
        // exit_code=0x80: coredump bit set but signal=0. Kernel won't produce
        // this, but classify_exit treats signal=0 as normal exit (coredump bit
        // is only meaningful with a terminating signal).
        assert_eq!(classify_exit(0x80), ExitClassification::NormalExit);
    }

    // ─── signal_name / format_crash_cause tests ──────────────────────

    #[test]
    fn test_signal_name_known() {
        assert_eq!(signal_name(11), "SIGSEGV");
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(6), "SIGABRT");
        assert_eq!(signal_name(15), "SIGTERM");
        assert_eq!(signal_name(2), "SIGINT");
        assert_eq!(signal_name(1), "SIGHUP");
        assert_eq!(signal_name(7), "SIGBUS");
        assert_eq!(signal_name(13), "SIGPIPE");
    }

    #[test]
    fn test_signal_name_unknown() {
        assert_eq!(signal_name(99), "SIG?");
        assert_eq!(signal_name(0), "SIG?");
    }

    #[test]
    fn test_format_crash_cause_sigsegv() {
        assert_eq!(
            format_crash_cause(11),
            "SIGSEGV (signal 11, coredump: false)"
        );
    }

    #[test]
    fn test_format_crash_cause_sigsegv_coredump() {
        assert_eq!(
            format_crash_cause(11 | 0x80),
            "SIGSEGV (signal 11, coredump: true)"
        );
    }

    #[test]
    fn test_format_crash_cause_abnormal_exit() {
        assert_eq!(format_crash_cause(1 << 8), "exit(1)");
    }

    #[test]
    fn test_format_crash_cause_normal_exit() {
        assert_eq!(format_crash_cause(0), "exit(0)");
    }

    #[test]
    fn test_format_crash_cause_sigterm_graceful() {
        assert_eq!(format_crash_cause(15), "SIGTERM (graceful)");
    }

    #[test]
    fn test_format_crash_cause_sigpipe_benign() {
        assert_eq!(format_crash_cause(13), "SIGPIPE (benign)");
    }

    // ─── format_crash_report / render_tree_to_string tests ────────────

    #[test]
    fn test_format_crash_report_sigsegv() {
        let detail = serde_json::json!({
            "signal": 11, "exit_code": 0, "core_dump": true, "oom": false,
            "process_type": "agent", "blast_radius": "total_session_loss",
            "call_ids": ["call-1", "call-2"],
            "children_at_exit": [200, 300],
        });
        let report = format_crash_report(&detail);
        assert!(report.contains("SIGSEGV"));
        assert!(report.contains("coredump: true"));
        assert!(report.contains("OOM Killed:   no"));
        assert!(report.contains("Process Type: agent"));
        assert!(report.contains("Blast Radius: total_session_loss"));
        assert!(report.contains("Pending LLM Calls (2)"));
        assert!(report.contains("call-1"));
        assert!(report.contains("Children at Exit: [200, 300]"));
    }

    #[test]
    fn test_format_crash_report_abnormal_exit() {
        let detail = serde_json::json!({
            "signal": 0, "exit_code": 1, "core_dump": false, "oom": false,
        });
        let report = format_crash_report(&detail);
        assert!(report.contains("exit(1)"));
        assert!(!report.contains("SIGSEGV"));
    }

    #[test]
    fn test_format_crash_report_with_process_tree() {
        let detail = serde_json::json!({
            "signal": 9, "exit_code": 0, "core_dump": false, "oom": true,
            "process_tree": {
                "pid": 100, "comm": "claude", "process_type": "agent",
                "children": [
                    {"pid": 200, "comm": "bash", "process_type": "tool", "children": []}
                ]
            }
        });
        let report = format_crash_report(&detail);
        assert!(report.contains("SIGKILL"));
        assert!(report.contains("OOM Killed:   yes"));
        assert!(report.contains("claude [100] (agent)"));
        assert!(report.contains("bash [200] (tool)"));
    }

    #[test]
    fn test_render_tree_to_string_nested() {
        let tree = serde_json::json!({
            "pid": 1, "comm": "root", "process_type": "agent",
            "children": [
                {"pid": 2, "comm": "child1", "process_type": "tool", "children": []},
                {"pid": 3, "comm": "child2", "process_type": "sub_agent", "children": [
                    {"pid": 4, "comm": "grandchild", "process_type": "tool", "children": []}
                ]}
            ]
        });
        let mut out = String::new();
        render_tree_to_string(&tree, "", true, &mut out);
        assert!(out.contains("`-- root [1] (agent)"));
        assert!(out.contains("|-- child1 [2] (tool)"));
        assert!(out.contains("`-- child2 [3] (sub_agent)"));
        assert!(out.contains("`-- grandchild [4] (tool)"));
    }

    #[test]
    fn test_format_token_by_type_table() {
        let rows = vec![
            ("agent".to_string(), 10, 5000, 3000, 8000),
            ("tool".to_string(), 2, 200, 100, 300),
        ];
        let table = format_token_by_type_table(&rows, 24);
        assert!(table.contains("Token Usage by Process Type (last 24h)"));
        assert!(table.contains("agent"));
        assert!(table.contains("tool"));
        assert!(table.contains("8,000"));
        assert!(table.contains("TOTAL"));
    }

    #[test]
    fn test_format_token_by_type_json() {
        let rows = vec![("agent".to_string(), 5, 1000, 500, 1500)];
        let json = format_token_by_type_json(&rows);
        assert!(json.contains("\"process_type\": \"agent\""));
        assert!(json.contains("\"total_tokens\": 1500"));
    }

    // ─── root_agent_ancestor tests ───────────────────────────────────

    #[test]
    fn test_root_agent_ancestor_tool_under_agent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::Tool));
        let root = tree.root_agent_ancestor(200).unwrap();
        assert_eq!(root.pid, 100);
    }

    #[test]
    fn test_root_agent_ancestor_nested() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        tree.insert(make_node(200, 100, ProcessType::SubAgent));
        tree.insert(make_node(300, 200, ProcessType::Tool));
        let root = tree.root_agent_ancestor(300).unwrap();
        assert_eq!(root.pid, 100);
    }

    #[test]
    fn test_root_agent_ancestor_none_for_agent() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(100, 1, ProcessType::Agent));
        assert!(tree.root_agent_ancestor(100).is_none());
    }

    #[test]
    fn test_root_agent_ancestor_none_for_orphan() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(200, 999, ProcessType::Tool));
        assert!(tree.root_agent_ancestor(200).is_none());
    }

    #[test]
    fn test_root_agent_ancestor_none_for_missing() {
        let tree = LineageTree::new();
        assert!(tree.root_agent_ancestor(999).is_none());
    }

    #[test]
    fn test_process_type_as_str() {
        assert_eq!(ProcessType::Agent.as_str(), "agent");
        assert_eq!(ProcessType::SubAgent.as_str(), "sub_agent");
        assert_eq!(ProcessType::Tool.as_str(), "tool");
        assert_eq!(ProcessType::Unknown.as_str(), "unknown");
        assert_eq!(ProcessType::Skill.as_str(), "skill");
    }

    #[test]
    fn test_process_type_from_u32() {
        assert_eq!(ProcessType::from_u32(0), ProcessType::Unknown);
        assert_eq!(ProcessType::from_u32(1), ProcessType::Agent);
        assert_eq!(ProcessType::from_u32(2), ProcessType::SubAgent);
        assert_eq!(ProcessType::from_u32(3), ProcessType::Tool);
        assert_eq!(ProcessType::from_u32(4), ProcessType::Skill);
        assert_eq!(ProcessType::from_u32(99), ProcessType::Unknown);
    }

    #[test]
    fn test_process_type_as_u32_roundtrip() {
        for v in 0..=4 {
            assert_eq!(ProcessType::from_u32(v).as_u32(), v);
        }
    }

    #[test]
    fn test_format_crash_report_no_optional_fields() {
        let detail = serde_json::json!({
            "signal": 0, "exit_status": 0, "coredump": false, "oom": false,
        });
        let report = format_crash_report(&detail);
        assert!(report.contains("exit(0)"));
        assert!(!report.contains("Pending LLM Calls"));
        assert!(!report.contains("Children at Exit"));
        assert!(!report.contains("Process Tree"));
        assert!(!report.contains("Process Type"));
        assert!(!report.contains("Blast Radius"));
    }

    #[test]
    fn test_format_crash_report_oom() {
        let detail = serde_json::json!({
            "signal": 9, "exit_status": 0, "coredump": false, "oom": true,
            "process_type": "tool", "blast_radius": "recoverable",
        });
        let report = format_crash_report(&detail);
        assert!(report.contains("SIGKILL"));
        assert!(report.contains("OOM Killed:   yes"));
        assert!(report.contains("Process Type: tool"));
        assert!(report.contains("Blast Radius: recoverable"));
    }

    #[test]
    fn test_format_token_by_type_table_empty() {
        let rows: Vec<(String, i64, i64, i64, i64)> = vec![];
        let table = format_token_by_type_table(&rows, 12);
        assert!(table.contains("last 12h"));
        assert!(table.contains("TOTAL"));
    }

    #[test]
    fn test_insert_preserves_children_on_reinsert() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(1, 0, ProcessType::Agent));
        tree.insert(make_node(2, 1, ProcessType::Tool));
        assert_eq!(tree.get(1).unwrap().children, vec![2]);
        let mut replacement = make_node(1, 0, ProcessType::Agent);
        replacement.comm = "updated".to_string();
        tree.insert(replacement);
        assert_eq!(tree.get(1).unwrap().children, vec![2]);
        assert_eq!(tree.get(1).unwrap().comm, "updated");
    }

    #[test]
    fn test_snapshot_returns_all_nodes() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(1, 0, ProcessType::Agent));
        tree.insert(make_node(2, 1, ProcessType::Tool));
        tree.insert(make_node(3, 1, ProcessType::SubAgent));
        assert_eq!(tree.snapshot().len(), 3);
    }

    #[test]
    fn test_roots_returns_parentless_nodes() {
        let mut tree = LineageTree::new();
        tree.insert(make_node(1, 999, ProcessType::Agent));
        tree.insert(make_node(2, 1, ProcessType::Tool));
        let roots = tree.roots();
        assert_eq!(roots, vec![1]);
    }
}
