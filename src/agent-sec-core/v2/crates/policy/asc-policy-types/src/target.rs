//! Target Adapter descriptors, artifacts, and translation diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::identifiers::Digest;

/// Frozen PCP-to-Adapter contract version.
pub const TARGET_ADAPTER_CONTRACT_VERSION: u16 = 1;

/// Stable identity and artifact formats exposed by one Adapter implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetDescriptor {
    /// PCP-to-Adapter protocol version implemented by this Adapter.
    pub adapter_contract_version: u16,
    /// Stable PEP family used for Adapter and Client selection.
    pub pep_type: String,
    /// Structured provenance of the translator implementation.
    pub translator: TranslatorIdentity,
    /// Versioned target artifact contracts this Adapter may emit.
    pub artifact_contracts: Vec<TargetArtifactContract>,
}

/// Provenance of one target translator implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatorIdentity {
    /// `AgentSecCore` workspace version containing the translator.
    pub agent_sec_core_version: String,
    /// Monotonic Adapter-wide revision of translator semantics and compiler inputs.
    pub translation_revision: u64,
}

/// Versioned format and semantic contract of one target artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetArtifactContract {
    /// Stable contract identifier accepted by compatible target runtimes.
    pub contract_id: String,
    /// Media type of the opaque target plan bytes.
    pub media_type: String,
    /// Schema version encoded inside the target plan.
    pub schema_version: u16,
}

/// Result of translating one complete immutable Binding snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum TranslationOutcome {
    /// The Adapter produced a target plan that passed its static translation checks.
    Translated(TranslatedTargetBinding),
    /// The target deterministically cannot express the Binding safely.
    Rejected(TranslationRejection),
}

/// Target artifact accepted by the Adapter's phase-one translation contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatedTargetBinding {
    /// Opaque target-specific Binding plan.
    pub plan: TargetBindingPlan,
    /// Non-blocking information about the successful translation.
    pub diagnostics: Vec<Diagnostic>,
    /// Digest over the artifact contract and exact target plan bytes.
    pub plan_content_digest: Digest,
    /// Digest over source, translator, plan, and capability evidence.
    pub artifact_identity_digest: Digest,
    /// Target capabilities required before this plan may be dispatched.
    pub required_capabilities: RequiredCapabilities,
}

/// Opaque target-specific Binding payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetBindingPlan {
    /// Artifact contract governing the payload format and target semantics.
    pub artifact_contract_id: String,
    /// Versioned media type understood by the matching target Client.
    pub media_type: String,
    /// Exact bytes that are persisted before Client request preparation.
    pub content: Vec<u8>,
}

/// Stable machine-readable explanation attached to a translation decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    /// Stable code suitable for state projection and tests.
    pub code: String,
    /// Optional JSON-style source path.
    pub path: Option<String>,
    /// Safe developer-facing explanation.
    pub message: String,
}

/// Deterministic semantic rejection produced by a functioning Adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslationRejection {
    /// Stable summary code for state projection and tests.
    pub code: String,
    /// Detailed safe explanations of the rejection.
    pub diagnostics: Vec<Diagnostic>,
}

/// Versioned lower-case dotted target capability identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Creates a validated capability identifier.
    ///
    /// # Errors
    /// Returns an error unless the value is a versioned lower-case dotted key.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        Self::try_from(value.into())
    }

    /// Returns the stable wire value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 128 {
            return Err("capability ID must contain 1..=128 bytes".to_owned());
        }
        let mut segments = value.split('.');
        let version = segments.next_back().unwrap_or_default();
        let version_digits = version.strip_prefix('v').unwrap_or_default();
        if !value.contains('.')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
            || value.split('.').any(str::is_empty)
            || version_digits.is_empty()
            || !version_digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(
                "capability ID must contain lower-case dotted segments ending in v<digits>"
                    .to_owned(),
            );
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for CapabilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Capability features and numeric limits required by one target artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequiredCapabilities {
    /// Boolean features required by the plan.
    pub features: BTreeSet<CapabilityId>,
    /// Minimum numeric limits required by the plan.
    pub minimum_limits: BTreeMap<CapabilityId, u64>,
}

