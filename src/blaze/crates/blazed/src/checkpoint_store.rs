// SPDX-License-Identifier: Apache-2.0
//! Filesystem-backed checkpoint catalog owned by the daemon.
//!
//! The catalog, sandbox directories, staging directories, committed
//! checkpoints, and artifacts are opened relative to retained directory
//! descriptors. Configured pathnames are retained only for diagnostics.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use blaze_core::checkpoint::{
    CHECKPOINT_FORMAT_VERSION, CheckpointArtifact, CheckpointInfo, CheckpointMetadata,
    CheckpointValidationError, CommitCheckpoint, REQUIRED_ARTIFACTS, validate_artifact_name,
    validate_checkpoint_id, validate_checkpoint_manifest, validate_commit_checkpoint,
};
use chrono::Utc;
use rustix::fs::{
    AtFlags, Dir, Mode, OFlags, RenameFlags, fchmod, fstat, fsync, mkdirat, openat, renameat,
    renameat_with, unlinkat,
};
use rustix::io::Errno;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::error::BlazeDaemonError;
use crate::state_store::{OwnedStateDirectory, StateStore};

const METADATA_FILE: &str = "metadata.json";
const HEAD_FILE: &str = "HEAD";
const STAGING_SUFFIX: &str = ".tmp";
const TOMBSTONE_SUFFIX: &str = ".tombstone";
const ABORT_TOMBSTONE_PREFIX: &str = ".abort.";
const CHECKPOINT_DIRECTORY_MODE: Mode = Mode::RWXU;
const CHECKPOINT_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);

/// Failure while reading or mutating the daemon checkpoint catalog.
#[derive(Debug, Error)]
pub enum CheckpointStoreError {
    /// A checkpoint record failed pure model validation.
    #[error(transparent)]
    Validation(#[from] CheckpointValidationError),

    /// Opening the namespace through the retained state root failed.
    #[error("checkpoint catalog state-root operation failed: {0}")]
    State(#[source] BlazeDaemonError),

    /// A catalog filesystem operation failed.
    #[error("checkpoint catalog {operation} failed for {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A metadata file could not be encoded or decoded.
    #[error("checkpoint metadata at {} is invalid: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The catalog layout violates an invariant required for safe mutation.
    #[error("checkpoint catalog invariant failed: {0}")]
    Invariant(String),
}

/// Convenient result type for checkpoint catalog operations.
pub type Result<T> = std::result::Result<T, CheckpointStoreError>;

/// Failure while creating a checkpoint stage.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct CheckpointBeginError {
    recovery_checkpoint_id: Option<String>,
    #[source]
    source: Box<CheckpointStoreError>,
}

impl CheckpointBeginError {
    /// Return the checkpoint whose stage cleanup could not be confirmed.
    pub fn recovery_checkpoint_id(&self) -> Option<&str> {
        self.recovery_checkpoint_id.as_deref()
    }

    fn recovery_required(checkpoint_id: String, source: CheckpointStoreError) -> Self {
        Self {
            recovery_checkpoint_id: Some(checkpoint_id),
            source: Box::new(source),
        }
    }
}

impl From<CheckpointStoreError> for CheckpointBeginError {
    fn from(source: CheckpointStoreError) -> Self {
        Self {
            recovery_checkpoint_id: None,
            source: Box::new(source),
        }
    }
}

/// Result of creating a checkpoint stage.
pub type BeginResult<T> = std::result::Result<T, CheckpointBeginError>;

/// Namespace outcome reported when checkpoint publication fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointPublishOutcome {
    /// The staging directory is known not to have been renamed.
    KnownUnpublished,
    /// The staging-to-catalog rename may have completed.
    Unknown,
}

/// Checkpoint publication failure with its observable namespace outcome.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct CheckpointPublishError {
    outcome: CheckpointPublishOutcome,
    #[source]
    source: CheckpointStoreError,
}

impl CheckpointPublishError {
    /// Return the strongest namespace outcome known at the failure boundary.
    pub fn outcome(&self) -> CheckpointPublishOutcome {
        self.outcome
    }

    /// Return the underlying catalog error.
    pub fn into_store_error(self) -> CheckpointStoreError {
        self.source
    }

    fn known_unpublished(source: CheckpointStoreError) -> Self {
        Self {
            outcome: CheckpointPublishOutcome::KnownUnpublished,
            source,
        }
    }

    fn unknown(source: CheckpointStoreError) -> Self {
        Self {
            outcome: CheckpointPublishOutcome::Unknown,
            source,
        }
    }
}

/// Result of publishing a checkpoint staging directory.
pub type PublishResult<T> = std::result::Result<T, CheckpointPublishError>;

/// Namespace outcome reported when moving checkpoint HEAD fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointHeadOutcome {
    /// HEAD retains its previous value and temporary cleanup is durable.
    KnownUnchanged,
    /// HEAD replacement may have completed, or temporary cleanup is uncertain.
    Unknown,
}

/// HEAD-update failure with its observable namespace outcome.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct CheckpointHeadError {
    outcome: CheckpointHeadOutcome,
    #[source]
    source: CheckpointStoreError,
}

impl CheckpointHeadError {
    /// Return the strongest namespace outcome known at the failure boundary.
    pub fn outcome(&self) -> CheckpointHeadOutcome {
        self.outcome
    }

    /// Return the underlying catalog error.
    pub fn into_store_error(self) -> CheckpointStoreError {
        self.source
    }

    fn known_unchanged(source: CheckpointStoreError) -> Self {
        Self {
            outcome: CheckpointHeadOutcome::KnownUnchanged,
            source,
        }
    }

    fn unknown(source: CheckpointStoreError) -> Self {
        Self {
            outcome: CheckpointHeadOutcome::Unknown,
            source,
        }
    }
}

/// Result of atomically moving checkpoint HEAD.
pub type SetHeadResult<T> = std::result::Result<T, CheckpointHeadError>;

/// Temporary checkpoint directory populated before atomic publication.
#[derive(Debug)]
pub struct CheckpointStage {
    id: String,
    sandbox_id: Uuid,
    catalog: OwnedStateDirectory,
    sandbox: OwnedStateDirectory,
    directory: OwnedStateDirectory,
    staging_name: String,
}

impl CheckpointStage {
    /// Generated checkpoint identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Resolve one artifact through the retained stage directory.
    pub fn artifact_path(&self, name: &str) -> Result<PathBuf> {
        validate_artifact_name(name)?;
        Ok(self.directory.path().join(name))
    }
}

struct OwnedArtifact {
    path: PathBuf,
    file: File,
}

struct VerifiedCheckpoint {
    metadata: CheckpointMetadata,
    directory: OwnedStateDirectory,
    metadata_file: OwnedArtifact,
    artifacts: Vec<OwnedArtifact>,
}

/// Restore target retained through the complete replacement operation.
///
/// The catalog, sandbox, checkpoint directory, and artifact descriptors stay
/// open so path replacement cannot redirect either restore input or HEAD.
pub(crate) struct RestoreCheckpoint {
    catalog: OwnedStateDirectory,
    sandbox: OwnedStateDirectory,
    verified: VerifiedCheckpoint,
}

impl RestoreCheckpoint {
    pub(crate) fn metadata(&self) -> &CheckpointMetadata {
        &self.verified.metadata
    }

    pub(crate) fn artifact_path(&self, name: &str) -> Result<PathBuf> {
        validate_artifact_name(name)?;
        let index = REQUIRED_ARTIFACTS
            .iter()
            .position(|candidate| *candidate == name)
            .ok_or_else(|| invariant(format!("checkpoint has no required artifact {name}")))?;
        Ok(self.verified.artifacts[index].stable_path())
    }
}

struct LoadedCheckpointMetadata {
    metadata: CheckpointMetadata,
    directory: OwnedStateDirectory,
    metadata_file: OwnedArtifact,
    artifacts: Vec<OwnedArtifact>,
}

pub(crate) struct PublishedCheckpoint {
    catalog: OwnedStateDirectory,
    sandbox: OwnedStateDirectory,
    loaded: LoadedCheckpointMetadata,
}

impl PublishedCheckpoint {
    pub(crate) fn metadata(&self) -> &CheckpointMetadata {
        &self.loaded.metadata
    }

    fn require_linked(&self) -> Result<()> {
        let sandbox_name = self.loaded.metadata.sandbox_id.to_string();
        require_linked_directory(&self.catalog, &sandbox_name, &self.sandbox)?;
        self.loaded
            .require_linked(&self.sandbox, &self.loaded.metadata.id)
    }

    fn into_metadata(self) -> CheckpointMetadata {
        self.loaded.metadata
    }
}

impl LoadedCheckpointMetadata {
    fn require_linked(&self, sandbox: &OwnedStateDirectory, directory_name: &str) -> Result<()> {
        validate_exact_entries(
            &self.directory,
            &[
                REQUIRED_ARTIFACTS[0],
                REQUIRED_ARTIFACTS[1],
                REQUIRED_ARTIFACTS[2],
                METADATA_FILE,
            ],
        )?;
        require_linked_file(&self.directory, METADATA_FILE, &self.metadata_file)?;
        for (name, artifact) in REQUIRED_ARTIFACTS.iter().zip(&self.artifacts) {
            require_linked_file(&self.directory, name, artifact)?;
        }
        require_linked_directory(sandbox, directory_name, &self.directory)
    }
}

impl VerifiedCheckpoint {
    fn require_linked(&self, sandbox: &OwnedStateDirectory, checkpoint_id: &str) -> Result<()> {
        validate_exact_entries(
            &self.directory,
            &[
                REQUIRED_ARTIFACTS[0],
                REQUIRED_ARTIFACTS[1],
                REQUIRED_ARTIFACTS[2],
                METADATA_FILE,
            ],
        )?;
        require_linked_file(&self.directory, METADATA_FILE, &self.metadata_file)?;
        for (name, artifact) in REQUIRED_ARTIFACTS.iter().zip(&self.artifacts) {
            require_linked_file(&self.directory, name, artifact)?;
        }
        require_linked_directory(sandbox, checkpoint_id, &self.directory)
    }
}

