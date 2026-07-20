//! Observe stage: assemble the planner's [`Facts`] for one (component,
//! scope) from the state store, the native authority, the journal
//! directory, and the integrity probe.
//!
//! This is the read-only half of the intent → observe → plan → execute
//! pipeline. Nothing here mutates the host: the native database is queried,
//! never transacted; journals are scanned, never consumed; owned files are
//! hashed, never touched.

use std::fs;
use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;
use thiserror::Error;

use crate::domain::{InstallationScope, ProviderBinding};
use crate::integrity::{IntegrityStatus, check_owned_file};
use crate::planner::{Facts, NativeProbe, RecordFacts};
use crate::providers::{DelegatedProvider, ProviderError};
use crate::state::{ObjectKind, OperationRecord};
use crate::state_store::StateStore;
use crate::transaction::{Transaction, TransactionError};

const LEGACY_INSTALL_PHASE: &str = "rpm-install";
const LEGACY_INSTALL_ACTION: &str = "dnf-install";
const LEGACY_STATE_PHASE: &str = "rpm-state";
const LEGACY_STATE_ACTION: &str = "commit-rpm-managed";

/// Same-root evidence used to classify validated operation journals.
///
/// Binding the journal directory to its operation history prevents one scope
/// from settling another scope's recovery markers accidentally.
#[derive(Debug, Clone, Copy)]
pub struct JournalEvidence<'a> {
    journal_dir: &'a Path,
    operations: &'a [OperationRecord],
}

impl<'a> JournalEvidence<'a> {
    /// Bind a journal directory to the operation history from the same state
    /// root.
    pub const fn new(journal_dir: &'a Path, operations: &'a [OperationRecord]) -> Self {
        Self {
            journal_dir,
            operations,
        }
    }

    /// Journal directory covered by this evidence snapshot.
    pub const fn journal_dir(self) -> &'a Path {
        self.journal_dir
    }
}

/// Whether a transaction uses the pre-subject two-step RPM install protocol.
///
/// One exact legacy step is accepted because a successful operation record
/// can outlive a malformed final journal revision. Modern context fields,
/// partial marker matches, duplicates, and foreign steps remain fail-closed.
pub fn is_legacy_rpm_install_journal(transaction: &Transaction) -> bool {
    if transaction.operation != "install"
        || transaction.subject.is_some()
        || transaction.delegated_recovery.is_some()
        || transaction.steps.is_empty()
        || transaction.steps.len() > 2
    {
        return false;
    }

    let mut install_index = None;
    let mut state_index = None;
    for (index, step) in transaction.steps.iter().enumerate() {
        let slot = match (step.phase.as_str(), step.action.as_str()) {
            (LEGACY_INSTALL_PHASE, LEGACY_INSTALL_ACTION) => &mut install_index,
            (LEGACY_STATE_PHASE, LEGACY_STATE_ACTION) => &mut state_index,
            _ => return false,
        };
        if slot.replace(index).is_some() {
            return false;
        }
    }

    match (install_index, state_index) {
        (Some(install), Some(state)) => install < state,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    }
}

fn legacy_rpm_install_is_committed(
    transaction: &Transaction,
    operations: &[OperationRecord],
) -> bool {
    let mut matching = operations
        .iter()
        .filter(|operation| operation.id == transaction.operation_id);
    is_legacy_rpm_install_journal(transaction)
        && matching
            .next()
            .is_some_and(|operation| operation.status == "ok")
        && matching.next().is_none()
}

fn validate_journal_binding(
    transaction: &Transaction,
    path: &Path,
    journal_dir: &Path,
) -> Result<(), TransactionError> {
    if transaction.journal_path != path {
        return Err(TransactionError::CorruptJournal(format!(
            "{}: embedded journal_path {} does not match scanned path {}",
            path.display(),
            transaction.journal_path.display(),
            path.display()
        )));
    }
    let expected_state_path = journal_dir
        .parent()
        .ok_or_else(|| {
            TransactionError::CorruptJournal(format!(
                "{}: journal directory has no state root",
                path.display()
            ))
        })?
        .join("installed.toml");
    if transaction.state_path != expected_state_path {
        return Err(TransactionError::CorruptJournal(format!(
            "{}: embedded state_path {} does not match state root path {}",
            path.display(),
            transaction.state_path.display(),
            expected_state_path.display()
        )));
    }
    Ok(())
}

