//! Dependency graph resolution.

/// Placeholder for DAG-based dependency resolution.
/// Will use petgraph to build and topologically sort component dependencies.
pub struct DependencyGraph;

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DependencyGraph {
    /// Create an empty dependency graph until manifest dependency wiring
    /// is implemented.
    pub fn new() -> Self {
        Self
    }

    /// Build a graph from registered manifests and return install order.
    pub fn resolve_order(&self, _targets: &[&str]) -> Vec<String> {
        // TODO(owner: core-planner, when: multi-component ordering ships):
        // build a DAG from manifest dependencies and topologically sort it.
        Vec::new()
    }
}
