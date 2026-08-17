// SPDX-License-Identifier: Apache-2.0
//! Pure checkpoint records and manifest validation.
//!
//! This module deliberately contains no filesystem or path handling. The
//! daemon owns checkpoint persistence, hashing, publication, and cleanup.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::backend::{BackendKind, SnapshotKind};

/// Current on-disk checkpoint metadata format.
pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;

/// Self-contained artifacts required for every committed checkpoint.
pub const REQUIRED_ARTIFACTS: [&str; 3] = ["vmstate.snap", "memory.snap", "rootfs.snap"];

/// One content digest recorded in a checkpoint manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointArtifact {
    /// File name relative to the checkpoint directory.
    pub name: String,
    /// Logical file size in bytes.
    pub size_bytes: u64,
    /// Lowercase SHA-256 digest.
    pub sha256: String,
}

/// Durable checkpoint identity and integrity manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Metadata schema version.
    pub format_version: u32,
    /// Stable `ckpt-<uuid>` identifier.
    pub id: String,
    /// Previous checkpoint on this branch.
    #[serde(default)]
    pub parent: Option<String>,
    /// Sandbox that owns the checkpoint.
    pub sandbox_id: Uuid,
    /// Policy that selected the captured runtime.
    pub policy_name: String,
    /// Image identity selected by the policy.
    pub image_digest: String,
    /// Backend that produced the runtime artifacts.
    pub backend: BackendKind,
    /// Backend version captured by the daemon, when available.
    #[serde(default)]
    pub backend_version: Option<String>,
    /// UTC publication time.
    pub created_at: DateTime<Utc>,
    /// Backend snapshot semantics.
    pub snapshot_kind: SnapshotKind,
    /// Integrity records for all required artifacts.
    pub artifacts: Vec<CheckpointArtifact>,
}

/// Read-only API view of a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInfo {
    /// Checkpoint identifier.
    pub id: String,
    /// Parent checkpoint.
    pub parent: Option<String>,
    /// Publication time.
    pub created_at: DateTime<Utc>,
    /// Sum of logical artifact sizes.
    pub size_bytes: u64,
    /// Whether this checkpoint is the current HEAD.
    pub is_head: bool,
    /// Whether this checkpoint is reachable from HEAD.
    pub on_head_chain: bool,
}

/// Values supplied by the daemon when publishing a populated stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitCheckpoint {
    /// Parent checkpoint, if the sandbox already has a HEAD.
    pub parent: Option<String>,
    /// Policy that selected the captured runtime.
    pub policy_name: String,
    /// Image identity selected by the policy.
    pub image_digest: String,
    /// Backend that produced the artifacts.
    pub backend: BackendKind,
    /// Backend version captured by the caller, when available.
    pub backend_version: Option<String>,
    /// Backend snapshot semantics.
    pub snapshot_kind: SnapshotKind,
}

/// Pure validation failure for a checkpoint identifier or manifest.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CheckpointValidationError {
    /// A checkpoint identifier is not canonical `ckpt-<hyphenated-uuid>`.
    #[error("invalid checkpoint identifier {checkpoint_id:?}: {reason}")]
    InvalidIdentifier {
        checkpoint_id: String,
        reason: String,
    },

    /// A manifest uses a schema version this daemon cannot interpret.
    #[error("checkpoint {checkpoint_id} uses unsupported format {actual}; expected {expected}")]
    UnsupportedFormat {
        checkpoint_id: String,
        actual: u32,
        expected: u32,
    },

    /// The durable identity does not agree with the catalog location.
    #[error("checkpoint manifest identity mismatch: {reason}")]
    IdentityMismatch { reason: String },

    /// A manifest field needed to identify or reproduce a capture is empty.
    #[error("checkpoint {checkpoint_id} has invalid {field}: {reason}")]
    InvalidField {
        checkpoint_id: String,
        field: &'static str,
        reason: String,
    },

    /// The artifact manifest is incomplete, duplicated, or malformed.
    #[error("checkpoint {checkpoint_id} has an invalid artifact manifest: {reason}")]
    InvalidArtifacts {
        checkpoint_id: String,
        reason: String,
    },

    /// A requested artifact name is outside the frozen format.
    #[error("artifact name {name:?} is not part of the checkpoint format")]
    InvalidArtifactName { name: String },
}

/// Validate a canonical `ckpt-<hyphenated-uuid>` identifier.
pub fn validate_checkpoint_id(checkpoint_id: &str) -> Result<Uuid, CheckpointValidationError> {
    let raw = checkpoint_id
        .strip_prefix("ckpt-")
        .ok_or_else(|| invalid_identifier(checkpoint_id, "missing ckpt- prefix"))?;
    let uuid = Uuid::parse_str(raw)
        .map_err(|error| invalid_identifier(checkpoint_id, error.to_string()))?;
    if checkpoint_id != format!("ckpt-{uuid}") {
        return Err(invalid_identifier(
            checkpoint_id,
            "identifier is not in canonical hyphenated lowercase form",
        ));
    }
    Ok(uuid)
}