impl OwnedArtifact {
    fn stable_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                self.file.as_raw_fd()
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.path.clone()
        }
    }
}

#[cfg(test)]
type BeforePublishRevalidation = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// Filesystem-backed checkpoint catalog.
#[derive(Clone)]
pub struct CheckpointStore {
    state_store: StateStore,
    root: Arc<Mutex<Option<OwnedStateDirectory>>>,
    #[cfg(test)]
    before_publish_revalidation: BeforePublishRevalidation,
    #[cfg(test)]
    verified_checkpoint_calls: Arc<AtomicUsize>,
}

impl std::fmt::Debug for CheckpointStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckpointStore")
            .field("state_store", &self.state_store)
            .finish_non_exhaustive()
    }
}

impl CheckpointStore {
    /// Bind the catalog to the daemon's retained state-root owner.
    pub fn new(state_store: StateStore) -> Self {
        Self {
            state_store,
            root: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            before_publish_revalidation: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            verified_checkpoint_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Create and durably expose a unique staging directory.
    pub fn begin(&self, sandbox_id: Uuid) -> BeginResult<CheckpointStage> {
        let catalog = self.root()?;
        let sandbox = self.ensure_sandbox_dir(&catalog, sandbox_id)?;
        loop {
            let id = format!("ckpt-{}", Uuid::new_v4());
            let staging_name = format!(".{id}{STAGING_SUFFIX}");
            let stage_path = sandbox.configured_path().join(&staging_name);
            match mkdirat(
                sandbox.descriptor(),
                staging_name.as_str(),
                CHECKPOINT_DIRECTORY_MODE,
            ) {
                Ok(()) => {}
                Err(Errno::EXIST) => continue,
                Err(source) => {
                    return Err(io_error(
                        "create checkpoint directory",
                        &stage_path,
                        std::io::Error::from(source),
                    )
                    .into());
                }
            }
            let directory =
                match checkpoint_store_failpoint("checkpoint-store-stage-open", &stage_path)
                    .and_then(|()| open_child_directory(&sandbox, &staging_name))
                {
                    Ok(directory) => directory,
                    Err(open_error) => {
                        let cleanup_result = checkpoint_store_failpoint(
                            "checkpoint-store-stage-open-cleanup-before-unlink",
                            &stage_path,
                        )
                        .and_then(|()| {
                            unlinkat(
                                sandbox.descriptor(),
                                staging_name.as_str(),
                                AtFlags::REMOVEDIR,
                            )
                            .map_err(|source| {
                                io_error(
                                    "remove unopened checkpoint stage",
                                    &stage_path,
                                    std::io::Error::from(source),
                                )
                            })
                        })
                        .and_then(|()| {
                            checkpoint_store_failpoint(
                                "checkpoint-store-stage-open-cleanup-parent-sync",
                                sandbox.configured_path(),
                            )
                            .and_then(|()| sync_directory(&sandbox))
                        });
                        return match cleanup_result {
                            Ok(()) => Err(open_error.into()),
                            Err(cleanup_error) => Err(CheckpointBeginError::recovery_required(
                                id,
                                invariant(format!(
                                    "checkpoint stage opening failed: {open_error}; \
                                 created-stage cleanup also failed: {cleanup_error}"
                                )),
                            )),
                        };
                    }
                };
            let sync_result = checkpoint_store_failpoint(
                "checkpoint-store-stage-parent-sync",
                sandbox.configured_path(),
            )
            .and_then(|()| sync_directory(&sandbox));
            if let Err(sync_error) = sync_result {
                return match self.abort_owned_stage(&sandbox, &staging_name, directory) {
                    Ok(()) => Err(sync_error.into()),
                    Err(cleanup_error) => Err(CheckpointBeginError::recovery_required(
                        id,
                        invariant(format!(
                            "checkpoint stage parent synchronization failed: \
                             {sync_error}; owned-stage cleanup also failed: \
                             {cleanup_error}"
                        )),
                    )),
                };
            }
            return Ok(CheckpointStage {
                id,
                sandbox_id,
                catalog,
                sandbox,
                directory,
                staging_name,
            });
        }
    }

    /// Hash, sync, and atomically publish a populated stage without moving HEAD.
    #[cfg(test)]
    pub fn publish(
        &self,
        stage: &CheckpointStage,
        input: CommitCheckpoint,
    ) -> PublishResult<CheckpointMetadata> {
        self.publish_retained(stage, input)
            .map(PublishedCheckpoint::into_metadata)
    }

    pub(crate) fn publish_retained(
        &self,
        stage: &CheckpointStage,
        input: CommitCheckpoint,
    ) -> PublishResult<PublishedCheckpoint> {
        let loaded = (|| -> Result<LoadedCheckpointMetadata> {
            self.validate_stage(stage)?;
            validate_commit_checkpoint(&stage.id, &input)?;
            if let Some(parent) = &input.parent {
                self.validated_chain_from(&stage.sandbox, stage.sandbox_id, parent)?;
            }
            if optional_child_directory(&stage.sandbox, &stage.id)?.is_some() {
                return Err(invariant(format!(
                    "checkpoint publication target {} already exists",
                    stage.sandbox.configured_path().join(&stage.id).display()
                )));
            }
            validate_exact_entries(&stage.directory, &REQUIRED_ARTIFACTS)?;

            let mut opened_artifacts = Vec::with_capacity(REQUIRED_ARTIFACTS.len());
            for name in REQUIRED_ARTIFACTS {
                let artifact =
                    open_required_file(&stage.directory, name, "open checkpoint artifact")?;
                validate_checkpoint_artifact_owner(&artifact)?;
                opened_artifacts.push((name, artifact));
            }

            let mut artifacts = Vec::with_capacity(REQUIRED_ARTIFACTS.len());
            for (name, artifact) in &mut opened_artifacts {
                fchmod(&artifact.file, CHECKPOINT_FILE_MODE).map_err(|source| {
                    io_error(
                        "restrict checkpoint artifact permissions",
                        &artifact.path,
                        std::io::Error::from(source),
                    )
                })?;
                artifact.file.sync_all().map_err(|source| {
                    io_error("sync checkpoint artifact", &artifact.path, source)
                })?;
                artifacts.push(hash_artifact(artifact, name)?);
            }

            let metadata = CheckpointMetadata {
                format_version: CHECKPOINT_FORMAT_VERSION,
                id: stage.id.clone(),
                parent: input.parent,
                sandbox_id: stage.sandbox_id,
                policy_name: input.policy_name,
                image_digest: input.image_digest,
                backend: input.backend,
                backend_version: input.backend_version,
                created_at: Utc::now(),
                snapshot_kind: input.snapshot_kind,
                artifacts,
            };
            validate_checkpoint_manifest(&metadata, stage.sandbox_id, &stage.id)?;
            let metadata_file = write_json_new(&stage.directory, METADATA_FILE, &metadata)?;
            sync_directory(&stage.directory)?;

            #[cfg(test)]
            self.run_before_publish_revalidation();

            let loaded = LoadedCheckpointMetadata {
                metadata,
                directory: stage.directory.clone(),
                metadata_file,
                artifacts: opened_artifacts
                    .into_iter()
                    .map(|(_, artifact)| artifact)
                    .collect(),
            };
            loaded.require_linked(&stage.sandbox, &stage.staging_name)?;
            checkpoint_store_failpoint(
                "checkpoint-store-publish-before-rename",
                &stage.sandbox.configured_path().join(&stage.staging_name),
            )?;
            Ok(loaded)
        })()
        .map_err(CheckpointPublishError::known_unpublished)?;

        renameat_with(
            stage.sandbox.descriptor(),
            stage.staging_name.as_str(),
            stage.sandbox.descriptor(),
            stage.id.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|source| {
            let source = std::io::Error::from(source);
            io_error(
                "publish checkpoint directory",
                stage.sandbox.configured_path().join(&stage.id),
                source,
            )
        })
        .map_err(CheckpointPublishError::unknown)?;
        checkpoint_store_failpoint(
            "checkpoint-store-publish-after-rename",
            &stage.sandbox.configured_path().join(&stage.id),
        )
        .map_err(CheckpointPublishError::unknown)?;
        loaded
            .require_linked(&stage.sandbox, &stage.id)
            .map_err(CheckpointPublishError::unknown)?;
        sync_directory(&stage.sandbox).map_err(CheckpointPublishError::unknown)?;
        Ok(PublishedCheckpoint {
            catalog: stage.catalog.clone(),
            sandbox: stage.sandbox.clone(),
            loaded,
        })
    }

    /// Remove an unpublished stage owned by this process.
    pub fn abort(&self, stage: CheckpointStage) -> Result<()> {
        self.validate_stage(&stage)?;
        self.abort_owned_stage(&stage.sandbox, &stage.staging_name, stage.directory)
    }

    /// Read and validate one committed checkpoint and all artifact hashes.
    #[cfg(test)]
    pub fn verify(&self, sandbox_id: Uuid, checkpoint_id: &str) -> Result<CheckpointMetadata> {
        let catalog = self.root()?;
        let sandbox = required_child_directory(
            &catalog,
            &sandbox_id.to_string(),
            "open checkpoint sandbox directory",
        )?;
        Ok(self
            .verified_checkpoint(&sandbox, sandbox_id, checkpoint_id)?
            .metadata)
    }

    /// Verify and retain a restore target and its complete ancestry.
    pub(crate) fn verify_restore_target(
        &self,
        sandbox_id: Uuid,
        checkpoint_id: &str,
    ) -> Result<RestoreCheckpoint> {
        let catalog = self.root()?;
        let sandbox_name = sandbox_id.to_string();
        let sandbox =
            required_child_directory(&catalog, &sandbox_name, "open checkpoint sandbox directory")?;
        self.validated_chain_from(&sandbox, sandbox_id, checkpoint_id)?;
        let verified = self.verified_checkpoint(&sandbox, sandbox_id, checkpoint_id)?;
        require_linked_directory(&catalog, &sandbox_name, &sandbox)?;
        Ok(RestoreCheckpoint {
            catalog,
            sandbox,
            verified,
        })
    }

    /// Atomically move HEAD to a restore target retained by this process.
    pub(crate) fn set_head_verified(&self, target: &RestoreCheckpoint) -> SetHeadResult<()> {
        let checkpoint_id = target.verified.metadata.id.clone();
        let sandbox_name = target.verified.metadata.sandbox_id.to_string();
        self.set_head_with_revalidation(&target.sandbox, &checkpoint_id, || {
            let root = self.root()?;
            if !same_directory(&root, &target.catalog)? {
                return Err(invariant(
                    "restore target belongs to a different checkpoint catalog root",
                ));
            }
            require_linked_directory(&target.catalog, &sandbox_name, &target.sandbox)?;
            target
                .verified
                .require_linked(&target.sandbox, &checkpoint_id)
        })
    }

    /// List committed checkpoints and mark the lineage reachable from HEAD.
    pub fn list(&self, sandbox_id: Uuid) -> Result<Vec<CheckpointInfo>> {
        let catalog_root = self.root()?;
        let Some(sandbox) = optional_child_directory(&catalog_root, &sandbox_id.to_string())?
        else {
            return Ok(Vec::new());
        };
        let catalog = self.load_catalog(&sandbox, sandbox_id)?;
        let head = self.read_head_id_from(&sandbox)?;
        let on_head_chain = match head.as_deref() {
            Some(head) => lineage_from(&catalog, head)?,
            None => HashSet::new(),
        };

        let mut checkpoints = Vec::with_capacity(catalog.len());
        for metadata in catalog.into_values() {
            let size_bytes = metadata
                .artifacts
                .iter()
                .try_fold(0_u64, |total, artifact| {
                    total.checked_add(artifact.size_bytes)
                })
                .ok_or_else(|| {
                    invariant(format!(
                        "checkpoint {} artifact sizes overflow u64",
                        metadata.id
                    ))
                })?;
            checkpoints.push(CheckpointInfo {
                id: metadata.id.clone(),
                parent: metadata.parent,
                created_at: metadata.created_at,
                size_bytes,
                is_head: head.as_deref() == Some(metadata.id.as_str()),
                on_head_chain: on_head_chain.contains(&metadata.id),
            });
        }
        checkpoints.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(checkpoints)
    }

    /// Atomically move HEAD to an already committed, verified checkpoint.
    #[cfg(test)]
    pub fn set_head(&self, sandbox_id: Uuid, checkpoint_id: &str) -> SetHeadResult<()> {
        let catalog = self.root().map_err(CheckpointHeadError::known_unchanged)?;
        let sandbox = required_child_directory(
            &catalog,
            &sandbox_id.to_string(),
            "open checkpoint sandbox directory",
        )
        .map_err(CheckpointHeadError::known_unchanged)?;
        let verified = self
            .verified_checkpoint(&sandbox, sandbox_id, checkpoint_id)
            .map_err(CheckpointHeadError::known_unchanged)?;
        let sandbox_name = sandbox_id.to_string();
        self.set_head_with_revalidation(&sandbox, checkpoint_id, || {
            require_linked_directory(&catalog, &sandbox_name, &sandbox)?;
            verified.require_linked(&sandbox, checkpoint_id)
        })
    }

    pub(crate) fn set_head_published(
        &self,
        published: PublishedCheckpoint,
    ) -> SetHeadResult<CheckpointMetadata> {
        let root = self.root().map_err(CheckpointHeadError::known_unchanged)?;
        let checkpoint_id = published.metadata().id.clone();
        self.set_head_with_revalidation(&published.sandbox, &checkpoint_id, || {
            if !same_directory(&root, &published.catalog)? {
                return Err(invariant(
                    "published checkpoint belongs to a different catalog root",
                ));
            }
            published.require_linked()
        })?;
        Ok(published.into_metadata())
    }

    fn set_head_with_revalidation<F>(
        &self,
        sandbox: &OwnedStateDirectory,
        checkpoint_id: &str,
        mut revalidate: F,
    ) -> SetHeadResult<()>
    where
        F: FnMut() -> Result<()>,
    {
        revalidate().map_err(CheckpointHeadError::known_unchanged)?;
        // Opening with O_NOFOLLOW performs the complete type validation needed
        // before atomic replacement. A missing HEAD is also valid.
        let _existing_head = optional_file(sandbox, HEAD_FILE, "inspect existing checkpoint HEAD")
            .map_err(CheckpointHeadError::known_unchanged)?;

        let temporary_name = format!(".HEAD.{}{STAGING_SUFFIX}", Uuid::new_v4());
        let mut temporary = create_new_file(sandbox, &temporary_name, "create temporary HEAD")
            .map_err(CheckpointHeadError::known_unchanged)?;
        let before_rename = (|| {
            write_all(
                &mut temporary.file,
                &temporary.path,
                checkpoint_id.as_bytes(),
            )?;
            write_all(&mut temporary.file, &temporary.path, b"\n")?;
            temporary
                .file
                .sync_all()
                .map_err(|source| io_error("sync temporary HEAD", &temporary.path, source))?;
            revalidate()?;
            checkpoint_store_failpoint("checkpoint-store-head-before-rename", &temporary.path)
        })();
        if let Err(source) = before_rename {
            let cleanup =
                checkpoint_store_failpoint("checkpoint-store-head-cleanup", &temporary.path)
                    .and_then(|()| remove_file_if_exists(sandbox, &temporary_name))
                    .and_then(|()| sync_directory(sandbox));
            return match cleanup {
                Ok(()) => Err(CheckpointHeadError::known_unchanged(source)),
                Err(cleanup) => Err(CheckpointHeadError::unknown(invariant(format!(
                    "{source}; temporary HEAD cleanup failed: {cleanup}"
                )))),
            };
        }

        renameat(
            sandbox.descriptor(),
            temporary_name.as_str(),
            sandbox.descriptor(),
            HEAD_FILE,
        )
        .map_err(|source| {
            io_error(
                "publish checkpoint HEAD",
                sandbox.configured_path().join(HEAD_FILE),
                std::io::Error::from(source),
            )
        })
        .map_err(CheckpointHeadError::unknown)?;
        checkpoint_store_failpoint(
            "checkpoint-store-head-after-rename",
            &sandbox.configured_path().join(HEAD_FILE),
        )
        .map_err(CheckpointHeadError::unknown)?;
        require_linked_file(sandbox, HEAD_FILE, &temporary)
            .map_err(CheckpointHeadError::unknown)?;
        sync_directory(sandbox).map_err(CheckpointHeadError::unknown)
    }

    /// Return the persisted HEAD, if present.
    pub fn read_head(&self, sandbox_id: Uuid) -> Result<Option<String>> {
        let catalog = self.root()?;
        let Some(sandbox) = optional_child_directory(&catalog, &sandbox_id.to_string())? else {
            return Ok(None);
        };
        self.read_head_from(&sandbox, sandbox_id)
    }

    /// Return the recorded HEAD identifier without verifying its artifacts.
    ///
    /// Callers that only need to report which checkpoint HEAD names must use
    /// this instead of [`Self::read_head`]. Hashing a complete checkpoint would
    /// make the observation cost proportional to the guest image size, and an
    /// unreadable artifact would replace the recorded identifier with an
    /// integrity error exactly when a caller needs the identifier to describe
    /// an interrupted operation.
    pub fn read_head_id(&self, sandbox_id: Uuid) -> Result<Option<String>> {
        let catalog = self.root()?;
        let Some(sandbox) = optional_child_directory(&catalog, &sandbox_id.to_string())? else {
            return Ok(None);
        };
        self.read_head_id_from(&sandbox)
    }

    /// Remove every checkpoint artifact owned by one sandbox.
    ///
    /// A missing sandbox directory is already clean. Any unexpected entry or
    /// changed identity fails closed so the lifecycle owner can retain a
    /// recoverable destroy record instead of deleting an unrelated object.
    pub fn remove_sandbox(&self, sandbox_id: Uuid) -> Result<()> {
        enum OwnedEntry {
            Directory(String, OwnedStateDirectory),
            File(String, OwnedArtifact),
        }

        let catalog = self.root()?;
        let name = sandbox_id.to_string();
        let Some(sandbox) = optional_child_directory(&catalog, &name)? else {
            checkpoint_store_failpoint(
                "checkpoint-store-sandbox-remove-parent-sync",
                catalog.configured_path(),
            )?;
            return sync_directory(&catalog);
        };
        let mut names: Vec<_> = directory_names(&sandbox, "scan sandbox checkpoint namespace")?
            .into_iter()
            .collect();
        names.sort();

        let mut entries = Vec::with_capacity(names.len());
        for entry in names {
            let kind = if entry == HEAD_FILE {
                ScratchKind::File
            } else if let Some(kind) = classify_scratch_name(&entry)? {
                kind
            } else {
                validate_checkpoint_id(&entry)?;
                ScratchKind::Directory
            };
            match kind {
                ScratchKind::Directory => entries.push(OwnedEntry::Directory(
                    entry.clone(),
                    required_child_directory(&sandbox, &entry, "open owned checkpoint directory")?,
                )),
                ScratchKind::File => entries.push(OwnedEntry::File(
                    entry.clone(),
                    open_required_file(&sandbox, &entry, "open owned checkpoint file")?,
                )),
            }
        }

        for entry in entries {
            match entry {
                OwnedEntry::Directory(name, directory) => {
                    remove_owned_directory(&sandbox, &name, directory)?;
                }
                OwnedEntry::File(name, file) => remove_owned_file(&sandbox, &name, file)?,
            }
        }
        sync_directory(&sandbox)?;
        require_linked_directory(&catalog, &name, &sandbox)?;
        checkpoint_store_failpoint(
            "checkpoint-store-sandbox-remove-before-unlink",
            sandbox.configured_path(),
        )?;
        unlinkat(catalog.descriptor(), name.as_str(), AtFlags::REMOVEDIR).map_err(|source| {
            io_error(
                "remove sandbox checkpoint namespace",
                catalog.configured_path().join(&name),
                std::io::Error::from(source),
            )
        })?;
        checkpoint_store_failpoint(
            "checkpoint-store-sandbox-remove-parent-sync",
            catalog.configured_path(),
        )?;
        sync_directory(&catalog)
    }

    fn root(&self) -> Result<OwnedStateDirectory> {
        let mut root = self
            .root
            .lock()
            .map_err(|_| invariant("checkpoint root owner lock poisoned"))?;
        if let Some(root) = root.as_ref() {
            return Ok(root.clone());
        }
        let opened = self
            .state_store
            .checkpoint_directory()
            .map_err(CheckpointStoreError::State)?;
        *root = Some(opened.clone());
        Ok(opened)
    }

    fn ensure_sandbox_dir(
        &self,
        catalog: &OwnedStateDirectory,
        sandbox_id: Uuid,
    ) -> Result<OwnedStateDirectory> {
        let name = sandbox_id.to_string();
        match create_child_directory(catalog, &name) {
            Ok(directory) => {
                checkpoint_store_failpoint(
                    "checkpoint-store-sandbox-parent-sync",
                    catalog.configured_path(),
                )?;
                sync_directory(catalog)?;
                Ok(directory)
            }
            Err(CheckpointStoreError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                let directory =
                    required_child_directory(catalog, &name, "open checkpoint sandbox directory")?;
                checkpoint_store_failpoint(
                    "checkpoint-store-sandbox-parent-sync",
                    catalog.configured_path(),
                )?;
                sync_directory(catalog)?;
                Ok(directory)
            }
            Err(error) => Err(error),
        }
    }

    fn validate_stage(&self, stage: &CheckpointStage) -> Result<()> {
        validate_checkpoint_id(&stage.id)?;
        let root = self.root()?;
        if !same_directory(&root, &stage.catalog)? {
            return Err(invariant("checkpoint stage belongs to a different catalog"));
        }
        require_linked_directory(&root, &stage.sandbox_id.to_string(), &stage.sandbox)?;
        require_linked_directory(&stage.sandbox, &stage.staging_name, &stage.directory)?;
        if optional_child_directory(&stage.sandbox, &stage.id)?.is_some() {
            return Err(invariant(format!(
                "checkpoint publication target {} already exists",
                stage.sandbox.configured_path().join(&stage.id).display()
            )));
        }
        Ok(())
    }

    fn abort_owned_stage(
        &self,
        sandbox: &OwnedStateDirectory,
        staging_name: &str,
        stage: OwnedStateDirectory,
    ) -> Result<()> {
        require_linked_directory(sandbox, staging_name, &stage)?;
        let checkpoint_id = staging_name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(STAGING_SUFFIX))
            .ok_or_else(|| invariant(format!("invalid staging name {staging_name:?}")))?;
        validate_checkpoint_id(checkpoint_id)?;
        let tombstone_name = format!(
            "{ABORT_TOMBSTONE_PREFIX}{checkpoint_id}.{}{TOMBSTONE_SUFFIX}",
            Uuid::new_v4()
        );
        checkpoint_store_failpoint(
            "checkpoint-store-abort-before-rename",
            &sandbox.configured_path().join(staging_name),
        )?;
        renameat_with(
            sandbox.descriptor(),
            staging_name,
            sandbox.descriptor(),
            tombstone_name.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|source| {
            io_error(
                "tombstone aborted checkpoint stage",
                sandbox.configured_path().join(&tombstone_name),
                std::io::Error::from(source),
            )
        })?;
        require_linked_directory(sandbox, &tombstone_name, &stage)?;
        sync_directory(sandbox)?;
        remove_owned_directory(sandbox, &tombstone_name, stage)?;
        sync_directory(sandbox)
    }

    fn verified_checkpoint(
        &self,
        sandbox: &OwnedStateDirectory,
        sandbox_id: Uuid,
        checkpoint_id: &str,
    ) -> Result<VerifiedCheckpoint> {
        #[cfg(test)]
        self.verified_checkpoint_calls
            .fetch_add(1, Ordering::SeqCst);

        let LoadedCheckpointMetadata {
            metadata,
            directory,
            metadata_file,
            mut artifacts,
        } = self.load_checkpoint_metadata(sandbox, sandbox_id, checkpoint_id)?;

        for (name, artifact) in REQUIRED_ARTIFACTS.iter().zip(&mut artifacts) {
            let expected = metadata
                .artifacts
                .iter()
                .find(|artifact| artifact.name == *name)
                .ok_or_else(|| {
                    invariant(format!(
                        "validated checkpoint {checkpoint_id} has no record for {name}"
                    ))
                })?;
            let actual = hash_artifact(artifact, name)?;
            if &actual != expected {
                return Err(invariant(format!(
                    "checkpoint {checkpoint_id} artifact {name} failed integrity validation"
                )));
            }
        }
        let verified = VerifiedCheckpoint {
            metadata,
            directory,
            metadata_file,
            artifacts,
        };
        verified.require_linked(sandbox, checkpoint_id)?;
        Ok(verified)
    }

    fn load_checkpoint_metadata(
        &self,
        sandbox: &OwnedStateDirectory,
        sandbox_id: Uuid,
        checkpoint_id: &str,
    ) -> Result<LoadedCheckpointMetadata> {
        validate_checkpoint_id(checkpoint_id)?;
        let directory = required_child_directory(
            sandbox,
            checkpoint_id,
            "open committed checkpoint directory",
        )?;
        validate_exact_entries(
            &directory,
            &[
                REQUIRED_ARTIFACTS[0],
                REQUIRED_ARTIFACTS[1],
                REQUIRED_ARTIFACTS[2],
                METADATA_FILE,
            ],
        )?;
        let mut metadata_file =
            open_required_file(&directory, METADATA_FILE, "open checkpoint metadata")?;
        let bytes = read_file(&mut metadata_file, "read checkpoint metadata")?;
        let metadata: CheckpointMetadata =
            serde_json::from_slice(&bytes).map_err(|source| CheckpointStoreError::Json {
                path: metadata_file.path.clone(),
                source,
            })?;
        validate_checkpoint_manifest(&metadata, sandbox_id, checkpoint_id)?;
        let mut artifacts = Vec::with_capacity(REQUIRED_ARTIFACTS.len());
        for name in REQUIRED_ARTIFACTS {
            artifacts.push(open_required_file(
                &directory,
                name,
                "open checkpoint artifact",
            )?);
        }
        let loaded = LoadedCheckpointMetadata {
            metadata,
            directory,
            metadata_file,
            artifacts,
        };
        loaded.require_linked(sandbox, checkpoint_id)?;
        Ok(loaded)
    }

    fn validated_chain_from(
        &self,
        sandbox: &OwnedStateDirectory,
        sandbox_id: Uuid,
        checkpoint_id: &str,
    ) -> Result<Vec<String>> {
        validate_checkpoint_id(checkpoint_id)?;
        let mut current = checkpoint_id.to_string();
        let mut lineage = Vec::new();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(invariant(format!(
                    "checkpoint parent cycle reaches {current}"
                )));
            }
            let metadata = self
                .load_checkpoint_metadata(sandbox, sandbox_id, &current)?
                .metadata;
            lineage.push(current);
            let Some(parent) = metadata.parent else {
                break;
            };
            current = parent;
        }
        Ok(lineage)
    }

    fn load_catalog(
        &self,
        sandbox: &OwnedStateDirectory,
        sandbox_id: Uuid,
    ) -> Result<HashMap<String, CheckpointMetadata>> {
        let mut catalog = HashMap::new();
        for name in directory_names(sandbox, "scan checkpoint catalog")? {
            if !name.starts_with("ckpt-") {
                continue;
            }
            validate_checkpoint_id(&name)?;
            let metadata = self
                .load_checkpoint_metadata(sandbox, sandbox_id, &name)?
                .metadata;
            catalog.insert(name, metadata);
        }
        Ok(catalog)
    }

    fn read_head_id_from(&self, sandbox: &OwnedStateDirectory) -> Result<Option<String>> {
        let Some(mut head) = optional_file(sandbox, HEAD_FILE, "open checkpoint HEAD")? else {
            return Ok(None);
        };
        let bytes = read_file(&mut head, "read checkpoint HEAD")?;
        require_linked_file(sandbox, HEAD_FILE, &head)?;
        let raw = std::str::from_utf8(&bytes)
            .map_err(|error| invariant(format!("checkpoint HEAD is not UTF-8: {error}")))?;
        let checkpoint_id = raw
            .strip_suffix('\n')
            .filter(|value| !value.contains('\n') && !value.contains('\r'))
            .ok_or_else(|| invariant("checkpoint HEAD is not one canonical line"))?;
        validate_checkpoint_id(checkpoint_id)?;
        Ok(Some(checkpoint_id.to_string()))
    }

    fn read_head_from(
        &self,
        sandbox: &OwnedStateDirectory,
        sandbox_id: Uuid,
    ) -> Result<Option<String>> {
        let checkpoint_id = self.read_head_id_from(sandbox)?;
        if let Some(checkpoint_id) = checkpoint_id.as_deref() {
            let _verified = self.verified_checkpoint(sandbox, sandbox_id, checkpoint_id)?;
        }
        Ok(checkpoint_id)
    }

    #[cfg(test)]
    fn set_before_publish_revalidation<F>(&self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        *self
            .before_publish_revalidation
            .lock()
            .expect("checkpoint test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn verified_checkpoint_count(&self) -> usize {
        self.verified_checkpoint_calls.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn run_before_publish_revalidation(&self) {
        if let Some(hook) = self
            .before_publish_revalidation
            .lock()
            .expect("checkpoint test hook lock")
            .take()
        {
            hook();
        }
    }

    #[cfg(test)]
    fn configured_root(&self) -> PathBuf {
        self.root()
            .expect("open checkpoint root")
            .configured_path()
            .to_path_buf()
    }
}

#[derive(Clone, Copy)]
enum ScratchKind {
    Directory,
    File,
}

fn lineage_from(
    catalog: &HashMap<String, CheckpointMetadata>,
    checkpoint_id: &str,
) -> Result<HashSet<String>> {
    let mut current = checkpoint_id.to_string();
    let mut lineage = HashSet::new();
    loop {
        if !lineage.insert(current.clone()) {
            return Err(invariant(format!(
                "checkpoint parent cycle reaches {current}"
            )));
        }
        let metadata = catalog.get(&current).ok_or_else(|| {
            invariant(format!(
                "checkpoint lineage references missing parent {current}"
            ))
        })?;
        let Some(parent) = &metadata.parent else {
            break;
        };
        current = parent.clone();
    }
    Ok(lineage)
}

fn create_child_directory(parent: &OwnedStateDirectory, name: &str) -> Result<OwnedStateDirectory> {
    mkdirat(parent.descriptor(), name, CHECKPOINT_DIRECTORY_MODE).map_err(|source| {
        io_error(
            "create checkpoint directory",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )
    })?;
    match open_child_directory(parent, name) {
        Ok(directory) => Ok(directory),
        Err(error) => {
            let _ = unlinkat(parent.descriptor(), name, AtFlags::REMOVEDIR);
            Err(error)
        }
    }
}

fn open_child_directory(parent: &OwnedStateDirectory, name: &str) -> Result<OwnedStateDirectory> {
    let directory = openat(
        parent.descriptor(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| {
        io_error(
            "open checkpoint directory",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )
    })?;
    Ok(OwnedStateDirectory::new(
        parent.configured_path().join(name),
        directory,
    ))
}

fn optional_child_directory(
    parent: &OwnedStateDirectory,
    name: &str,
) -> Result<Option<OwnedStateDirectory>> {
    match openat(
        parent.descriptor(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => Ok(Some(OwnedStateDirectory::new(
            parent.configured_path().join(name),
            directory,
        ))),
        Err(Errno::NOENT) => Ok(None),
        Err(source) => Err(io_error(
            "open checkpoint directory",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )),
    }
}

fn required_child_directory(
    parent: &OwnedStateDirectory,
    name: &str,
    operation: &'static str,
) -> Result<OwnedStateDirectory> {
    optional_child_directory(parent, name)?.ok_or_else(|| {
        io_error(
            operation,
            parent.configured_path().join(name),
            std::io::Error::from(std::io::ErrorKind::NotFound),
        )
    })
}

fn create_new_file(
    directory: &OwnedStateDirectory,
    name: &str,
    operation: &'static str,
) -> Result<OwnedArtifact> {
    let path = directory.configured_path().join(name);
    let descriptor = openat(
        directory.descriptor(),
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        CHECKPOINT_FILE_MODE,
    )
    .map_err(|source| io_error(operation, &path, std::io::Error::from(source)))?;
    Ok(OwnedArtifact {
        path,
        file: File::from(descriptor),
    })
}

fn open_required_file(
    directory: &OwnedStateDirectory,
    name: &str,
    operation: &'static str,
) -> Result<OwnedArtifact> {
    optional_file(directory, name, operation)?.ok_or_else(|| {
        io_error(
            operation,
            directory.configured_path().join(name),
            std::io::Error::from(std::io::ErrorKind::NotFound),
        )
    })
}

fn optional_file(
    directory: &OwnedStateDirectory,
    name: &str,
    operation: &'static str,
) -> Result<Option<OwnedArtifact>> {
    let path = directory.configured_path().join(name);
    let descriptor = match openat(
        directory.descriptor(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(source) => return Err(io_error(operation, &path, std::io::Error::from(source))),
    };
    let file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|source| io_error("inspect checkpoint file", &path, source))?;
    if !metadata.is_file() {
        return Err(invariant(format!(
            "checkpoint file {} is not a regular file",
            path.display()
        )));
    }
    Ok(Some(OwnedArtifact { path, file }))
}

fn validate_checkpoint_artifact_owner(artifact: &OwnedArtifact) -> Result<()> {
    let metadata = fstat(&artifact.file).map_err(|source| {
        io_error(
            "inspect checkpoint file",
            &artifact.path,
            std::io::Error::from(source),
        )
    })?;
    let expected_uid = unsafe { libc::geteuid() };
    if metadata.st_uid != expected_uid {
        return Err(invariant(format!(
            "checkpoint artifact {} is not owned by the daemon user",
            artifact.path.display()
        )));
    }
    if metadata.st_nlink != 1 {
        return Err(invariant(format!(
            "checkpoint artifact {} must have exactly one hard link",
            artifact.path.display()
        )));
    }
    Ok(())
}

fn validate_exact_entries(directory: &OwnedStateDirectory, expected: &[&str]) -> Result<()> {
    let actual = directory_names(directory, "scan checkpoint directory")?;
    let expected: HashSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
    if actual != expected {
        let mut unexpected: Vec<_> = actual.difference(&expected).cloned().collect();
        let mut missing: Vec<_> = expected.difference(&actual).cloned().collect();
        unexpected.sort();
        missing.sort();
        return Err(invariant(format!(
            "checkpoint directory {} has unexpected entries {:?} and missing entries {:?}",
            directory.configured_path().display(),
            unexpected,
            missing
        )));
    }
    Ok(())
}

fn directory_names(
    directory: &OwnedStateDirectory,
    operation: &'static str,
) -> Result<HashSet<String>> {
    // Open a fresh description so directory offsets from an earlier scan are
    // never reused through the retained owner.
    let scan = openat(
        directory.descriptor(),
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| {
        io_error(
            operation,
            directory.configured_path(),
            std::io::Error::from(source),
        )
    })?;
    let entries = Dir::read_from(&scan).map_err(|source| {
        io_error(
            operation,
            directory.configured_path(),
            std::io::Error::from(source),
        )
    })?;
    let mut names = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            io_error(
                operation,
                directory.configured_path(),
                std::io::Error::from(source),
            )
        })?;
        let Some(name) = entry.file_name().to_str().ok() else {
            return Err(invariant(format!(
                "checkpoint directory {} contains a non-UTF-8 name",
                directory.configured_path().display()
            )));
        };
        if name != "." && name != ".." {
            names.insert(name.to_string());
        }
    }
    Ok(names)
}

