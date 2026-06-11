//! Atomic lifecycle transactions with rollback support.
//!
//! A [`Transaction`] is a small journal that lifecycle operations
//! (install / enable / disable / uninstall / purge) can plug into to get
//! crash-safe behaviour without each call site re-implementing the
//! "snapshot → mutate → rollback on error" dance.
//!
//! The shape is intentionally concrete:
//!
//! 1. `begin` mints a sortable `operation_id`, snapshots the existing
//!    `state_path` bytes (if any), and writes an empty journal file under
//!    `journal_dir/<operation_id>.journal.toml`.
//! 2. Each meaningful side effect (writing a file, modifying state,
//!    starting a service, …) records a [`TransactionStep`] up front with
//!    `Planned` status. The journal is rewritten atomically (`tmp` →
//!    `rename`) on every change so the file on disk is never half-written.
//! 3. On success the orchestrator calls [`Transaction::mark_done`]; on
//!    failure it calls [`Transaction::mark_failed`] and walks the journal
//!    backwards calling rollback primitives.
//! 4. After a crash, [`Transaction::load_journal`] reads the file back in
//!    so a later `repair` command can finish or rewind the operation.
//!
//! Journal format is TOML (human-greppable, lines up with `installed.toml`
//! and `enable-plan.toml`) and is rewritten in full on every mutation —
//! steps lists are short (tens of entries per op at most) so the cost is
//! negligible, and a single rewrite-and-rename guarantees the on-disk
//! file always parses.
//!
//! AgentSight is not mentioned anywhere on purpose: this primitive is
//! shared by every capability the package manager knows about.

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Schema version for the transaction journal on disk. Bump on
/// incompatible changes; old journals with a different version are
/// reported as [`TransactionError::CorruptJournal`] so callers don't
/// silently mis-parse them.
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;

/// Lifecycle status for a single recorded step.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStepStatus {
    /// Step was recorded but the side effect has not yet been performed.
    Planned,
    /// Step completed successfully.
    Done,
    /// Step had been `Done` and was reverted by a rollback primitive.
    RolledBack,
    /// Step failed; rollback may still be required for prior `Done` steps.
    Failed,
    /// Step was intentionally skipped (idempotency, preconditions, …).
    Skipped,
}

/// Discriminator for rollback strategies. Each variant pairs with the
/// optional fields on [`RollbackAction`] (e.g. `RestoreFile` expects
/// both `source` and `dest`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RollbackActionKind {
    /// Rewrite `state_path` from `Transaction::state_snapshot`.
    RestoreState,
    /// Copy bytes from `source` back to `dest`, optionally checked
    /// against `sha256`.
    RestoreFile,
    /// Delete `dest`. The primitive refuses to touch a path that was not
    /// previously recorded by this transaction; see
    /// [`Transaction::remove_file`].
    RemoveFile,
    /// Recreate `dest` as an empty directory (idempotent).
    RecreateDir,
    /// No-op marker — useful for steps that don't need rollback (e.g.
    /// logging, read-only probes).
    None,
}

/// Concrete parameters for a rollback action. Optional fields are
/// populated based on `kind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackAction {
    /// Strategy selector; determines which optional fields are required.
    pub kind: RollbackActionKind,
    /// Backup/source path used by restore actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    /// Destination path that rollback will restore, remove, or recreate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<PathBuf>,
    /// Expected digest for [`Self::source`] when restore needs verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl RollbackAction {
    /// No-op rollback — convenience for steps that don't need one.
    pub fn none() -> Self {
        Self {
            kind: RollbackActionKind::None,
            source: None,
            dest: None,
            sha256: None,
        }
    }

    /// Rollback that removes a file the transaction created.
    pub fn remove_file(dest: PathBuf) -> Self {
        Self {
            kind: RollbackActionKind::RemoveFile,
            source: None,
            dest: Some(dest),
            sha256: None,
        }
    }

    /// Rollback that copies `source` back over `dest`, optionally
    /// verifying `source`'s SHA256 first.
    pub fn restore_file(source: PathBuf, dest: PathBuf, sha256: Option<String>) -> Self {
        Self {
            kind: RollbackActionKind::RestoreFile,
            source: Some(source),
            dest: Some(dest),
            sha256,
        }
    }
}

/// One row in the journal. `phase` lets the orchestrator tag groups of
/// steps (`"plan"`, `"backup"`, `"materialise"`, `"persist-state"`, …)
/// for nicer diagnostics and replay heuristics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionStep {
    /// Orchestrator phase label for diagnostics and replay ordering.
    pub phase: String,
    /// Path, object name, or unit affected by the step.
    pub target: String,
    /// Human-readable action label recorded before the side effect runs.
    pub action: String,
    /// Current journal status for this step.
    pub status: TransactionStepStatus,
    /// Rollback primitive to apply if a later step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RollbackAction>,
    /// Optional human-readable note; populated by `mark_failed` /
    /// `mark_skipped` so a recovery tool can render *why* without re-reading
    /// the central log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl TransactionStep {
    /// Build a step initialised to `Planned` status.
    pub fn planned(
        phase: impl Into<String>,
        target: impl Into<String>,
        action: impl Into<String>,
        rollback: Option<RollbackAction>,
    ) -> Self {
        Self {
            phase: phase.into(),
            target: target.into(),
            action: action.into(),
            status: TransactionStepStatus::Planned,
            rollback,
            note: None,
        }
    }
}