/// Validate a name before the daemon resolves it inside a checkpoint stage.
pub fn validate_artifact_name(name: &str) -> Result<(), CheckpointValidationError> {
    if REQUIRED_ARTIFACTS.contains(&name) {
        Ok(())
    } else {
        Err(CheckpointValidationError::InvalidArtifactName {
            name: name.to_string(),
        })
    }
}

/// Validate daemon-supplied values before constructing a durable manifest.
pub fn validate_commit_checkpoint(
    checkpoint_id: &str,
    input: &CommitCheckpoint,
) -> Result<(), CheckpointValidationError> {
    validate_checkpoint_id(checkpoint_id)?;
    if let Some(parent) = &input.parent {
        validate_checkpoint_id(parent)?;
        if parent == checkpoint_id {
            return Err(CheckpointValidationError::InvalidField {
                checkpoint_id: checkpoint_id.to_string(),
                field: "parent",
                reason: "a checkpoint cannot be its own parent".to_string(),
            });
        }
    }
    validate_runtime_identity(
        checkpoint_id,
        &input.policy_name,
        &input.image_digest,
        input.backend_version.as_deref(),
    )
}

/// Validate a parsed manifest against the catalog location that contained it.
///
/// Artifact content hashes are intentionally not checked here: reading files
/// belongs to the daemon. This function only validates the pure record.
pub fn validate_checkpoint_manifest(
    metadata: &CheckpointMetadata,
    expected_sandbox_id: Uuid,
    expected_checkpoint_id: &str,
) -> Result<(), CheckpointValidationError> {
    validate_checkpoint_id(expected_checkpoint_id)?;
    validate_checkpoint_id(&metadata.id)?;
    if metadata.format_version != CHECKPOINT_FORMAT_VERSION {
        return Err(CheckpointValidationError::UnsupportedFormat {
            checkpoint_id: metadata.id.clone(),
            actual: metadata.format_version,
            expected: CHECKPOINT_FORMAT_VERSION,
        });
    }
    if metadata.id != expected_checkpoint_id {
        return Err(CheckpointValidationError::IdentityMismatch {
            reason: format!(
                "manifest id {:?} does not match catalog id {expected_checkpoint_id:?}",
                metadata.id
            ),
        });
    }
    if metadata.sandbox_id != expected_sandbox_id {
        return Err(CheckpointValidationError::IdentityMismatch {
            reason: format!(
                "manifest sandbox {} does not match catalog sandbox {expected_sandbox_id}",
                metadata.sandbox_id
            ),
        });
    }
    if let Some(parent) = &metadata.parent {
        validate_checkpoint_id(parent)?;
        if parent == &metadata.id {
            return Err(CheckpointValidationError::InvalidField {
                checkpoint_id: metadata.id.clone(),
                field: "parent",
                reason: "a checkpoint cannot be its own parent".to_string(),
            });
        }
    }
    validate_runtime_identity(
        &metadata.id,
        &metadata.policy_name,
        &metadata.image_digest,
        metadata.backend_version.as_deref(),
    )?;
    validate_artifact_manifest(&metadata.id, &metadata.artifacts)
}

fn validate_runtime_identity(
    checkpoint_id: &str,
    policy_name: &str,
    image_digest: &str,
    backend_version: Option<&str>,
) -> Result<(), CheckpointValidationError> {
    if policy_name.trim().is_empty() {
        return Err(CheckpointValidationError::InvalidField {
            checkpoint_id: checkpoint_id.to_string(),
            field: "policy_name",
            reason: "value is empty".to_string(),
        });
    }
    if image_digest.trim().is_empty() {
        return Err(CheckpointValidationError::InvalidField {
            checkpoint_id: checkpoint_id.to_string(),
            field: "image_digest",
            reason: "value is empty".to_string(),
        });
    }
    if backend_version.is_some_and(|version| version.trim().is_empty()) {
        return Err(CheckpointValidationError::InvalidField {
            checkpoint_id: checkpoint_id.to_string(),
            field: "backend_version",
            reason: "present version is empty".to_string(),
        });
    }
    Ok(())
}