fn hash_artifact(artifact: &mut OwnedArtifact, name: &str) -> Result<CheckpointArtifact> {
    artifact
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error("rewind checkpoint artifact", &artifact.path, source))?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = artifact
            .file
            .read(&mut buffer)
            .map_err(|source| io_error("read checkpoint artifact", &artifact.path, source))?;
        if read == 0 {
            break;
        }
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or_else(|| invariant(format!("checkpoint artifact {name} size overflow")))?;
        hasher.update(&buffer[..read]);
    }
    Ok(CheckpointArtifact {
        name: name.to_string(),
        size_bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn write_json_new<T: serde::Serialize>(
    directory: &OwnedStateDirectory,
    name: &str,
    value: &T,
) -> Result<OwnedArtifact> {
    let path = directory.configured_path().join(name);
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| CheckpointStoreError::Json { path, source })?;
    let mut file = create_new_file(directory, name, "create checkpoint metadata")?;
    write_all(&mut file.file, &file.path, &bytes)?;
    write_all(&mut file.file, &file.path, b"\n")?;
    file.file
        .sync_all()
        .map_err(|source| io_error("sync checkpoint metadata", &file.path, source))?;
    Ok(file)
}

fn read_file(file: &mut OwnedArtifact, operation: &'static str) -> Result<Vec<u8>> {
    file.file
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error("rewind checkpoint file", &file.path, source))?;
    let mut bytes = Vec::new();
    file.file
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(operation, &file.path, source))?;
    Ok(bytes)
}