/// Terminal classification of an operation, suitable for CentralLog.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransactionOutcomeStatus {
    /// `finish` was not (yet) called.
    InFlight,
    /// All recorded steps succeeded or were skipped.
    Ok,
    /// Some step failed; rollback was not performed (or also failed).
    Failed,
    /// Some step failed and prior `Done` steps were rolled back.
    RolledBack,
    /// A mix of `Done` and `Failed` steps with no rollback performed.
    Partial,
}

/// Snapshot summary of a finished or in-flight transaction. Designed to
/// be cheap to compute and trivially serialisable so CentralLog (and the
/// upcoming `LifecycleJournal` trait in the C worktree) can persist
/// `started / phase / succeeded / failed / rolled_back` entries without
/// having to walk the journal themselves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionOutcome {
    /// Operation id shared by the journal, installed state, and central log.
    pub operation_id: String,
    /// Operation verb originally passed to [`Transaction::begin`].
    pub operation: String,
    /// RFC3339 UTC start timestamp.
    pub started_at: String,
    /// RFC3339 UTC finish timestamp, absent for in-flight transactions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Terminal classification for the whole transaction.
    pub status: TransactionOutcomeStatus,
    /// Number of recorded journal steps.
    pub steps_total: usize,
    /// Steps marked [`TransactionStepStatus::Done`].
    pub steps_done: usize,
    /// Steps marked [`TransactionStepStatus::Failed`].
    pub steps_failed: usize,
    /// Steps that were done and later rolled back.
    pub steps_rolled_back: usize,
    /// Steps skipped intentionally.
    pub steps_skipped: usize,
}

/// Atomic lifecycle transaction journal.
///
/// One `Transaction` corresponds to one user-facing operation
/// (`enable foo`, `purge bar`, …). It owns its journal file and keeps
/// in-memory state in lockstep with the on-disk file via `persist`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    /// Journal schema version this struct was serialised against.
    #[serde(default = "default_journal_version")]
    pub schema_version: u32,
    /// `op-YYYYMMDDHHMMSS-<6-hex>` — sortable, unique per call, shared
    /// with the rest of the package manager so a journal entry can be
    /// joined against `installed.toml` and the central log.
    pub operation_id: String,
    /// Operation verb. Free-form so future commands can opt in without
    /// schema churn (`install`, `uninstall`, `disable`, `enable`,
    /// `purge`, …).
    pub operation: String,
    /// RFC3339 UTC timestamp captured during `begin`.
    pub started_at: String,
    /// RFC3339 UTC timestamp captured during `finish`. `None` while the
    /// transaction is still in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Path to the state file the snapshot was taken from. Must be the
    /// same path `restore_state` will write back to.
    pub state_path: PathBuf,
    /// Bytes of `state_path` as observed at `begin`. `None` means the
    /// file did not exist; `restore_state` will delete the file to
    /// match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_snapshot: Option<Vec<u8>>,
    /// On-disk location of this journal file.
    pub journal_path: PathBuf,
    /// Recorded steps, ordered by insertion.
    #[serde(default)]
    pub steps: Vec<TransactionStep>,
    /// Overall classification; populated by `finish` and rollback
    /// helpers.
    #[serde(default = "default_outcome_status")]
    pub status: TransactionOutcomeStatus,
}

fn default_journal_version() -> u32 {
    JOURNAL_SCHEMA_VERSION
}

fn default_outcome_status() -> TransactionOutcomeStatus {
    TransactionOutcomeStatus::InFlight
}

/// Errors raised by [`Transaction`] and its rollback primitives.
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// Filesystem error at the associated path.
    #[error("io error at {0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// Journal content could not be parsed or has an unsupported schema.
    #[error("corrupt journal: {0}")]
    CorruptJournal(String),
    /// A rollback/remove primitive was asked to touch an untracked path.
    #[error("refused to operate on path not tracked by transaction: {0}")]
    UntrackedPath(PathBuf),
    /// Rollback failed after a prior step had already failed.
    #[error("rollback failed: {0}")]
    Rollback(String),
    /// Generic transaction-level failure.
    #[error("transaction failed: {0}")]
    Failed(String),
}

