//! Attribute contracts shared by PIP, decision requests and verdicts:
//! provenance-graded context attributes and missing-context requirements
//! (§7.1, §7.3). Missing context is part of the decision, not an error.

use serde::{Deserialize, Serialize};

use crate::event::EventTrust;
use crate::primitives::Timestamp;

/// Single context attribute with its provenance assurance level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribute {
    /// Attribute path within the closed namespace set, e.g. `taint.labels`.
    pub path: String,
    /// Attribute value.
    pub value: serde_json::Value,
    /// Provenance assurance of the source (Table 4); gates `minAssurance`.
    pub assurance: EventTrust,
    /// When the value was fetched, for freshness-based caching.
    pub fetched_at: Timestamp,
}

/// A context attribute a rule needs but could not obtain at the required
/// assurance; listed in `Verdict::missing_context` and used as the trigger
/// data for STEP_UP and DEFER (§7.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttributeRequirement {
    /// Attribute path the rule references.
    pub path: String,
    /// Minimum provenance assurance the rule demands.
    pub min_assurance: EventTrust,
}