fn validate_artifact_manifest(
    checkpoint_id: &str,
    artifacts: &[CheckpointArtifact],
) -> Result<(), CheckpointValidationError> {
    if artifacts.len() != REQUIRED_ARTIFACTS.len() {
        return Err(invalid_artifacts(
            checkpoint_id,
            format!(
                "expected {} artifacts, found {}",
                REQUIRED_ARTIFACTS.len(),
                artifacts.len()
            ),
        ));
    }

    let mut names = HashSet::with_capacity(artifacts.len());
    for artifact in artifacts {
        validate_artifact_name(&artifact.name).map_err(|_| {
            invalid_artifacts(
                checkpoint_id,
                format!("unexpected artifact {:?}", artifact.name),
            )
        })?;
        if !names.insert(artifact.name.as_str()) {
            return Err(invalid_artifacts(
                checkpoint_id,
                format!("duplicate artifact {:?}", artifact.name),
            ));
        }
        if artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_artifacts(
                checkpoint_id,
                format!("artifact {:?} has an invalid SHA-256 digest", artifact.name),
            ));
        }
    }
    if REQUIRED_ARTIFACTS
        .iter()
        .any(|required| !names.contains(required))
    {
        return Err(invalid_artifacts(
            checkpoint_id,
            "one or more required artifacts are missing",
        ));
    }
    Ok(())
}

fn invalid_identifier(checkpoint_id: &str, reason: impl Into<String>) -> CheckpointValidationError {
    CheckpointValidationError::InvalidIdentifier {
        checkpoint_id: checkpoint_id.to_string(),
        reason: reason.into(),
    }
}

fn invalid_artifacts(checkpoint_id: &str, reason: impl Into<String>) -> CheckpointValidationError {
    CheckpointValidationError::InvalidArtifacts {
        checkpoint_id: checkpoint_id.to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(name: &str, fill: char) -> CheckpointArtifact {
        CheckpointArtifact {
            name: name.to_string(),
            size_bytes: 10,
            sha256: std::iter::repeat_n(fill, 64).collect(),
        }
    }

    fn metadata() -> CheckpointMetadata {
        let sandbox_id = Uuid::new_v4();
        CheckpointMetadata {
            format_version: CHECKPOINT_FORMAT_VERSION,
            id: format!("ckpt-{}", Uuid::new_v4()),
            parent: None,
            sandbox_id,
            policy_name: "default".to_string(),
            image_digest: "sha256:image".to_string(),
            backend: BackendKind::Mock,
            backend_version: Some("mock-v1".to_string()),
            created_at: Utc::now(),
            snapshot_kind: SnapshotKind::Full,
            artifacts: vec![
                artifact("vmstate.snap", 'a'),
                artifact("memory.snap", 'b'),
                artifact("rootfs.snap", 'c'),
            ],
        }
    }

    #[test]
    fn canonical_identifier_round_trips() {
        let uuid = Uuid::new_v4();
        assert_eq!(
            validate_checkpoint_id(&format!("ckpt-{uuid}")).expect("valid identifier"),
            uuid
        );
    }

    #[test]
    fn noncanonical_identifier_is_rejected() {
        let uuid = Uuid::new_v4();
        assert!(
            validate_checkpoint_id(&format!("ckpt-{}", uuid.to_string().to_uppercase())).is_err()
        );
        assert!(validate_checkpoint_id(&format!("ckpt-{}", uuid.simple())).is_err());
        assert!(validate_checkpoint_id("../checkpoint").is_err());
    }

    #[test]
    fn valid_manifest_passes_pure_validation() {
        let metadata = metadata();
        validate_checkpoint_manifest(&metadata, metadata.sandbox_id, &metadata.id)
            .expect("valid manifest");
    }

    #[test]
    fn manifest_identity_must_match_catalog_location() {
        let metadata = metadata();
        let error = validate_checkpoint_manifest(&metadata, Uuid::new_v4(), &metadata.id)
            .expect_err("sandbox mismatch must fail");
        assert!(matches!(
            error,
            CheckpointValidationError::IdentityMismatch { .. }
        ));
    }

    #[test]
    fn manifest_requires_the_exact_artifact_set() {
        let mut metadata = metadata();
        metadata.artifacts[2].name = "memory.snap".to_string();
        let error = validate_checkpoint_manifest(&metadata, metadata.sandbox_id, &metadata.id)
            .expect_err("duplicate artifact must fail");
        assert!(matches!(
            error,
            CheckpointValidationError::InvalidArtifacts { .. }
        ));
    }

    #[test]
    fn manifest_rejects_noncanonical_digest() {
        let mut metadata = metadata();
        metadata.artifacts[0].sha256 = "A".repeat(64);
        assert!(
            validate_checkpoint_manifest(&metadata, metadata.sandbox_id, &metadata.id).is_err()
        );
    }

    #[test]
    fn commit_input_rejects_self_parent_and_empty_identity() {
        let id = format!("ckpt-{}", Uuid::new_v4());
        let input = CommitCheckpoint {
            parent: Some(id.clone()),
            policy_name: String::new(),
            image_digest: String::new(),
            backend: BackendKind::Mock,
            backend_version: None,
            snapshot_kind: SnapshotKind::Full,
        };
        assert!(validate_commit_checkpoint(&id, &input).is_err());
    }
}
