use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Policy Template put params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutPolicyParams {
    /// Stable product identity. Omit it to create a new Policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<Uuid>,
    /// Human-readable policy name; it is not a lookup key.
    pub policy_name: String,
    /// Complete desired product-level template.
    pub template: PolicyTemplateDto,
}

/// Phase-one product policy vocabulary, duplicated at the wire boundary deliberately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PolicyTemplateDto {
    /// Deny reads from high-sensitivity files.
    HighSensitivityReadDeny {
        /// Absolute paths or bounded globs.
        files: Vec<String>,
    },
    /// Deny deletion or namespace removal of protected files.
    PreventFileDeletion {
        /// Absolute paths or bounded globs.
        files: Vec<String>,
    },
    /// Deny direct flow to destinations outside the trusted set.
    LowSensitivityEgress {
        /// Low-sensitivity file paths.
        files: Vec<String>,
        /// Trusted egress destinations.
        trusted_destinations: Vec<TrustedDestinationDto>,
    },
}

/// Product-level trusted destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TrustedDestinationDto {
    /// DNS host pattern.
    Host {
        /// Exact host or bounded wildcard.
        pattern: String,
        /// Allowed destination ports.
        ports: Vec<u16>,
    },
    /// Canonical IP network.
    Cidr {
        /// CIDR notation.
        cidr: String,
        /// Allowed destination ports.
        ports: Vec<u16>,
    },
}