/// Internal Adapter failure distinct from a deterministic translation rejection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("target Adapter failed with code {code}")]
pub struct AdapterFault {
    /// Stable internal failure code.
    pub code: String,
    /// Safe diagnostic context.
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(hex_digit: char) -> Digest {
        Digest::new(format!("sha256:{}", hex_digit.to_string().repeat(64))).unwrap()
    }

    fn translated_outcome() -> TranslationOutcome {
        TranslationOutcome::Translated(TranslatedTargetBinding {
            plan: TargetBindingPlan {
                artifact_contract_id: "agentsight.binding.v1".to_owned(),
                media_type: "application/vnd.agentseccore.agentsight-binding.v1+json".to_owned(),
                content: vec![1, 2, 3],
            },
            diagnostics: vec![Diagnostic {
                code: "TRANSLATION_NOTE".to_owned(),
                path: Some("policy".to_owned()),
                message: "target rules were generated deterministically".to_owned(),
            }],
            plan_content_digest: digest('a'),
            artifact_identity_digest: digest('b'),
            required_capabilities: RequiredCapabilities {
                features: BTreeSet::from([CapabilityId::new("policy.file.delete.v1").unwrap()]),
                minimum_limits: BTreeMap::from([(
                    CapabilityId::new("policy.rules.max.v1").unwrap(),
                    1,
                )]),
            },
        })
    }

    #[test]
    fn translation_outcome_separates_target_artifact_from_rejection_details() {
        let translated = translated_outcome();
        let translated_json = serde_json::to_value(&translated).unwrap();
        assert_eq!(translated_json["status"], "translated");
        assert_eq!(
            translated_json["result"]["plan"]["content"],
            serde_json::json!([1, 2, 3])
        );
        for removed_field in [
            "mappingReport",
            "guarantees",
            "overallRelation",
            "mappingDigest",
        ] {
            assert!(
                translated_json["result"].get(removed_field).is_none(),
                "serialized removed field {removed_field}"
            );
        }
        assert_eq!(
            serde_json::from_value::<TranslationOutcome>(translated_json).unwrap(),
            translated
        );

        let rejected = TranslationOutcome::Rejected(TranslationRejection {
            code: "UNSUPPORTED_SCOPE".to_owned(),
            diagnostics: vec![Diagnostic {
                code: "UNSUPPORTED_SCOPE_SELECTOR".to_owned(),
                path: Some("scope.selector".to_owned()),
                message: "the target cannot express this Scope selector".to_owned(),
            }],
        });
        let rejected_json = serde_json::to_value(&rejected).unwrap();
        assert_eq!(rejected_json["status"], "rejected");
        assert_eq!(rejected_json["result"]["code"], "UNSUPPORTED_SCOPE");
        assert!(rejected_json["result"].get("plan").is_none());
        assert_eq!(
            serde_json::from_value::<TranslationOutcome>(rejected_json).unwrap(),
            rejected
        );
    }

    #[test]
    fn removed_mapping_fields_are_rejected_on_the_wire() {
        for removed_field in [
            "mappingReport",
            "guarantees",
            "overallRelation",
            "mappingDigest",
        ] {
            let mut translated_json = serde_json::to_value(translated_outcome()).unwrap();
            translated_json["result"][removed_field] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<TranslationOutcome>(translated_json).is_err(),
                "accepted removed field {removed_field}"
            );
        }
    }

    #[test]
    fn capability_ids_require_stable_dotted_keys() {
        assert!(CapabilityId::new("policy.file.delete.v1").is_ok());
        assert!(CapabilityId::new("Policy.File.Delete").is_err());
        assert!(CapabilityId::new("missing-version-shape").is_err());
        assert!(CapabilityId::new("policy..delete").is_err());
        assert!(CapabilityId::new("policy.file.delete.latest").is_err());
    }
}
