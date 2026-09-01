//! Product-facing phase-one policy templates.

use asc_policy_types::identifiers::{PolicyId, Revision};
use serde::{Deserialize, Serialize};

/// Immutable product template envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateEnvelope {
    /// PAP-assigned stable policy identity retained by the lowered IR.
    pub policy_id: PolicyId,
    /// PAP-assigned immutable template and policy revision.
    pub revision: Revision,
    /// Product-level intent lowered by this crate.
    pub template: PolicyTemplate,
}

/// Minimal phase-one product policy vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PolicyTemplate {
    /// High-sensitivity files must not be read into Agent context.
    HighSensitivityReadDeny {
        /// Absolute file paths or bounded glob patterns selected by the user.
        files: Vec<String>,
    },
    /// Protected files must not be removed or renamed out of the namespace.
    PreventFileDeletion {
        /// Absolute file paths or bounded glob patterns selected by the user.
        files: Vec<String>,
    },
    /// Low-sensitivity data may be read but direct flow to untrusted endpoints is denied.
    LowSensitivityEgress {
        /// Low-sensitivity paths whose direct flow is tracked.
        files: Vec<String>,
        /// Destinations excluded from the deny rule.
        trusted_destinations: Vec<TrustedDestination>,
    },
}

/// Product-level trusted egress destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TrustedDestination {
    /// Lowercase DNS name or `*.` suffix pattern.
    Host {
        /// Host pattern selected by the user.
        pattern: String,
        /// Allowed non-zero destination ports.
        ports: Vec<u16>,
    },
    /// Canonical IP network.
    Cidr {
        /// Canonical network and prefix.
        cidr: String,
        /// Allowed non-zero destination ports.
        ports: Vec<u16>,
    },
}