fn write_all(file: &mut File, path: &Path, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .map_err(|source| io_error("write checkpoint file", path, source))
}

fn require_linked_directory(
    parent: &OwnedStateDirectory,
    name: &str,
    expected: &OwnedStateDirectory,
) -> Result<()> {
    let linked = optional_child_directory(parent, name)?.ok_or_else(|| {
        invariant(format!(
            "checkpoint directory {} disappeared",
            parent.configured_path().join(name).display()
        ))
    })?;
    if !same_directory(&linked, expected)? {
        return Err(invariant(format!(
            "checkpoint directory {} changed identity",
            parent.configured_path().join(name).display()
        )));
    }
    Ok(())
}

fn require_linked_file(
    parent: &OwnedStateDirectory,
    name: &str,
    expected: &OwnedArtifact,
) -> Result<()> {
    let linked = open_required_file(parent, name, "revalidate checkpoint file")?;
    let expected_stat = fstat(&expected.file).map_err(|source| {
        io_error(
            "inspect checkpoint file",
            &expected.path,
            std::io::Error::from(source),
        )
    })?;
    let linked_stat = fstat(&linked.file).map_err(|source| {
        io_error(
            "inspect checkpoint file",
            &linked.path,
            std::io::Error::from(source),
        )
    })?;
    if expected_stat.st_dev != linked_stat.st_dev || expected_stat.st_ino != linked_stat.st_ino {
        return Err(invariant(format!(
            "checkpoint file {} changed identity",
            parent.configured_path().join(name).display()
        )));
    }
    Ok(())
}