impl Transaction {
    /// Begin a new transaction.
    ///
    /// * `operation` — verb the orchestrator is performing (`install`,
    ///   `enable`, `disable`, `uninstall`, `purge`, …). Stored verbatim.
    /// * `state_path` — path to the state file (`installed.toml` today)
    ///   that the transaction will snapshot. Reading the file is
    ///   non-fatal: a missing file is treated as `state_snapshot = None`
    ///   so first-run installs work.
    /// * `journal_dir` — directory the journal file will be created in.
    ///   The directory is created if it does not exist.
    pub fn begin(
        operation: &str,
        state_path: PathBuf,
        journal_dir: &Path,
    ) -> Result<Self, TransactionError> {
        let now = Utc::now();
        let operation_id = build_operation_id(&now);
        let started_at = now.to_rfc3339_opts(SecondsFormat::Secs, true);

        // Snapshot the state file. Missing file is OK — first-run case.
        let state_snapshot = match fs::read(&state_path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(TransactionError::Io(state_path.clone(), err)),
        };

        if !journal_dir.as_os_str().is_empty() {
            fs::create_dir_all(journal_dir)
                .map_err(|err| TransactionError::Io(journal_dir.to_path_buf(), err))?;
        }

        let journal_path = journal_dir.join(format!("{operation_id}.journal.toml"));

        let tx = Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            operation_id,
            operation: operation.to_string(),
            started_at,
            finished_at: None,
            state_path,
            state_snapshot,
            journal_path,
            steps: Vec::new(),
            status: TransactionOutcomeStatus::InFlight,
        };
        tx.persist()?;
        Ok(tx)
    }

    /// Append a step to the journal and persist.
    pub fn record_step(&mut self, step: TransactionStep) -> Result<(), TransactionError> {
        self.steps.push(step);
        self.persist()
    }

    /// Mark `idx` as [`TransactionStepStatus::Done`] and persist.
    pub fn mark_done(&mut self, idx: usize) -> Result<(), TransactionError> {
        self.set_step_status(idx, TransactionStepStatus::Done, None)
    }

    /// Mark `idx` as [`TransactionStepStatus::Failed`] and persist the
    /// supplied error message under `note` for later diagnostics.
    pub fn mark_failed(&mut self, idx: usize, err: &str) -> Result<(), TransactionError> {
        self.set_step_status(idx, TransactionStepStatus::Failed, Some(err.to_string()))
    }

    /// Mark `idx` as [`TransactionStepStatus::Skipped`] with a reason.
    pub fn mark_skipped(&mut self, idx: usize, reason: &str) -> Result<(), TransactionError> {
        self.set_step_status(
            idx,
            TransactionStepStatus::Skipped,
            Some(reason.to_string()),
        )
    }

    /// Mark `idx` as [`TransactionStepStatus::RolledBack`]. Called by the
    /// orchestrator after a successful `restore_file` / `restore_state`
    /// inside the rollback walk so the journal records the terminal
    /// per-step status (not just `Done`) for forensic reads.
    pub fn mark_rolled_back(&mut self, idx: usize) -> Result<(), TransactionError> {
        self.set_step_status(idx, TransactionStepStatus::RolledBack, None)
    }

    /// Stamp `finished_at` and a terminal `status`, then persist.
    pub fn finish(&mut self, status: TransactionOutcomeStatus) -> Result<(), TransactionError> {
        self.finished_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
        self.status = status;
        self.persist()
    }

    /// Restore `state_path` from the snapshot captured at `begin`.
    ///
    /// If the snapshot is `None` the state file is removed (the pre-op
    /// state was "did not exist"). All other errors are wrapped in
    /// [`TransactionError::Rollback`] so callers can distinguish a
    /// rollback failure from the original failure.
    pub fn restore_state(&self) -> Result<(), TransactionError> {
        match &self.state_snapshot {
            Some(bytes) => write_atomic(&self.state_path, bytes)
                .map_err(|err| TransactionError::Rollback(err.to_string())),
            None => match fs::remove_file(&self.state_path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(TransactionError::Rollback(format!(
                    "remove {}: {err}",
                    self.state_path.display()
                ))),
            },
        }
    }

    /// Remove `path`, but only if the transaction recorded it as a file
    /// it owns.
    ///
    /// The check is deliberately strict: `path` must appear as the
    /// `dest` of at least one step whose status is `Done` or `Planned`
    /// AND whose rollback kind is [`RollbackActionKind::RemoveFile`].
    /// Anything else is rejected with [`TransactionError::UntrackedPath`]
    /// so a buggy caller cannot turn this into `rm -f` for an arbitrary
    /// path. Matching `Planned` lets a forward pass that has not yet
    /// flipped its step to `Done` still call this helper from a `Drop`
    /// guard.
    pub fn remove_file(&self, path: &Path) -> Result<(), TransactionError> {
        let tracked = self.steps.iter().any(|step| match &step.rollback {
            Some(rb) if rb.kind == RollbackActionKind::RemoveFile => {
                rb.dest.as_deref() == Some(path)
                    && matches!(
                        step.status,
                        TransactionStepStatus::Done | TransactionStepStatus::Planned
                    )
            }
            _ => false,
        });
        if !tracked {
            return Err(TransactionError::UntrackedPath(path.to_path_buf()));
        }
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(TransactionError::Io(path.to_path_buf(), err)),
        }
    }

    /// Copy bytes from `rollback.source` to `rollback.dest`. If
    /// `rollback.sha256` is set, the source bytes are verified first;
    /// a mismatch returns [`TransactionError::Rollback`].
    ///
    /// A backup that is itself a symlink (a managed `FileKind::Symlink`
    /// entry backed up as a link) is restored by recreating an identical
    /// link at `dest` — its bytes are never read through, so `sha256` is
    /// not applicable and is ignored.
    pub fn restore_file(&self, rollback: &RollbackAction) -> Result<(), TransactionError> {
        if rollback.kind != RollbackActionKind::RestoreFile {
            return Err(TransactionError::Rollback(format!(
                "restore_file called with {:?}",
                rollback.kind
            )));
        }
        let source = rollback.source.as_ref().ok_or_else(|| {
            TransactionError::Rollback("restore_file: missing source".to_string())
        })?;
        let dest = rollback
            .dest
            .as_ref()
            .ok_or_else(|| TransactionError::Rollback("restore_file: missing dest".to_string()))?;

        let meta = fs::symlink_metadata(source)
            .map_err(|err| TransactionError::Io(source.clone(), err))?;
        if meta.file_type().is_symlink() {
            let referent =
                fs::read_link(source).map_err(|err| TransactionError::Io(source.clone(), err))?;
            if let Some(parent) = dest.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)
                    .map_err(|err| TransactionError::Io(parent.to_path_buf(), err))?;
            }
            match fs::remove_file(dest) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(TransactionError::Io(dest.clone(), err)),
            }
            std::os::unix::fs::symlink(&referent, dest)
                .map_err(|err| TransactionError::Io(dest.clone(), err))?;
            return Ok(());
        }

        let bytes = fs::read(source).map_err(|err| TransactionError::Io(source.clone(), err))?;
        if let Some(expected) = &rollback.sha256 {
            let actual = sha256_hex(&bytes);
            if &actual != expected {
                return Err(TransactionError::Rollback(format!(
                    "sha256 mismatch restoring {}: expected {expected}, got {actual}",
                    source.display()
                )));
            }
        }
        write_atomic(dest, &bytes).map_err(|err| TransactionError::Rollback(err.to_string()))?;
        Ok(())
    }

    /// Load a previously-written journal. Returns
    /// [`TransactionError::CorruptJournal`] (not `Failed`) when the file
    /// exists but cannot be parsed, so callers can distinguish a
    /// genuinely broken journal from a missing one.
    pub fn load_journal(path: &Path) -> Result<Self, TransactionError> {
        let bytes = fs::read(path).map_err(|err| TransactionError::Io(path.to_path_buf(), err))?;
        let text = std::str::from_utf8(&bytes).map_err(|err| {
            TransactionError::CorruptJournal(format!("{}: invalid utf-8: {err}", path.display()))
        })?;
        let tx: Self = toml::from_str(text).map_err(|err| {
            TransactionError::CorruptJournal(format!("{}: {err}", path.display()))
        })?;
        if tx.schema_version != JOURNAL_SCHEMA_VERSION {
            return Err(TransactionError::CorruptJournal(format!(
                "{}: unsupported journal schema_version {}",
                path.display(),
                tx.schema_version
            )));
        }
        Ok(tx)
    }

    /// Summary view aligned with the upcoming CentralLog operation
    /// records. Cheap; safe to call from a `Drop` guard.
    pub fn outcome_record(&self) -> TransactionOutcome {
        let mut steps_done = 0usize;
        let mut steps_failed = 0usize;
        let mut steps_rolled_back = 0usize;
        let mut steps_skipped = 0usize;
        for s in &self.steps {
            match s.status {
                TransactionStepStatus::Done => steps_done += 1,
                TransactionStepStatus::Failed => steps_failed += 1,
                TransactionStepStatus::RolledBack => steps_rolled_back += 1,
                TransactionStepStatus::Skipped => steps_skipped += 1,
                TransactionStepStatus::Planned => {}
            }
        }
        TransactionOutcome {
            operation_id: self.operation_id.clone(),
            operation: self.operation.clone(),
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
            status: self.status,
            steps_total: self.steps.len(),
            steps_done,
            steps_failed,
            steps_rolled_back,
            steps_skipped,
        }
    }

    fn set_step_status(
        &mut self,
        idx: usize,
        status: TransactionStepStatus,
        note: Option<String>,
    ) -> Result<(), TransactionError> {
        let len = self.steps.len();
        let step = self.steps.get_mut(idx).ok_or_else(|| {
            TransactionError::Failed(format!("step index {idx} out of range (have {len} steps)"))
        })?;
        step.status = status;
        if note.is_some() {
            step.note = note;
        }
        self.persist()
    }

    /// Rewrite the journal file atomically. We rewrite (rather than
    /// append) on every mutation so the file always parses; step lists
    /// are short enough that this is cheaper than a JSONL append plus
    /// a recovery step.
    fn persist(&self) -> Result<(), TransactionError> {
        if let Some(parent) = self.journal_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|err| TransactionError::Io(parent.to_path_buf(), err))?;
        }
        let content = toml::to_string_pretty(self).map_err(|err| {
            TransactionError::Failed(format!(
                "serialise journal {}: {err}",
                self.journal_path.display()
            ))
        })?;
        write_atomic(&self.journal_path, content.as_bytes())
            .map_err(|err| TransactionError::Io(self.journal_path.clone(), err))
    }
}

