//! Tier decomposition (§5.3): annotates predicates as Tier A (kernel plan),
//! Tier B (local synchronous plan) or Tier C (slow detector plan) by
//! decidability and cost. Predicates that cannot be lowered to a target
//! enforcement plane are reported explicitly, never silently relaxed.

use serde::{Deserialize, Serialize};

/// Execution tier of a compiled rule or predicate (Table 3). A rule's tier
/// is that of its slowest predicate; KernelRule obligations always emit an
/// additional Tier A entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    /// Kernel plan: bounded predicates only, enforced by OS Harness/sandbox.
    A,
    /// Local semantic plan: synchronous in-process evaluation.
    B,
    /// Slow analysis plan: async detectors; may never produce allow.
    C,
}