fn same_directory(left: &OwnedStateDirectory, right: &OwnedStateDirectory) -> Result<bool> {
    let left = fstat(left.descriptor()).map_err(|source| {
        io_error(
            "inspect checkpoint directory",
            left.configured_path(),
            std::io::Error::from(source),
        )
    })?;
    let right = fstat(right.descriptor()).map_err(|source| {
        io_error(
            "inspect checkpoint directory",
            right.configured_path(),
            std::io::Error::from(source),
        )
    })?;
    Ok(left.st_dev == right.st_dev && left.st_ino == right.st_ino)
}

fn sync_directory(directory: &OwnedStateDirectory) -> Result<()> {
    fsync(directory.descriptor()).map_err(|source| {
        io_error(
            "sync checkpoint directory",
            directory.configured_path(),
            std::io::Error::from(source),
        )
    })
}

fn remove_owned_file(parent: &OwnedStateDirectory, name: &str, file: OwnedArtifact) -> Result<()> {
    require_linked_file(parent, name, &file)?;
    unlinkat(parent.descriptor(), name, AtFlags::empty()).map_err(|source| {
        io_error(
            "remove checkpoint scratch file",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )
    })
}

fn remove_file_if_exists(parent: &OwnedStateDirectory, name: &str) -> Result<()> {
    match unlinkat(parent.descriptor(), name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(source) => Err(io_error(
            "remove checkpoint temporary file",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )),
    }
}