/// `op-YYYYMMDDHHMMSS-<6-hex>` — matches the format used by the rest of
/// anolisa (`enable_execute::build_operation_id`,
/// `disable_execute::build_operation_id`) so journal ids round-trip
/// 1:1 with `installed.toml::operations[].id` and the central log.
fn build_operation_id(now: &DateTime<Utc>) -> String {
    let ts = now.format("%Y%m%d%H%M%S").to_string();
    let nanos = now.timestamp_nanos_opt().unwrap_or_else(|| now.timestamp());
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    let suffix = hasher.finish() & 0xff_ffff;
    format!("op-{ts}-{suffix:06x}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `tmp` + `rename` write so a crash mid-write cannot leave a truncated
/// file. Mirrors `InstalledState::save` in `state.rs`.
///
/// Security-critical: the tmp sibling is opened with `O_CREAT|O_EXCL`
/// (plus `O_NOFOLLOW` on Unix) by [`open_excl_nofollow`] so a pre-placed
/// `.{file_name}.<...>.tmp` symlink — or any other existing entry at the
/// tmp path — fails the open instead of letting us write through it to a
/// path outside the journal directory. The tmp name itself is salted
/// with the writer's pid, a process-wide monotonic counter and a
/// nanosecond timestamp so two concurrent `record_step` writers on the
/// same operation_id (or a stale tmp left behind by an earlier process)
/// cannot collide on the same path.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    let mut f = open_excl_nofollow(&tmp)?;
    if let Err(err) = f.write_all(bytes) {
        // Drop the half-written tmp so we don't leak it.
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    // Best-effort durability: matches the pattern in download.rs /
    // install_runner.rs — a sync_all failure here is not fatal because
    // the rename below is the actual atomicity guarantee.
    let _ = f.sync_all();
    // Close before rename so the bytes are fully flushed to the
    // descriptor before another process can observe the renamed file.
    drop(f);
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// Open `tmp` for writing with `O_CREAT|O_EXCL` (+ `O_NOFOLLOW` on Unix).
///
/// Extracted as a named helper so the symlink/TOCTOU hardening can be
/// exercised directly from tests without having to race the random tmp
/// suffix produced by [`tmp_path_for`]. Mirrors the pattern used by
/// `download::stream_reader_and_hash` and
/// `install_runner::stream_write_and_hash`.
fn open_excl_nofollow(tmp: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    opts.open(tmp)
}

/// Monotonic, process-wide counter mixed into [`tmp_path_for`] so that
/// concurrent writers on the same `path` don't pick the same tmp name.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique tmp sibling path for `path`.
///
/// Pattern: `.{file_name}.{pid}.{counter}.{nanos}.tmp`. The pid keeps
/// cross-process writes disjoint; the atomic counter keeps same-process
/// concurrent writes disjoint; the nanosecond timestamp adds entropy in
/// case the counter wraps. Combined with `O_CREAT|O_EXCL` in
/// [`open_excl_nofollow`] this means a stale tmp (or a hostile plant) at
/// the *exact* generated path is a hard error, not a silent overwrite.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "journal.toml".to_string());
    let pid = std::process::id();
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    tmp.set_file_name(format!(".{file_name}.{pid}.{counter}.{nanos}.tmp"));
    tmp
}

// NOTE on integration with existing call sites:
//
// `enable_execute.rs` keeps its own ad-hoc `prior_state_bytes: Option<Vec<u8>>`
// snapshot today (see comment around the `Step 5 — persist state` block).
// Migrating that to `Transaction` is a deliberate follow-up: it would change
// the order in which we mint the `operation_id` (transaction-first vs.
// late-binding) and would intertwine with the central-log + journal merge
// the C-branch is doing. Doing it here would create cross-branch merge pain
// for zero behaviour change today, so we leave the wiring for a follow-up
// commit once both D (this branch) and C land.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as std_fs;
    use tempfile::tempdir;

    fn fresh(tmp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        (state_path, journal_dir)
    }

    #[test]
    fn begin_creates_journal_file() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);

        let tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        assert!(tx.journal_path.exists(), "journal file must be created");
        assert!(tx.journal_path.starts_with(&journal_dir));
        assert!(tx.operation_id.starts_with("op-"));
        let on_disk = std_fs::read_to_string(&tx.journal_path).expect("read journal");
        assert!(on_disk.contains(&tx.operation_id));
        assert!(on_disk.contains("operation = \"install\""));
    }

    #[test]
    fn begin_with_missing_state_yields_none_snapshot() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);

        let tx = Transaction::begin("enable", state_path.clone(), &journal_dir).expect("begin");
        assert!(tx.state_snapshot.is_none());
        assert!(!state_path.exists());
    }

    #[test]
    fn begin_captures_existing_state_bytes() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        std_fs::write(&state_path, b"prior bytes").expect("seed state");

        let tx = Transaction::begin("disable", state_path, &journal_dir).expect("begin");
        assert_eq!(tx.state_snapshot.as_deref(), Some(b"prior bytes".as_ref()));
    }

    #[test]
    fn record_step_persists_to_journal() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");

        tx.record_step(TransactionStep::planned(
            "materialise",
            "/opt/anolisa/bin/foo",
            "install_file",
            Some(RollbackAction::remove_file(PathBuf::from(
                "/opt/anolisa/bin/foo",
            ))),
        ))
        .expect("record step");

        let reloaded = Transaction::load_journal(&tx.journal_path).expect("load");
        assert_eq!(reloaded.steps.len(), 1);
        assert_eq!(reloaded.steps[0].action, "install_file");
        assert_eq!(reloaded.steps[0].status, TransactionStepStatus::Planned);
    }

    #[test]
    fn restore_state_from_snapshot_writes_bytes() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        std_fs::write(&state_path, b"original").expect("seed");

        let tx = Transaction::begin("install", state_path.clone(), &journal_dir).expect("begin");
        std_fs::write(&state_path, b"mutated").expect("simulate mid-op write");

        tx.restore_state().expect("restore");
        assert_eq!(std_fs::read(&state_path).expect("read"), b"original");
    }

    #[test]
    fn restore_state_without_snapshot_removes_file() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);

        let tx = Transaction::begin("install", state_path.clone(), &journal_dir).expect("begin");
        assert!(tx.state_snapshot.is_none());

        std_fs::write(&state_path, b"mutated").expect("simulate write");
        tx.restore_state().expect("restore");
        assert!(
            !state_path.exists(),
            "missing-snapshot rollback deletes state file"
        );
    }

    #[test]
    fn restore_state_with_no_snapshot_and_no_file_is_noop() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        tx.restore_state().expect("restore idempotent");
    }

    #[test]
    fn remove_file_refuses_untracked_path() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let stranger = tmp.path().join("stranger.bin");
        std_fs::write(&stranger, b"do not touch").expect("seed");

        let tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        let err = tx.remove_file(&stranger).expect_err("must refuse");
        match err {
            TransactionError::UntrackedPath(p) => assert_eq!(p, stranger),
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(stranger.exists(), "untracked path must NOT be deleted");
    }

    #[test]
    fn remove_file_removes_tracked_path() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let owned = tmp.path().join("owned.bin");
        std_fs::write(&owned, b"anolisa-managed").expect("seed");

        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        tx.record_step(TransactionStep::planned(
            "materialise",
            owned.to_string_lossy(),
            "install_file",
            Some(RollbackAction::remove_file(owned.clone())),
        ))
        .expect("record");
        tx.mark_done(0).expect("done");

        tx.remove_file(&owned).expect("remove tracked");
        assert!(!owned.exists());
    }

    #[test]
    fn mark_failed_persists_and_round_trips() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        tx.record_step(TransactionStep::planned(
            "precheck",
            "agent-observability",
            "env-check",
            None,
        ))
        .expect("record");
        tx.mark_failed(0, "env-check failed: kernel too old")
            .expect("mark failed");

        let reloaded = Transaction::load_journal(&tx.journal_path).expect("load");
        assert_eq!(reloaded.steps[0].status, TransactionStepStatus::Failed);
        assert_eq!(
            reloaded.steps[0].note.as_deref(),
            Some("env-check failed: kernel too old")
        );
    }

    #[test]
    fn mark_skipped_persists() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        tx.record_step(TransactionStep::planned(
            "materialise",
            "/opt/a",
            "install",
            None,
        ))
        .expect("record");
        tx.mark_skipped(0, "already up to date").expect("skip");

        let reloaded = Transaction::load_journal(&tx.journal_path).expect("load");
        assert_eq!(reloaded.steps[0].status, TransactionStepStatus::Skipped);
        assert_eq!(
            reloaded.steps[0].note.as_deref(),
            Some("already up to date")
        );
    }

    #[test]
    fn load_journal_on_corrupt_content_returns_corrupt_journal() {
        let tmp = tempdir().expect("tempdir");
        let bad = tmp.path().join("bad.journal.toml");
        std_fs::write(&bad, b"= not valid toml =").expect("seed");

        let err = Transaction::load_journal(&bad).expect_err("must fail");
        match err {
            TransactionError::CorruptJournal(_) => {}
            other => panic!("expected CorruptJournal, got {other:?}"),
        }
    }

    #[test]
    fn load_journal_rejects_unknown_schema_version() {
        let tmp = tempdir().expect("tempdir");
        let bad = tmp.path().join("future.journal.toml");
        std_fs::write(
            &bad,
            br#"schema_version = 999
operation_id = "op-x"
operation = "install"
started_at = "2026-01-01T00:00:00Z"
state_path = "/dev/null"
journal_path = "/tmp/x.journal.toml"
"#,
        )
        .expect("seed");

        let err = Transaction::load_journal(&bad).expect_err("must fail");
        match err {
            TransactionError::CorruptJournal(msg) => {
                assert!(msg.contains("schema_version"), "msg: {msg}");
            }
            other => panic!("expected CorruptJournal, got {other:?}"),
        }
    }

    #[test]
    fn restore_file_copies_bytes_and_verifies_sha256() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");

        let backup = tmp.path().join("backup/foo.conf");
        let dest = tmp.path().join("etc/foo.conf");
        std_fs::create_dir_all(backup.parent().expect("parent")).expect("mkdir");
        std_fs::write(&backup, b"original config").expect("seed backup");

        let rb = RollbackAction::restore_file(
            backup.clone(),
            dest.clone(),
            Some(sha256_hex(b"original config")),
        );
        tx.restore_file(&rb).expect("restore_file");
        assert_eq!(std_fs::read(&dest).expect("read"), b"original config");

        // Sha mismatch surfaces as Rollback error.
        let mut bad = rb.clone();
        bad.sha256 = Some("deadbeef".to_string());
        let err = tx.restore_file(&bad).expect_err("mismatch");
        match err {
            TransactionError::Rollback(_) => {}
            other => panic!("expected Rollback, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn restore_file_recreates_symlink_backup_as_link() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let tx = Transaction::begin("uninstall", state_path, &journal_dir).expect("begin");

        let referent = tmp.path().join("libexec/rtk");
        std_fs::create_dir_all(referent.parent().expect("parent")).expect("mkdir");
        std_fs::write(&referent, b"rtk bytes").expect("seed referent");
        let backup = tmp.path().join("backup/0.bak");
        std_fs::create_dir_all(backup.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&referent, &backup).expect("seed backup link");
        let dest = tmp.path().join("bin/rtk");

        let rb = RollbackAction::restore_file(backup, dest.clone(), None);
        tx.restore_file(&rb).expect("restore_file");

        let meta = std_fs::symlink_metadata(&dest).expect("dest exists");
        assert!(meta.file_type().is_symlink(), "dest must be a link");
        assert_eq!(std_fs::read_link(&dest).expect("read_link"), referent);
    }

    #[test]
    fn outcome_record_counts_step_statuses() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        for action in ["a", "b", "c", "d"] {
            tx.record_step(TransactionStep::planned("p", action, "do", None))
                .expect("record");
        }
        tx.mark_done(0).expect("done");
        tx.mark_done(1).expect("done");
        tx.mark_failed(2, "boom").expect("fail");
        tx.mark_skipped(3, "skip").expect("skip");
        tx.finish(TransactionOutcomeStatus::Partial)
            .expect("finish");

        let outcome = tx.outcome_record();
        assert_eq!(outcome.operation_id, tx.operation_id);
        assert_eq!(outcome.operation, "install");
        assert_eq!(outcome.steps_total, 4);
        assert_eq!(outcome.steps_done, 2);
        assert_eq!(outcome.steps_failed, 1);
        assert_eq!(outcome.steps_skipped, 1);
        assert_eq!(outcome.steps_rolled_back, 0);
        assert_eq!(outcome.status, TransactionOutcomeStatus::Partial);
        assert!(outcome.finished_at.is_some());
    }

    #[test]
    fn finish_persists_status_and_finished_at() {
        let tmp = tempdir().expect("tempdir");
        let (state_path, journal_dir) = fresh(&tmp);
        let mut tx = Transaction::begin("install", state_path, &journal_dir).expect("begin");
        tx.finish(TransactionOutcomeStatus::Ok).expect("finish");

        let reloaded = Transaction::load_journal(&tx.journal_path).expect("load");
        assert_eq!(reloaded.status, TransactionOutcomeStatus::Ok);
        assert!(reloaded.finished_at.is_some());
    }

    // --- write_atomic hardening: tmp-symlink TOCTOU regression suite.
    //
    // Testing approach (documented inline because it's a bit non-obvious):
    //
    // The random suffix in `tmp_path_for` means we can't race-fully plant a
    // symlink at the exact tmp path that the production code will pick.
    // Instead we exercise the two invariants directly:
    //
    //   1. `open_excl_nofollow` is extracted as a private helper and tested
    //      against a pre-placed symlink at a *known* path. This is the
    //      primitive that closes the TOCTOU hole; if it ever regresses to
    //      following symlinks the test fires immediately.
    //
    //   2. `tmp_path_for` is exercised end-to-end via `write_atomic` to
    //      confirm (a) two back-to-back writes don't collide and (b) a
    //      symlink planted at the *final* target gets atomically replaced
    //      by the rename (which is the documented unix `rename(2)` behaviour
    //      — the symlink itself, not its target, is replaced).

    #[test]
    fn tmp_path_for_includes_random_suffix_and_does_not_collide() {
        let p = Path::new("/tmp/x/journal.toml");
        let a = tmp_path_for(p);
        let b = tmp_path_for(p);
        let an = a.file_name().expect("tmp file_name").to_string_lossy();
        let bn = b.file_name().expect("tmp file_name").to_string_lossy();
        assert!(an.starts_with(".journal.toml."));
        assert!(an.ends_with(".tmp"));
        assert_ne!(an, bn, "two tmp paths for the same target must differ");
    }

    #[cfg(unix)]
    #[test]
    fn open_excl_nofollow_refuses_existing_symlink() {
        // Direct test of the primitive: a pre-placed symlink at the tmp path
        // must error rather than letting the open follow it through to the
        // victim. Without O_NOFOLLOW + O_EXCL this would silently truncate
        // `victim`.
        let dir = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside tempdir");
        let victim = outside.path().join("victim");
        std_fs::write(&victim, b"do not touch").expect("seed victim");

        let tmp_plant = dir.path().join(".target.tmp");
        std::os::unix::fs::symlink(&victim, &tmp_plant).expect("plant symlink");

        let err = open_excl_nofollow(&tmp_plant).expect_err("must refuse symlink");
        // Either ELOOP (NOFOLLOW kicked in) or EEXIST (EXCL kicked in) is
        // acceptable; both mean the bytes never touched the victim.
        let kind = err.kind();
        assert!(
            kind == io::ErrorKind::AlreadyExists || err.raw_os_error() == Some(nix::libc::ELOOP),
            "expected EEXIST or ELOOP, got {err:?}",
        );
        let victim_bytes = std_fs::read(&victim).expect("victim still readable");
        assert_eq!(
            victim_bytes, b"do not touch",
            "symlinked tmp must never be written through",
        );
    }

    #[test]
    fn open_excl_nofollow_refuses_existing_regular_file() {
        // create_new semantics: simulating "a previous tmp file is already
        // sitting at the exact generated path" must surface as EEXIST so
        // we never blindly overwrite arbitrary on-disk state.
        let dir = tempdir().expect("tempdir");
        let tmp_plant = dir.path().join(".already-here.tmp");
        std_fs::write(&tmp_plant, b"stale").expect("seed stale tmp");

        let err = open_excl_nofollow(&tmp_plant).expect_err("must refuse existing file");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn write_atomic_back_to_back_calls_both_succeed() {
        // Verifies the random suffix is doing its job: two write_atomic
        // calls in quick succession must both succeed without an EEXIST
        // collision on the tmp path. Catches the regression where the
        // tmp name was fixed and a leftover tmp from call N would make
        // call N+1 fail.
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("journal.toml");

        write_atomic(&target, b"one").expect("first write");
        assert_eq!(std_fs::read(&target).expect("read"), b"one");
        write_atomic(&target, b"two").expect("second write");
        assert_eq!(std_fs::read(&target).expect("read"), b"two");

        // No tmp file should linger in the parent dir.
        let leftovers: Vec<_> = std_fs::read_dir(dir.path())
            .expect("read parent dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "write_atomic must not leak tmp siblings: {leftovers:?}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_replaces_symlinked_target_without_touching_victim() {
        // If the *final* path is a symlink to a victim outside the journal
        // dir, `rename(2)` replaces the symlink itself (not the target).
        // The victim must be untouched and the journal dir must end up
        // holding a regular file with the new bytes.
        let dir = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside tempdir");
        let victim = outside.path().join("victim");
        std_fs::write(&victim, b"do not touch").expect("seed victim");

        let target = dir.path().join("journal.toml");
        std::os::unix::fs::symlink(&victim, &target).expect("plant symlink at target");

        write_atomic(&target, b"fresh bytes").expect("write_atomic over symlink");

        // Final target is now a regular file with our bytes.
        let meta = std_fs::symlink_metadata(&target).expect("stat target");
        assert!(
            meta.file_type().is_file(),
            "target must be a regular file after rename, was {:?}",
            meta.file_type(),
        );
        assert_eq!(std_fs::read(&target).expect("read target"), b"fresh bytes");

        // Victim outside the journal dir is unchanged.
        let victim_bytes = std_fs::read(&victim).expect("read victim");
        assert_eq!(
            victim_bytes, b"do not touch",
            "rename must replace the symlink, not write through it",
        );
    }
}