/// One validated transaction journal and the filesystem path it was loaded
/// from.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    path: PathBuf,
    transaction: Transaction,
    effective_pending: bool,
}

impl JournalEntry {
    /// Journal path discovered while scanning the state root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Validated transaction stored at [`Self::path`].
    pub fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    /// Whether recovery remains pending after applying compatible state
    /// evidence.
    pub fn is_effectively_pending(&self) -> bool {
        self.effective_pending
    }
}

/// Validated snapshot of every journal-shaped file under one state root.
///
/// Construction validates the entire directory before exposing any entry, so
/// callers cannot accidentally accept an earlier matching journal while a
/// later unreadable or unsupported journal leaves recovery scope unknown.
#[derive(Debug, Clone, Default)]
pub struct JournalInventory {
    entries: Vec<JournalEntry>,
}

impl JournalInventory {
    /// Load and validate every `*.journal.toml` file covered by `evidence`.
    ///
    /// A missing directory is an empty inventory. Enumeration, IO, parse, and
    /// schema failures are returned with the directory or journal path that
    /// made the recovery state untrustworthy.
    pub fn load(evidence: JournalEvidence<'_>) -> Result<Self, FactsError> {
        let journal_dir = evidence.journal_dir;
        let entries = match fs::read_dir(journal_dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(FactsError::JournalScan {
                    dir: journal_dir.to_path_buf(),
                    source,
                });
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| FactsError::JournalScan {
                dir: journal_dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".journal.toml"))
            {
                continue;
            }
            paths.push(path);
        }
        paths.sort();

        let entries = paths
            .into_iter()
            .map(|path| {
                let transaction =
                    Transaction::load_journal(&path).map_err(|source| FactsError::JournalLoad {
                        path: path.clone(),
                        source,
                    })?;
                validate_journal_binding(&transaction, &path, journal_dir).map_err(|source| {
                    FactsError::JournalLoad {
                        path: path.clone(),
                        source,
                    }
                })?;
                let effective_pending = transaction.is_pending()
                    && !legacy_rpm_install_is_committed(&transaction, evidence.operations);
                Ok(JournalEntry {
                    path,
                    transaction,
                    effective_pending,
                })
            })
            .collect::<Result<Vec<_>, FactsError>>()?;
        Ok(Self { entries })
    }

    /// All validated journals, including settled entries retained for audit
    /// and legacy recovery compatibility.
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// First pending journal that may affect `subject`.
    ///
    /// Journals written before subject attribution existed match every
    /// subject conservatively because their mutation scope is unknown.
    pub fn blocking_for(&self, subject: &str) -> Option<&JournalEntry> {
        self.entries.iter().find(|entry| {
            entry.is_effectively_pending()
                && match entry.transaction.subject.as_deref() {
                    Some(candidate) => candidate == subject,
                    None => true,
                }
        })
    }

    /// First pending journal explicitly attributed to `subject`.
    ///
    /// Unlike [`Self::blocking_for`], this excludes unattributed legacy
    /// journals because conservative blocking is not proof of recovery
    /// ownership.
    pub fn recoverable_for(&self, subject: &str) -> Option<&JournalEntry> {
        self.entries.iter().find(|entry| {
            entry.is_effectively_pending() && entry.transaction.subject.as_deref() == Some(subject)
        })
    }
}

