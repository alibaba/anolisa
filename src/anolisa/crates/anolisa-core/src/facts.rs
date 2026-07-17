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
use crate::state::ObjectKind;
use crate::state_store::StateStore;
use crate::transaction::Transaction;

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

    let pending_journal = pending_journal_for(journal_dir, req.name)?.is_some();

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
/// A journal is pending when [`Transaction::is_pending`] holds (in flight
/// or `Partial`). Attribution: a journal whose `subject` matches, or an
/// unattributed journal (written before subjects existed) — the latter
/// blocks conservatively, because an interrupted operation of unknown scope
/// may well concern this object. Files that do not parse as journals are
/// skipped: they cannot be attributed, and `repair` surfaces them through
/// its own scan.
pub fn pending_journal_for(
    journal_dir: &Path,
    subject: &str,
) -> Result<Option<PathBuf>, FactsError> {
    if !journal_dir.exists() {
        return Ok(None);
    }
    let entries = fs::read_dir(journal_dir).map_err(|source| FactsError::JournalScan {
        dir: journal_dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| FactsError::JournalScan {
            dir: journal_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".journal.toml"))
        {
            continue;
        }
        let Ok(journal) = Transaction::load_journal(&path) else {
            continue;
        };
        if !journal.is_pending() {
            continue;
        }
        match &journal.subject {
            Some(s) if s == subject => return Ok(Some(path)),
            None => return Ok(Some(path)),
            Some(_) => continue,
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Installation, LifecycleStatus, OwnedArtifact};
    use crate::providers::test_fakes::{FakeQuery, FakeTxn, InstalledOutcome, pkg_info};
    use crate::state::{FileOwner, OwnedFile, OwnedFileKind};
    use crate::transaction::TransactionOutcomeStatus;

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
            pending_journal_for(&journal_dir, "cosh")
                .expect("scan")
                .is_some()
        );
        assert!(
            pending_journal_for(&journal_dir, "tokenless")
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
                pending_journal_for(&journal_dir, subject)
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
            pending_journal_for(&journal_dir, "d")
                .expect("scan")
                .is_some()
        );
    }

    #[test]
    fn unattributed_pending_journal_blocks_conservatively() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let journal =
            Transaction::begin("install", tmp.path().join("installed.toml"), &journal_dir)
                .expect("begin journal");
        drop(journal);

        assert!(
            pending_journal_for(&journal_dir, "anything")
                .expect("scan")
                .is_some()
        );
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
