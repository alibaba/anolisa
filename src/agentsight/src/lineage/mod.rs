//! Blood lineage tree for AI agent process tracking
//!
//! Maintains a userspace mirror of the BPF lineage_tree map, enriched with
//! process type classification (Agent / SubAgent / Tool / Skill).

use std::collections::HashMap;

use serde::Serialize;

/// Process type classification for lineage tree nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessType {
    Unknown,
    Agent,
    SubAgent,
    Tool,
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
pub struct LineageTree {
    nodes: HashMap<u32, LineageNode>,
}

impl LineageTree {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Insert or update a node. Automatically maintains parent→child links.
    pub fn insert(&mut self, node: LineageNode) {
        let pid = node.pid;
        let ppid = node.ppid;

        // Add this pid as a child of its parent
        if let Some(parent) = self.nodes.get_mut(&ppid) {
            if !parent.children.contains(&pid) {
                parent.children.push(pid);
            }
        }

        self.nodes.insert(pid, node);
    }

    /// Remove a node and clean up parent→child links.
    pub fn remove(&mut self, pid: u32) -> Option<LineageNode> {
        let node = self.nodes.remove(&pid)?;

        // Remove from parent's children list
        if let Some(parent) = self.nodes.get_mut(&node.ppid) {
            parent.children.retain(|&c| c != pid);
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

    /// Classify a newly added process based on its ancestry and environment.
    ///
    /// Rules (evaluated in order):
    /// 1. Parent is Agent/SubAgent → child inherits lineage:
    ///    - matches agent pattern → SubAgent
    ///    - otherwise → Tool
    /// 2. No tracked parent + AGENT_MODE=1 in env → Agent (new root)
    /// 3. Otherwise → Unknown
    ///
    /// Note: child processes inherit AGENT_MODE=1 via environment, but that
    /// does NOT make them Agents — only top-level processes (without a tracked
    /// parent) are classified as Agent via AGENT_MODE.
    pub fn classify(
        &mut self,
        pid: u32,
        has_agent_mode_env: bool,
        matches_agent_pattern: bool,
    ) {
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
                if has_agent_mode_env {
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

    /// Snapshot the entire tree as a flat list (for REST API)
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
            comm: format!("proc-{}", pid),
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
}