/// What to observe for one object.
#[derive(Debug, Clone, Copy)]
pub struct ObserveRequest<'a> {
    /// Object vocabulary (component, adapter, osbase).
    pub kind: ObjectKind,
    /// Object name.
    pub name: &'a str,
    /// Scope the intent operates in.
    pub scope: InstallationScope,
    /// Native package to probe, when the intent involves the delegated
    /// family and the name is already resolved. `None` skips the probe
    /// (owned-only paths, user scope) and yields [`NativeProbe::NotProbed`].
    pub native_package: Option<&'a str>,
    /// RFC3339 UTC timestamp stamped into fresh observations.
    pub observed_at: &'a str,
    /// Run the integrity probe over the record's owned files (repair
    /// paths). Skipped otherwise — hashing every file on every plan would
    /// tax `install` for evidence only `repair` consumes.
    pub verify_owned_files: bool,
}

/// How fact assembly failed. Only *reads* can fail here; an absent record
/// or package is a fact, not an error.
#[derive(Debug, Error)]
pub enum FactsError {
    /// The native database could not be queried.
    #[error("native probe failed: {0}")]
    Probe(#[from] ProviderError),
    /// The journal directory could not be scanned.
    #[error("cannot scan journal directory {dir}: {source}")]
    JournalScan {
        /// Directory that failed to enumerate.
        dir: PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// A journal could not be read safely, parsed, or validated against the
    /// supported schema. Mutation must stop because its subject and status
    /// cannot be trusted.
    #[error("cannot safely inspect operation journal {path}: {source}")]
    JournalLoad {
        /// Journal that failed validation.
        path: PathBuf,
        /// Parser, schema, or IO failure returned by the transaction layer.
        #[source]
        source: TransactionError,
    },
}

/// Assemble [`Facts`] for one object.
///
/// `provider` supplies the native probe and may be `None` only when
/// [`ObserveRequest::native_package`] is also `None`. `layout` bounds the
/// integrity probe; it is consulted only when
/// [`ObserveRequest::verify_owned_files`] is set and the record is owned.
pub fn assemble_facts(
    req: &ObserveRequest<'_>,
    store: &StateStore,
    provider: Option<&DelegatedProvider<'_>>,
    layout: &FsLayout,
    journal_dir: &Path,
) -> Result<Facts, FactsError> {
    let record = store.record_facts(req.kind, req.name);

    let native = match (req.native_package, provider) {
        (Some(package), Some(provider)) => provider.observe(package, req.observed_at)?,
        _ => NativeProbe::NotProbed,
    };

    let pending_journal = pending_journal_for(
        JournalEvidence::new(journal_dir, &store.operations),
        req.name,
    )?
    .is_some();

    let active_adapter_claims: Vec<String> = store
        .adapter_claims
        .iter()
        .filter(|claim| claim.component == req.name)
        .map(|claim| claim.framework.clone())
        .collect();

    let owned_files_verified = if req.verify_owned_files {
        verify_owned_files(&record, store, req.kind, req.name, layout)
    } else {
        None
    };

    Ok(Facts {
        scope: req.scope,
        record,
        native,
        pending_journal,
        active_adapter_claims,
        owned_files_verified,
    })
}

/// Integrity verdict over a record's owned file list: `Some(true)` when
/// every probe is healthy, `Some(false)` on any hard finding, `None` when
/// there is nothing to verify (no record, delegated binding, empty file
/// list).
///
/// A quarantined record is verified against the legacy record's file list —
/// that verdict is the evidence repair's R6 exit (rebuild the owned record)
/// consumes.
///
/// `Skipped` (not ANOLISA-owned) and `Unverified` (no recorded digest) are
/// healthy: neither proves drift, and treating absence of evidence as
/// damage would route every digest-less install into repair.
fn verify_owned_files(
    record: &RecordFacts,
    store: &StateStore,
    kind: ObjectKind,
    name: &str,
    layout: &FsLayout,
) -> Option<bool> {
    let files: &[crate::state::OwnedFile] = match record {
        RecordFacts::Active(installation) => match &installation.binding {
            ProviderBinding::Owned { artifact } => &artifact.files,
            ProviderBinding::Delegated { .. } => return None,
        },
        RecordFacts::Quarantined(_) => {
            let quarantined = store
                .quarantined
                .iter()
                .find(|q| q.record.kind == kind && q.record.name == name)?;
            &quarantined.record.files
        }
        RecordFacts::Absent => return None,
    };
    if files.is_empty() {
        return None;
    }
    let all_healthy = files.iter().all(|file| {
        matches!(
            check_owned_file(layout, file),
            IntegrityStatus::Ok | IntegrityStatus::Skipped | IntegrityStatus::Unverified
        )
    });
    Some(all_healthy)
}

/// First pending journal attributed to `subject`, if any.
///
/// A journal is effectively pending when [`Transaction::is_pending`] holds
/// (in flight or `Partial`) and compatible same-root operation history does
/// not prove an exact legacy RPM install committed. Attribution then matches
/// the journal's `subject`; journals written before subjects existed block
/// conservatively. Every journal-shaped file must load and use the supported
/// schema before a result is returned: an unreadable or unrecognized journal
/// has unknown scope and therefore fails closed.
pub fn pending_journal_for(
    evidence: JournalEvidence<'_>,
    subject: &str,
) -> Result<Option<PathBuf>, FactsError> {
    Ok(JournalInventory::load(evidence)?
        .blocking_for(subject)
        .map(|entry| entry.path.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Installation, LifecycleStatus, NativePm, OwnedArtifact};
    use crate::providers::test_fakes::{FakeQuery, FakeTxn, InstalledOutcome, pkg_info};
    use crate::state::{FileOwner, OperationRecord, OwnedFile, OwnedFileKind};
    use crate::transaction::{
        DelegatedRecordAction, DelegatedRecoveryContext, TransactionOutcomeStatus, TransactionStep,
    };

    const NOW: &str = "2026-07-16T00:00:00Z";

    fn request<'a>(name: &'a str, native_package: Option<&'a str>) -> ObserveRequest<'a> {
        ObserveRequest {
            kind: ObjectKind::Component,
            name,
            scope: InstallationScope::System,
            native_package,
            observed_at: NOW,
            verify_owned_files: false,
        }
    }

    fn layout_under(prefix: &Path) -> FsLayout {
        let layout = FsLayout::system(Some(prefix.to_path_buf()));
        fs::create_dir_all(&layout.bin_dir).expect("mkdir bin_dir");
        layout
    }

    fn owned_installation(name: &str, files: Vec<OwnedFile>) -> Installation {
        Installation {
            kind: ObjectKind::Component,
            name: name.to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Owned {
                artifact: OwnedArtifact {
                    version: "1.0.0".to_string(),
                    distribution_source: None,
                    raw_package: None,
                    manifest_digest: None,
                    files,
                    services: Vec::new(),
                    external_modified_files: Vec::new(),
                    provisioned_packages: Vec::new(),
                },
            },
            status: LifecycleStatus::Installed,
            installed_at: NOW.to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        }
    }

    fn begin_legacy_journal(layout: &FsLayout, subject: Option<&str>) -> Transaction {
        let mut journal = Transaction::begin_with_subject(
            "install",
            subject,
            layout.state_dir.join("installed.toml"),
            &layout.state_dir.join("journal"),
        )
        .expect("begin legacy journal");
        journal
            .record_steps([
                TransactionStep::planned(
                    LEGACY_INSTALL_PHASE,
                    "copilot-shell",
                    LEGACY_INSTALL_ACTION,
                    None,
                ),
                TransactionStep::planned(LEGACY_STATE_PHASE, "cosh", LEGACY_STATE_ACTION, None),
            ])
            .expect("record legacy steps");
        journal
    }

    fn operation(operation_id: &str, status: &str) -> OperationRecord {
        OperationRecord {
            id: operation_id.to_string(),
            command: "install cosh".to_string(),
            status: status.to_string(),
            started_at: NOW.to_string(),
            finished_at: Some(NOW.to_string()),
            parent_operation_id: None,
        }
    }

    #[test]
    fn absent_everything_yields_bare_facts() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let store = StateStore::empty();

        let facts = assemble_facts(
            &request("cosh", None),
            &store,
            None,
            &layout,
            &tmp.path().join("journal"),
        )
        .expect("facts");

        assert!(matches!(facts.record, RecordFacts::Absent));
        assert_eq!(facts.native, NativeProbe::NotProbed);
        assert!(!facts.pending_journal);
        assert!(facts.active_adapter_claims.is_empty());
        assert_eq!(facts.owned_files_verified, None);
    }

    #[test]
    fn native_probe_runs_when_a_package_is_named() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let store = StateStore::empty();
        let mut query = FakeQuery::default();
        query.installed.insert(
            "cosh".to_string(),
            InstalledOutcome::Present(pkg_info("cosh", "2.7.0", Some("1.al4"), "x86_64")),
        );
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);

        let facts = assemble_facts(
            &request("cosh", Some("cosh")),
            &store,
            Some(&provider),
            &layout,
            &tmp.path().join("journal"),
        )
        .expect("facts");

        assert!(matches!(
            facts.native,
            NativeProbe::Present { ref package, .. } if package == "cosh"
        ));
    }