fn remove_owned_directory(
    parent: &OwnedStateDirectory,
    name: &str,
    directory: OwnedStateDirectory,
) -> Result<()> {
    require_linked_directory(parent, name, &directory)?;
    let mut entries: Vec<_> = directory_names(&directory, "scan checkpoint scratch directory")?
        .into_iter()
        .collect();
    entries.sort();
    let files = entries
        .into_iter()
        .map(|entry| {
            let file = open_required_file(&directory, &entry, "open checkpoint scratch entry")?;
            Ok((entry, file))
        })
        .collect::<Result<Vec<_>>>()?;
    for (entry, file) in files {
        remove_owned_file(&directory, &entry, file)?;
    }
    sync_directory(&directory)?;
    require_linked_directory(parent, name, &directory)?;
    unlinkat(parent.descriptor(), name, AtFlags::REMOVEDIR).map_err(|source| {
        io_error(
            "remove checkpoint scratch directory",
            parent.configured_path().join(name),
            std::io::Error::from(source),
        )
    })
}

fn classify_scratch_name(name: &str) -> Result<Option<ScratchKind>> {
    if let Some(checkpoint_id) = name
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(STAGING_SUFFIX))
        .filter(|name| name.starts_with("ckpt-"))
    {
        validate_checkpoint_id(checkpoint_id)?;
        return Ok(Some(ScratchKind::Directory));
    }
    if let Some(nonce) = name
        .strip_prefix(".HEAD.")
        .and_then(|name| name.strip_suffix(STAGING_SUFFIX))
    {
        parse_uuid_component(nonce, "checkpoint HEAD staging")?;
        return Ok(Some(ScratchKind::File));
    }
    if let Some(body) = name
        .strip_prefix(ABORT_TOMBSTONE_PREFIX)
        .and_then(|name| name.strip_suffix(TOMBSTONE_SUFFIX))
    {
        let (checkpoint_id, nonce) = body
            .rsplit_once('.')
            .ok_or_else(|| invariant(format!("invalid checkpoint tombstone name {name:?}")))?;
        validate_checkpoint_id(checkpoint_id)?;
        parse_uuid_component(nonce, "checkpoint tombstone")?;
        return Ok(Some(ScratchKind::Directory));
    }
    Ok(None)
}

fn parse_uuid_component(value: &str, label: &str) -> Result<Uuid> {
    let uuid = Uuid::parse_str(value)
        .map_err(|error| invariant(format!("invalid {label} identifier {value:?}: {error}")))?;
    if value != uuid.to_string() {
        return Err(invariant(format!(
            "{label} identifier {value:?} is not canonical"
        )));
    }
    Ok(uuid)
}

fn io_error(
    operation: &'static str,
    path: impl AsRef<Path>,
    source: std::io::Error,
) -> CheckpointStoreError {
    CheckpointStoreError::Io {
        operation,
        path: path.as_ref().to_path_buf(),
        source,
    }
}

fn invariant(message: impl Into<String>) -> CheckpointStoreError {
    CheckpointStoreError::Invariant(message.into())
}