    #[test]
    fn pending_journal_is_attributed_to_its_subject() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");

        // An in-flight journal for cosh (dropped without finish → pending).
        let journal = Transaction::begin_with_subject(
            "install",
            Some("cosh"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("begin journal");
        drop(journal);

        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
                .expect("scan")
                .is_some()
        );
        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "tokenless")
                .expect("scan")
                .is_none()
        );
    }

    #[test]
    fn settled_journals_are_not_pending() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");

        for (subject, status) in [
            ("a", TransactionOutcomeStatus::Ok),
            ("b", TransactionOutcomeStatus::Failed),
            ("c", TransactionOutcomeStatus::RolledBack),
        ] {
            let mut journal = Transaction::begin_with_subject(
                "install",
                Some(subject),
                tmp.path().join("installed.toml"),
                &journal_dir,
            )
            .expect("begin journal");
            journal.finish(status).expect("finish");
            assert!(
                pending_journal_for(JournalEvidence::new(&journal_dir, &[]), subject)
                    .expect("scan")
                    .is_none(),
                "{status:?} journals are settled"
            );
        }

        // Partial IS pending: side effects exist the record does not reflect.
        let mut journal = Transaction::begin_with_subject(
            "install",
            Some("d"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("begin journal");
        journal
            .finish(TransactionOutcomeStatus::Partial)
            .expect("finish");
        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "d")
                .expect("scan")
                .is_some()
        );
    }

    #[test]
    fn unattributed_pending_journal_blocks_but_is_not_recoverable() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let journal =
            Transaction::begin("install", tmp.path().join("installed.toml"), &journal_dir)
                .expect("begin journal");
        drop(journal);

        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "anything")
                .expect("scan")
                .is_some()
        );
        let inventory = JournalInventory::load(JournalEvidence::new(&journal_dir, &[]))
            .expect("load inventory");
        assert!(inventory.blocking_for("anything").is_some());
        assert!(inventory.recoverable_for("anything").is_none());
    }

    #[test]
    fn committed_legacy_journal_does_not_block_an_unrelated_fact() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let journal_dir = layout.state_dir.join("journal");
        let journal = begin_legacy_journal(&layout, None);
        let operation_id = journal.operation_id.clone();
        drop(journal);
        let mut store = StateStore::empty();
        store.operations.push(operation(&operation_id, "ok"));

        let facts = assemble_facts(
            &request("unrelated", None),
            &store,
            None,
            &layout,
            &journal_dir,
        )
        .expect("assemble facts");

        assert!(!facts.pending_journal);
    }

    #[test]
    fn subjected_journal_cannot_use_the_legacy_commit_override() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let journal_dir = layout.state_dir.join("journal");
        let journal = begin_legacy_journal(&layout, Some("cosh"));
        let operation_id = journal.operation_id.clone();
        drop(journal);
        let mut store = StateStore::empty();
        store.operations.push(operation(&operation_id, "ok"));

        let facts = assemble_facts(&request("cosh", None), &store, None, &layout, &journal_dir)
            .expect("assemble facts");

        assert!(facts.pending_journal);
    }

    #[test]
    fn legacy_commit_override_requires_matching_success() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let journal_dir = layout.state_dir.join("journal");
        let journal = begin_legacy_journal(&layout, None);
        let operation_id = journal.operation_id.clone();
        drop(journal);

        for operations in [
            vec![operation("different-operation", "ok")],
            vec![operation(&operation_id, "failed")],
            vec![operation(&operation_id, "partial")],
            vec![
                operation(&operation_id, "ok"),
                operation(&operation_id, "failed"),
            ],
            vec![
                operation(&operation_id, "ok"),
                operation(&operation_id, "ok"),
            ],
        ] {
            assert!(
                pending_journal_for(JournalEvidence::new(&journal_dir, &operations), "cosh",)
                    .expect("scan journal")
                    .is_some()
            );
        }

        assert!(
            pending_journal_for(
                JournalEvidence::new(&journal_dir, &[operation(&operation_id, "ok")]),
                "cosh",
            )
            .expect("scan committed journal")
            .is_none()
        );
    }

    #[test]
    fn legacy_protocol_recognition_rejects_modern_or_ambiguous_shapes() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let legacy = begin_legacy_journal(&layout, None);
        assert!(is_legacy_rpm_install_journal(&legacy));

        let mut one_step = legacy.clone();
        one_step.steps.pop();
        assert!(is_legacy_rpm_install_journal(&one_step));

        let mut delegated = legacy.clone();
        delegated.delegated_recovery = Some(DelegatedRecoveryContext {
            pm: NativePm::Rpm,
            package: Some("copilot-shell".to_string()),
            record_action: DelegatedRecordAction::WriteManaged,
        });
        assert!(!is_legacy_rpm_install_journal(&delegated));

        let mut reversed = legacy.clone();
        reversed.steps.reverse();
        assert!(!is_legacy_rpm_install_journal(&reversed));

        let mut duplicate = legacy.clone();
        duplicate.steps[1] = duplicate.steps[0].clone();
        assert!(!is_legacy_rpm_install_journal(&duplicate));

        let mut partial_marker = legacy.clone();
        partial_marker.steps[0].action = "other-action".to_string();
        assert!(!is_legacy_rpm_install_journal(&partial_marker));

        let mut foreign_step = legacy.clone();
        foreign_step.steps.push(TransactionStep::planned(
            "other-phase",
            "cosh",
            "other-action",
            None,
        ));
        assert!(!is_legacy_rpm_install_journal(&foreign_step));

        let mut wrong_operation = legacy;
        wrong_operation.operation = "update".to_string();
        assert!(!is_legacy_rpm_install_journal(&wrong_operation));
    }

    #[test]
    fn corrupt_journal_fails_closed_with_its_path() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        fs::create_dir_all(&journal_dir).expect("journal dir");
        let path = journal_dir.join("broken.journal.toml");
        fs::write(&path, "this is not valid = [toml").expect("corrupt journal");

        let err = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect_err("invalid journals have unknown scope and must block");

        match err {
            FactsError::JournalLoad {
                path: failed_path,
                source: TransactionError::CorruptJournal(_),
            } => assert_eq!(failed_path, path),
            other => panic!("expected corrupt journal error, got {other:?}"),
        }
    }

    #[test]
    fn embedded_journal_path_mismatch_fails_closed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let mut journal = Transaction::begin_with_subject(
            "install",
            Some("cosh"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("journal");
        let actual_path = journal.journal_path.clone();
        journal.journal_path = tmp.path().join("outside.journal.toml");
        fs::write(
            &actual_path,
            toml::to_string_pretty(&journal).expect("serialize journal"),
        )
        .expect("rewrite journal");

        let err = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect_err("a journal cannot redirect recovery writes outside its scanned path");

        match err {
            FactsError::JournalLoad {
                path,
                source: TransactionError::CorruptJournal(reason),
            } => {
                assert_eq!(path, actual_path);
                assert!(reason.contains("embedded journal_path"));
            }
            other => panic!("expected journal path binding error, got {other:?}"),
        }
    }

    #[test]
    fn embedded_state_path_mismatch_fails_closed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let mut journal = Transaction::begin_with_subject(
            "install",
            Some("cosh"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("journal");
        let actual_path = journal.journal_path.clone();
        journal.state_path = tmp.path().join("outside-state.toml");
        fs::write(
            &actual_path,
            toml::to_string_pretty(&journal).expect("serialize journal"),
        )
        .expect("rewrite journal");

        let err = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect_err("a journal cannot redirect state recovery outside its scanned root");

        match err {
            FactsError::JournalLoad {
                path,
                source: TransactionError::CorruptJournal(reason),
            } => {
                assert_eq!(path, actual_path);
                assert!(reason.contains("embedded state_path"));
            }
            other => panic!("expected state path binding error, got {other:?}"),
        }
    }

    #[test]
    fn future_journal_schema_fails_closed_with_its_path() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let journal = Transaction::begin_with_subject(
            "install",
            Some("cosh"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("journal");
        let path = journal.journal_path.clone();
        drop(journal);
        let text = fs::read_to_string(&path).expect("read journal");
        fs::write(
            &path,
            text.replacen("schema_version = 1", "schema_version = 999", 1),
        )
        .expect("write future journal");

        let err = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect_err("unsupported journals have unknown scope and must block");

        match err {
            FactsError::JournalLoad {
                path: failed_path,
                source: TransactionError::CorruptJournal(reason),
            } => {
                assert_eq!(failed_path, path);
                assert!(reason.contains("unsupported journal schema_version 999"));
            }
            other => panic!("expected future-schema journal error, got {other:?}"),
        }
    }

    #[test]
    fn journal_scan_validates_all_files_before_returning_pending() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let mut journal = Transaction::begin_with_subject(
            "install",
            Some("cosh"),
            tmp.path().join("installed.toml"),
            &journal_dir,
        )
        .expect("journal");
        let first = journal_dir.join("000-pending.journal.toml");
        fs::rename(&journal.journal_path, &first).expect("rename pending journal");
        journal.journal_path = first.clone();
        fs::write(
            &first,
            toml::to_string_pretty(&journal).expect("serialize renamed journal"),
        )
        .expect("rewrite renamed journal");
        drop(journal);
        let corrupt = journal_dir.join("zzz-corrupt.journal.toml");
        fs::write(&corrupt, "invalid = [").expect("corrupt journal");

        let err = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect_err("a later invalid journal must outrank an earlier match");

        match err {
            FactsError::JournalLoad { path, .. } => assert_eq!(path, corrupt),
            other => panic!("expected journal load error, got {other:?}"),
        }
    }

    #[test]
    fn owned_files_verified_reports_health_and_damage() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let journal_dir = tmp.path().join("journal");

        // A real file with a matching digest.
        let good_path = layout.bin_dir.join("cosh");
        fs::write(&good_path, b"binary").expect("write file");
        let sha = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(b"binary");
            hash.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            })
        };
        let good_file = OwnedFile {
            path: good_path,
            owner: FileOwner::Anolisa,
            sha256: Some(sha),
            kind: OwnedFileKind::File,
            referent: None,
        };

        let mut store = StateStore::empty();
        store.upsert(owned_installation("cosh", vec![good_file.clone()]));

        let healthy_req = ObserveRequest {
            verify_owned_files: true,
            ..request("cosh", None)
        };
        let facts =
            assemble_facts(&healthy_req, &store, None, &layout, &journal_dir).expect("facts");
        assert_eq!(facts.owned_files_verified, Some(true));

        // Add a missing file: the verdict flips to damaged.
        let missing_file = OwnedFile {
            path: layout.bin_dir.join("gone"),
            ..good_file
        };
        store.upsert(owned_installation("cosh", vec![missing_file]));
        let facts =
            assemble_facts(&healthy_req, &store, None, &layout, &journal_dir).expect("facts");
        assert_eq!(facts.owned_files_verified, Some(false));

        // Without the flag the probe never runs.
        let facts = assemble_facts(&request("cosh", None), &store, None, &layout, &journal_dir)
            .expect("facts");
        assert_eq!(facts.owned_files_verified, None);
    }

    #[test]
    fn owned_files_verified_covers_quarantined_records() {
        use crate::state::{InstalledObject, ObjectStatus, SubscriptionScope};
        use crate::state_migration::{QuarantineReason, QuarantinedObject};

        fn quarantined(name: &str, files: Vec<OwnedFile>) -> QuarantinedObject {
            QuarantinedObject {
                record: InstalledObject {
                    kind: ObjectKind::Component,
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    status: ObjectStatus::Installed,
                    manifest_digest: None,
                    distribution_source: None,
                    raw_package: None,
                    install_backend: None,
                    ownership: None,
                    rpm_metadata: None,
                    installed_at: NOW.to_string(),
                    last_operation_id: None,
                    managed: true,
                    adopted: false,
                    subscription_scope: SubscriptionScope::None,
                    enabled_features: Vec::new(),
                    component_refs: Vec::new(),
                    files,
                    external_modified_files: Vec::new(),
                    services: Vec::new(),
                    health: Vec::new(),
                    provisioned_packages: Vec::new(),
                },
                reason: QuarantineReason::NoEvidence,
            }
        }

        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let journal_dir = tmp.path().join("journal");

        let good_path = layout.bin_dir.join("cosh");
        fs::write(&good_path, b"binary").expect("write file");
        let good_file = OwnedFile {
            path: good_path,
            owner: FileOwner::Anolisa,
            sha256: None,
            kind: OwnedFileKind::File,
            referent: None,
        };

        let req = ObserveRequest {
            verify_owned_files: true,
            ..request("cosh", None)
        };

        // Healthy quarantined files → Some(true): the R6 evidence.
        let mut store = StateStore::empty();
        store.quarantined.push(quarantined("cosh", vec![good_file]));
        let facts = assemble_facts(&req, &store, None, &layout, &journal_dir).expect("facts");
        assert!(matches!(facts.record, RecordFacts::Quarantined(_)));
        assert_eq!(facts.owned_files_verified, Some(true));

        // A missing file flips the verdict.
        let missing_file = OwnedFile {
            path: layout.bin_dir.join("gone"),
            owner: FileOwner::Anolisa,
            sha256: None,
            kind: OwnedFileKind::File,
            referent: None,
        };
        let mut store = StateStore::empty();
        store
            .quarantined
            .push(quarantined("cosh", vec![missing_file]));
        let facts = assemble_facts(&req, &store, None, &layout, &journal_dir).expect("facts");
        assert_eq!(facts.owned_files_verified, Some(false));

        // No file list → nothing to verify.
        let mut store = StateStore::empty();
        store.quarantined.push(quarantined("cosh", Vec::new()));
        let facts = assemble_facts(&req, &store, None, &layout, &journal_dir).expect("facts");
        assert_eq!(facts.owned_files_verified, None);
    }

    #[test]
    fn adapter_claims_filter_by_component() {
        use crate::adapter::claim::{AdapterClaim, ClaimStatus, CoshClaim, DriverPayload};

        fn claim(component: &str, framework: &str) -> AdapterClaim {
            AdapterClaim {
                claim_schema: 1,
                component: component.to_string(),
                framework: framework.to_string(),
                plugin_id: None,
                adapter_type: None,
                enabled_at: NOW.to_string(),
                resource_root: PathBuf::new(),
                bundle_digest: None,
                driver_schema: 1,
                status: ClaimStatus::Enabled,
                resources: Vec::new(),
                driver_payload: DriverPayload::Cosh(CoshClaim {
                    extension_dir_resource: "ext".to_string(),
                }),
            }
        }

        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout = layout_under(tmp.path());
        let mut store = StateStore::empty();
        store.adapter_claims.push(claim("cosh", "copilot"));
        store.adapter_claims.push(claim("tokenless", "gemini"));

        let facts = assemble_facts(
            &request("cosh", None),
            &store,
            None,
            &layout,
            &tmp.path().join("journal"),
        )
        .expect("facts");

        assert_eq!(facts.active_adapter_claims, vec!["copilot".to_string()]);
    }
}