fn checkpoint_store_failpoint(name: &'static str, path: &Path) -> Result<()> {
    crate::failpoint::storage(name).map_err(|error| {
        io_error(
            "run checkpoint store failpoint",
            path,
            std::io::Error::other(error.to_string()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use blaze_core::backend::{BackendKind, SnapshotKind};

    use super::*;

    fn store(temp: &tempfile::TempDir) -> CheckpointStore {
        let state_root = temp.path().join("state");
        fs::create_dir(&state_root).expect("state root");
        CheckpointStore::new(StateStore::new(state_root))
    }

    fn commit_input(parent: Option<String>) -> CommitCheckpoint {
        CommitCheckpoint {
            parent,
            policy_name: "default".to_string(),
            image_digest: "sha256:test".to_string(),
            backend: BackendKind::Mock,
            backend_version: Some("mock-v1".to_string()),
            snapshot_kind: SnapshotKind::Full,
        }
    }

    fn populate(stage: &CheckpointStage, suffix: &str) {
        for name in REQUIRED_ARTIFACTS {
            let mut artifact = create_new_file(&stage.directory, name, "create test artifact")
                .expect("create artifact");
            artifact
                .file
                .write_all(format!("{name}-{suffix}").as_bytes())
                .expect("write artifact");
        }
    }

    fn publish(
        store: &CheckpointStore,
        sandbox_id: Uuid,
        parent: Option<String>,
        move_head: bool,
    ) -> String {
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let id = stage.id().to_string();
        populate(&stage, &id);
        store
            .publish(&stage, commit_input(parent))
            .expect("publish checkpoint");
        if move_head {
            store.set_head(sandbox_id, &id).expect("move HEAD");
        }
        id
    }

    #[test]
    fn publish_verify_and_list_preserve_the_head_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let root = publish(&store, sandbox_id, None, true);
        let unreachable = publish(&store, sandbox_id, Some(root.clone()), false);

        assert_eq!(store.read_head(sandbox_id).expect("HEAD"), Some(root));
        store
            .verify(sandbox_id, &unreachable)
            .expect("published checkpoint");
        let listed = store.list(sandbox_id).expect("list checkpoints");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed.iter().filter(|info| info.is_head).count(), 1);
        assert!(
            listed
                .iter()
                .any(|info| info.id == unreachable && !info.on_head_chain)
        );
    }

    #[test]
    fn checkpoint_tree_uses_owner_only_permissions() {
        if std::env::var_os("BLAZE_CHECKPOINT_MODE_CHILD").is_none() {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "private");
        let root = store.configured_root();
        let sandbox = root.join(sandbox_id.to_string());
        let staging = sandbox.join(&stage.staging_name);

        assert_mode(&root, 0o700);
        assert_mode(&sandbox, 0o700);
        assert_mode(&staging, 0o700);

        for name in REQUIRED_ARTIFACTS {
            fs::set_permissions(staging.join(name), fs::Permissions::from_mode(0o666))
                .expect("make backend artifact permissive");
        }
        let checkpoint_id = stage.id().to_string();
        store
            .publish(&stage, commit_input(None))
            .expect("publish checkpoint");
        store
            .set_head(sandbox_id, &checkpoint_id)
            .expect("set HEAD");

        let committed = sandbox.join(&checkpoint_id);
        assert_mode(&committed, 0o700);
        for name in REQUIRED_ARTIFACTS {
            assert_mode(&committed.join(name), 0o600);
        }
        assert_mode(&committed.join(METADATA_FILE), 0o600);
        assert_mode(&sandbox.join(HEAD_FILE), 0o600);
    }

    #[test]
    fn publish_rejects_multiply_linked_artifacts_without_changing_permissions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "hard-linked");
        let vmstate = stage.artifact_path("vmstate.snap").expect("VM state path");
        let rootfs = stage.artifact_path("rootfs.snap").expect("rootfs path");
        let external = temp.path().join("external-rootfs");
        fs::set_permissions(&vmstate, fs::Permissions::from_mode(0o666))
            .expect("make preceding artifact permissive");
        fs::set_permissions(&rootfs, fs::Permissions::from_mode(0o640))
            .expect("set observable mode");
        fs::hard_link(&rootfs, &external).expect("link artifact outside checkpoint tree");

        let error = store
            .publish(&stage, commit_input(None))
            .expect_err("multiply linked artifact must fail closed");

        assert!(error.to_string().contains("exactly one hard link"));
        assert_eq!(
            fs::metadata(&vmstate).expect("VM state metadata").mode() & 0o777,
            0o666,
            "validation must finish before any artifact permissions change"
        );
        assert_eq!(
            fs::metadata(&external).expect("external metadata").mode() & 0o777,
            0o640
        );
        assert!(store.list(sandbox_id).expect("list").is_empty());
    }

    #[test]
    fn sandbox_removal_accepts_an_interrupted_internal_rootfs_link() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let rootfs = stage.artifact_path("rootfs.snap").expect("rootfs path");
        let temporary = stage
            .directory
            .configured_path()
            .join(".rootfs.snap.capture-interrupted.tmp");
        fs::write(&temporary, b"partial rootfs").expect("write temporary rootfs");
        fs::hard_link(&temporary, &rootfs).expect("link captured rootfs");

        store.remove_sandbox(sandbox_id).expect("remove sandbox");

        assert!(!stage.directory.configured_path().exists());
    }

    #[test]
    fn owner_only_modes_ignore_a_permissive_umask() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script = "umask 000; \"$1\" --exact checkpoint_store::tests::checkpoint_tree_uses_owner_only_permissions --nocapture";
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(std::env::current_exe().expect("test binary"))
            .env("BLAZE_CHECKPOINT_MODE_CHILD", "1")
            .env("TMPDIR", temp.path())
            .output()
            .expect("run child test with a permissive umask");
        assert!(
            output.status.success(),
            "child test failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_mode(path: &Path, expected: u32) {
        assert_eq!(
            fs::symlink_metadata(path)
                .unwrap_or_else(|error| panic!("inspect {}: {error}", path.display()))
                .mode()
                & 0o777,
            expected,
            "unexpected permissions for {}",
            path.display()
        );
    }

    #[test]
    fn list_uses_committed_metadata_without_rehashing_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let root = publish(&store, sandbox_id, None, true);
        let head = publish(&store, sandbox_id, Some(root.clone()), true);
        let expected = store.list(sandbox_id).expect("list intact checkpoints");
        let sandbox = store.configured_root().join(sandbox_id.to_string());

        fs::write(sandbox.join(&root).join("rootfs.snap"), b"corrupted root")
            .expect("corrupt historical checkpoint artifact");
        fs::write(sandbox.join(&head).join("memory.snap"), b"corrupted head")
            .expect("corrupt HEAD checkpoint artifact");

        assert_eq!(
            store.list(sandbox_id).expect("list from metadata"),
            expected
        );
        let verify_error = store
            .verify(sandbox_id, &root)
            .expect_err("verification must hash historical artifacts");
        assert!(
            verify_error
                .to_string()
                .contains("failed integrity validation")
        );
        let set_head_error = store
            .set_head(sandbox_id, &root)
            .expect_err("setting HEAD must hash the target artifacts");
        assert_eq!(
            set_head_error.outcome(),
            CheckpointHeadOutcome::KnownUnchanged
        );
        assert!(
            set_head_error
                .to_string()
                .contains("failed integrity validation")
        );
        assert!(
            store.read_head(sandbox_id).is_err(),
            "reading HEAD must retain full artifact verification"
        );
        assert_eq!(
            store
                .read_head_id(sandbox_id)
                .expect("observing the recorded HEAD must not hash artifacts"),
            Some(head),
            "an unreadable artifact must not hide which checkpoint HEAD names"
        );
    }

    #[test]
    fn publish_does_not_rehash_non_head_ancestor_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let root = publish(&store, sandbox_id, None, true);
        let head = publish(&store, sandbox_id, Some(root.clone()), true);
        let sandbox = store.configured_root().join(sandbox_id.to_string());

        fs::write(sandbox.join(&root).join("rootfs.snap"), b"corrupted root")
            .expect("corrupt non-HEAD ancestor artifact");

        assert_eq!(
            store.read_head(sandbox_id).expect("read intact HEAD"),
            Some(head.clone())
        );
        let next = publish(&store, sandbox_id, Some(head), true);
        assert_eq!(
            store
                .read_head(sandbox_id)
                .expect("read newly published HEAD"),
            Some(next)
        );
        let verify_error = store
            .verify(sandbox_id, &root)
            .expect_err("explicit verification must hash ancestor artifacts");
        assert!(
            verify_error
                .to_string()
                .contains("failed integrity validation")
        );
    }

    #[test]
    fn publish_metadata_only_lineage_validation_rejects_a_missing_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let missing_parent = format!("ckpt-{}", Uuid::new_v4());
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "missing-parent");

        let error = store
            .publish(&stage, commit_input(Some(missing_parent)))
            .expect_err("missing parent must prevent publication");

        assert_eq!(error.outcome(), CheckpointPublishOutcome::KnownUnpublished);
        assert!(
            error
                .to_string()
                .contains("open committed checkpoint directory")
        );
    }

    #[test]
    fn publish_metadata_only_lineage_validation_rejects_a_parent_cycle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let root = publish(&store, sandbox_id, None, true);
        let head = publish(&store, sandbox_id, Some(root.clone()), true);
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        let root_metadata_path = sandbox.join(&root).join(METADATA_FILE);
        let mut root_metadata: CheckpointMetadata =
            serde_json::from_slice(&fs::read(&root_metadata_path).expect("read root metadata"))
                .expect("decode root metadata");
        root_metadata.parent = Some(head.clone());
        fs::write(
            &root_metadata_path,
            serde_json::to_vec(&root_metadata).expect("encode cyclic root metadata"),
        )
        .expect("write cyclic root metadata");
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "parent-cycle");

        let error = store
            .publish(&stage, commit_input(Some(head)))
            .expect_err("parent cycle must prevent publication");

        assert_eq!(error.outcome(), CheckpointPublishOutcome::KnownUnpublished);
        assert!(
            error
                .to_string()
                .contains("checkpoint parent cycle reaches")
        );
    }

    #[test]
    fn sandbox_removal_clears_scratch_and_committed_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let committed = publish(&store, sandbox_id, None, true);
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let sandbox_dir = store.configured_root().join(sandbox_id.to_string());
        let stage_path = sandbox_dir.join(&stage.staging_name);
        let temporary_head = sandbox_dir.join(format!(".HEAD.{}{STAGING_SUFFIX}", Uuid::new_v4()));
        fs::write(&temporary_head, b"temporary").expect("write temporary HEAD");

        store.remove_sandbox(sandbox_id).expect("remove sandbox");

        assert!(!stage_path.exists());
        assert!(!temporary_head.exists());
        assert!(!sandbox_dir.join(committed).exists());
        assert_eq!(store.read_head(sandbox_id).expect("HEAD"), None);
        assert!(!sandbox_dir.exists());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn sandbox_parent_sync_is_retried_for_an_existing_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let hook = crate::failpoint::TestFailpoint::new(&["checkpoint-store-sandbox-parent-sync"]);

        let first_error = hook
            .run(async { store.begin(sandbox_id) })
            .await
            .expect_err("initial parent sync must fail");
        assert!(
            first_error
                .to_string()
                .contains("checkpoint-store-sandbox-parent-sync")
        );
        assert!(
            store
                .configured_root()
                .join(sandbox_id.to_string())
                .is_dir(),
            "the failed parent sync leaves the newly created directory"
        );

        let retry_error = hook
            .run(async { store.begin(sandbox_id) })
            .await
            .expect_err("retry must synchronize the catalog again");
        assert!(
            retry_error
                .to_string()
                .contains("checkpoint-store-sandbox-parent-sync")
        );

        let stage = store.begin(sandbox_id).expect("unarmed retry");
        store.abort(stage).expect("discard retry stage");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn stage_parent_sync_failure_removes_the_owned_stage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let hook = crate::failpoint::TestFailpoint::new(&["checkpoint-store-stage-parent-sync"]);

        let error = hook
            .run(async { store.begin(sandbox_id) })
            .await
            .expect_err("stage parent sync must fail");
        assert!(
            error
                .to_string()
                .contains("checkpoint-store-stage-parent-sync")
        );

        let sandbox = store.configured_root().join(sandbox_id.to_string());
        assert!(
            fs::read_dir(&sandbox)
                .expect("checkpoint sandbox")
                .next()
                .is_none(),
            "failed stage creation must not leave scratch entries"
        );

        let stage = store.begin(sandbox_id).expect("unarmed retry");
        store.abort(stage).expect("discard retry stage");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn sandbox_removal_retries_parent_sync_when_the_namespace_is_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        store.abort(stage).expect("discard stage");
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        let hook =
            crate::failpoint::TestFailpoint::new(&["checkpoint-store-sandbox-remove-parent-sync"]);

        let first_error = hook
            .run(async { store.remove_sandbox(sandbox_id) })
            .await
            .expect_err("initial parent sync must fail");
        assert!(
            first_error
                .to_string()
                .contains("checkpoint-store-sandbox-remove-parent-sync")
        );
        assert!(!sandbox.exists(), "the namespace was already unlinked");

        let retry_error = hook
            .run(async { store.remove_sandbox(sandbox_id) })
            .await
            .expect_err("retry must synchronize the catalog again");
        assert!(
            retry_error
                .to_string()
                .contains("checkpoint-store-sandbox-remove-parent-sync")
        );

        store
            .remove_sandbox(sandbox_id)
            .expect("unarmed retry synchronizes the catalog");
    }

    #[test]
    fn state_root_replacement_does_not_redirect_catalog_creation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let configured_state = temp.path().join("state");
        let retained_state = temp.path().join("retained-state");
        fs::rename(&configured_state, &retained_state).expect("move retained state root");
        fs::create_dir(&configured_state).expect("replacement state root");

        let sandbox_id = Uuid::new_v4();
        let stage = store
            .begin(sandbox_id)
            .expect("begin through retained root");
        populate(&stage, "retained-state");

        assert!(
            retained_state
                .join("checkpoints")
                .join(sandbox_id.to_string())
                .is_dir()
        );
        assert!(!configured_state.join("checkpoints").exists());
    }

    #[test]
    fn catalog_replacement_does_not_redirect_later_operations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let first = Uuid::new_v4();
        let first_stage = store.begin(first).expect("open catalog");
        store.abort(first_stage).expect("discard first stage");
        let configured_catalog = temp.path().join("state/checkpoints");
        let retained_catalog = temp.path().join("retained-checkpoints");
        fs::rename(&configured_catalog, &retained_catalog).expect("move retained catalog");
        fs::create_dir(&configured_catalog).expect("replacement catalog");

        let second = Uuid::new_v4();
        let stage = store.begin(second).expect("begin through retained catalog");
        populate(&stage, "retained-catalog");

        assert!(retained_catalog.join(second.to_string()).is_dir());
        assert!(!configured_catalog.join(second.to_string()).exists());
    }

    #[test]
    fn sandbox_replacement_is_detected_before_publication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "sandbox-owner");
        let configured = store.configured_root().join(sandbox_id.to_string());
        let retained = store.configured_root().join("retained-sandbox");
        fs::rename(&configured, &retained).expect("move retained sandbox");
        fs::create_dir(&configured).expect("replacement sandbox");

        let error = store
            .publish(&stage, commit_input(None))
            .expect_err("sandbox replacement must fail closed");

        assert!(error.to_string().contains("changed identity"));
        assert!(!configured.join(stage.id()).exists());
        assert!(retained.join(&stage.staging_name).is_dir());
    }

    #[test]
    fn stage_replacement_is_detected_before_publication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "stage-owner");
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        let configured_stage = sandbox.join(&stage.staging_name);
        let retained_stage = sandbox.join("retained-stage");
        fs::rename(&configured_stage, &retained_stage).expect("move retained stage");
        fs::create_dir(&configured_stage).expect("replacement stage");
        fs::write(configured_stage.join("sentinel"), b"replacement")
            .expect("write replacement sentinel");

        let error = store
            .publish(&stage, commit_input(None))
            .expect_err("stage replacement must fail closed");

        assert_eq!(error.outcome(), CheckpointPublishOutcome::KnownUnpublished);
        assert!(error.to_string().contains("changed identity"));
        assert!(!sandbox.join(stage.id()).exists());
        let error = store
            .abort(stage)
            .expect_err("replacement must prevent retained-stage cleanup");
        assert!(error.to_string().contains("changed identity"));
        assert_eq!(
            fs::read(configured_stage.join("sentinel")).expect("read replacement sentinel"),
            b"replacement"
        );
        assert!(retained_stage.is_dir());
    }

    #[test]
    fn artifact_replacement_during_publication_is_detected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        populate(&stage, "artifact-owner");
        let rootfs = store
            .configured_root()
            .join(sandbox_id.to_string())
            .join(&stage.staging_name)
            .join("rootfs.snap");
        let retained = temp.path().join("retained-rootfs.snap");
        store.set_before_publish_revalidation(move || {
            fs::rename(&rootfs, &retained).expect("move retained artifact");
            fs::write(&rootfs, b"replacement").expect("replacement artifact");
        });

        let error = store
            .publish(&stage, commit_input(None))
            .expect_err("artifact replacement must fail closed");

        assert!(error.to_string().contains("changed identity"));
        assert!(store.list(sandbox_id).expect("list").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn verify_rejects_artifact_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let checkpoint_id = publish(&store, sandbox_id, None, true);
        let artifact = store
            .configured_root()
            .join(sandbox_id.to_string())
            .join(&checkpoint_id)
            .join("rootfs.snap");
        fs::remove_file(&artifact).expect("remove artifact");
        let outside = temp.path().join("outside");
        fs::write(&outside, b"outside").expect("write outside file");
        symlink(&outside, &artifact).expect("link artifact");

        assert!(store.verify(sandbox_id, &checkpoint_id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn begin_rejects_a_symlinked_catalog_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("state");
        let actual = temp.path().join("actual");
        fs::create_dir(&state).expect("state root");
        fs::create_dir(&actual).expect("actual root");
        symlink(&actual, state.join("checkpoints")).expect("link root");
        let store = CheckpointStore::new(StateStore::new(state));

        assert!(store.begin(Uuid::new_v4()).is_err());
    }

    #[cfg(not(feature = "test-failpoints"))]
    #[test]
    fn production_checkpoint_store_boundary_hooks_are_inert() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let root = publish(&store, sandbox_id, None, true);

        assert_eq!(store.read_head(sandbox_id).expect("HEAD"), Some(root));
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn publish_boundary_error_leaves_a_committed_unreachable_checkpoint() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let checkpoint_id = stage.id().to_string();
        populate(&stage, "publish-boundary");
        let hook = crate::failpoint::TestFailpoint::new(&["checkpoint-store-publish-after-rename"]);

        let error = hook
            .run(async { store.publish(&stage, commit_input(None)) })
            .await
            .expect_err("publish boundary must return a store error");

        assert!(
            error
                .to_string()
                .contains("checkpoint-store-publish-after-rename")
        );
        assert_eq!(error.outcome(), CheckpointPublishOutcome::Unknown);
        store
            .verify(sandbox_id, &checkpoint_id)
            .expect("renamed checkpoint remains committed");
        assert_eq!(store.read_head(sandbox_id).expect("HEAD"), None);
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn publish_pre_rename_error_reports_a_known_unpublished_stage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let checkpoint_id = stage.id().to_string();
        let staging_name = stage.staging_name.clone();
        populate(&stage, "pre-rename-boundary");
        let hook =
            crate::failpoint::TestFailpoint::new(&["checkpoint-store-publish-before-rename"]);

        let error = hook
            .run(async { store.publish(&stage, commit_input(None)) })
            .await
            .expect_err("pre-rename boundary must return a store error");

        assert_eq!(error.outcome(), CheckpointPublishOutcome::KnownUnpublished);
        assert!(
            error
                .to_string()
                .contains("checkpoint-store-publish-before-rename")
        );
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        assert!(sandbox.join(&staging_name).is_dir());
        assert!(sandbox.join(&staging_name).join(METADATA_FILE).is_file());
        assert!(!sandbox.join(checkpoint_id).exists());
        store
            .abort(stage)
            .expect("abort retained unpublished stage");
        assert!(!sandbox.join(staging_name).exists());
    }

    #[test]
    fn publish_rename_error_reports_an_unknown_outcome() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let checkpoint_id = stage.id().to_string();
        let staging_name = stage.staging_name.clone();
        populate(&stage, "rename-collision");
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        let target = sandbox.join(&checkpoint_id);
        let collision = target.clone();
        store.set_before_publish_revalidation(move || {
            fs::create_dir(&collision).expect("create publication collision");
            fs::write(collision.join("sentinel"), b"collision").expect("write collision sentinel");
        });

        let error = store
            .publish(&stage, commit_input(None))
            .expect_err("rename collision must fail publication");

        assert_eq!(error.outcome(), CheckpointPublishOutcome::Unknown);
        assert!(sandbox.join(staging_name).is_dir());
        assert_eq!(
            fs::read(target.join("sentinel")).expect("read collision sentinel"),
            b"collision"
        );
    }

    #[test]
    fn published_witness_sets_head_without_full_verification() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let checkpoint_id = stage.id().to_string();
        populate(&stage, "retained-publication");

        let published = store
            .publish_retained(&stage, commit_input(None))
            .expect("publish checkpoint with retained owners");
        assert_eq!(store.verified_checkpoint_count(), 0);

        let metadata = store
            .set_head_published(published)
            .expect("advance HEAD from published witness");
        assert_eq!(metadata.id, checkpoint_id);
        assert_eq!(
            store.verified_checkpoint_count(),
            0,
            "the publication witness must avoid a second payload scan"
        );
        let head = store
            .configured_root()
            .join(sandbox_id.to_string())
            .join(HEAD_FILE);
        assert_eq!(
            fs::read_to_string(head).expect("read HEAD").trim(),
            checkpoint_id
        );

        store
            .set_head(sandbox_id, &checkpoint_id)
            .expect("public HEAD update performs full verification");
        assert_eq!(store.verified_checkpoint_count(), 1);
    }

    #[test]
    fn published_witness_rejects_replaced_artifact_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let existing_head = publish(&store, sandbox_id, None, true);
        let stage = store.begin(sandbox_id).expect("begin checkpoint");
        let checkpoint_id = stage.id().to_string();
        populate(&stage, "replaced-after-publication");
        let published = store
            .publish_retained(&stage, commit_input(Some(existing_head.clone())))
            .expect("publish checkpoint with retained owners");

        let sandbox = store.configured_root().join(sandbox_id.to_string());
        let artifact = sandbox.join(&checkpoint_id).join("rootfs.snap");
        let displaced = sandbox.join("displaced-rootfs.snap");
        let bytes = fs::read(&artifact).expect("read published rootfs");
        fs::rename(&artifact, &displaced).expect("move retained rootfs");
        fs::write(&artifact, &bytes).expect("write same-content replacement");

        let error = store
            .set_head_published(published)
            .expect_err("replacement must invalidate the publication witness");
        assert_eq!(error.outcome(), CheckpointHeadOutcome::KnownUnchanged);
        assert!(error.to_string().contains("changed identity"));
        assert_eq!(
            fs::read_to_string(sandbox.join(HEAD_FILE))
                .expect("read unchanged HEAD")
                .trim(),
            existing_head
        );
        assert_eq!(fs::read(&artifact).expect("read replacement rootfs"), bytes);
        assert!(displaced.is_file());
        assert!(
            fs::read_dir(&sandbox)
                .expect("checkpoint sandbox")
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".HEAD.")),
            "identity rejection must not leave temporary HEAD state"
        );
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn head_pre_rename_error_removes_scratch_and_reports_known_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let existing_head = publish(&store, sandbox_id, None, true);
        let checkpoint_id = publish(&store, sandbox_id, Some(existing_head.clone()), false);
        let hook = crate::failpoint::TestFailpoint::new(&["checkpoint-store-head-before-rename"]);

        let error = hook
            .run(async { store.set_head(sandbox_id, &checkpoint_id) })
            .await
            .expect_err("pre-rename HEAD boundary must fail");

        assert_eq!(error.outcome(), CheckpointHeadOutcome::KnownUnchanged);
        assert!(
            error
                .to_string()
                .contains("checkpoint-store-head-before-rename")
        );
        assert_eq!(
            store.read_head(sandbox_id).expect("HEAD"),
            Some(existing_head.clone())
        );
        let checkpoints = store.list(sandbox_id).expect("checkpoint catalog");
        assert_eq!(checkpoints.len(), 2);
        assert!(
            checkpoints
                .iter()
                .any(|checkpoint| checkpoint.id == existing_head && checkpoint.is_head)
        );
        assert!(
            checkpoints
                .iter()
                .any(|checkpoint| checkpoint.id == checkpoint_id && !checkpoint.is_head)
        );
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        assert!(
            fs::read_dir(sandbox)
                .expect("checkpoint sandbox")
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".HEAD.")),
            "known-unchanged failure must remove its temporary HEAD"
        );
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn head_pre_rename_cleanup_error_reports_an_unknown_outcome() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let checkpoint_id = publish(&store, sandbox_id, None, false);
        let hook = crate::failpoint::TestFailpoint::new(&[
            "checkpoint-store-head-before-rename",
            "checkpoint-store-head-cleanup",
        ]);

        let error = hook
            .run(async { store.set_head(sandbox_id, &checkpoint_id) })
            .await
            .expect_err("failed pre-rename cleanup must be uncertain");

        assert_eq!(error.outcome(), CheckpointHeadOutcome::Unknown);
        assert!(error.to_string().contains("temporary HEAD cleanup failed"));
        assert_eq!(store.read_head(sandbox_id).expect("HEAD"), None);
        let sandbox = store.configured_root().join(sandbox_id.to_string());
        assert_eq!(
            fs::read_dir(&sandbox)
                .expect("checkpoint sandbox")
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(".HEAD."))
                .count(),
            1,
            "failed cleanup must remain observable for recovery"
        );
        store.remove_sandbox(sandbox_id).expect("remove sandbox");
        assert!(!sandbox.exists());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn head_boundary_error_leaves_the_new_head_visible() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(&temp);
        let sandbox_id = Uuid::new_v4();
        let checkpoint_id = publish(&store, sandbox_id, None, false);
        let hook = crate::failpoint::TestFailpoint::new(&["checkpoint-store-head-after-rename"]);

        let error = hook
            .run(async { store.set_head(sandbox_id, &checkpoint_id) })
            .await
            .expect_err("HEAD boundary must return a store error");

        assert_eq!(error.outcome(), CheckpointHeadOutcome::Unknown);
        assert!(
            error
                .to_string()
                .contains("checkpoint-store-head-after-rename")
        );
        assert_eq!(
            store.read_head(sandbox_id).expect("HEAD"),
            Some(checkpoint_id)
        );
    }
}
