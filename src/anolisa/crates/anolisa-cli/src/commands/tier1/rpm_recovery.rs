//! Durable recovery markers for delegated RPM installs.
//!
//! RPM file transactions cannot be atomically committed with
//! `installed.toml`. This module gives install and repair a shared journal
//! contract so a completed dnf transaction can be reconciled after a crash or
//! state-write failure without guessing package ownership.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    OperationRecord, Ownership, RpmMetadata,
};
use anolisa_core::transaction::{
    Transaction, TransactionError, TransactionOutcomeStatus, TransactionStep,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::PackageInfo;

const OPERATION: &str = "install";
const INSTALL_PHASE: &str = "rpm_install";
const INSTALL_ACTION: &str = "dnf_install";
const PERSIST_PHASE: &str = "persist_state";
const PERSIST_ACTION: &str = "record_rpm_managed";

/// Errors raised while creating or discovering RPM install recovery journals.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RpmRecoveryError {
    /// A journal directory could not be read.
    #[error("failed to read RPM recovery journals at {path}: {source}")]
    Io {
        /// Directory or entry path involved in the failure.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// A transaction journal could not be loaded or updated.
    #[error("RPM recovery journal error at {path}: {source}")]
    Journal {
        /// Journal file involved in the failure.
        path: PathBuf,
        /// Underlying transaction error.
        #[source]
        source: TransactionError,
    },
    /// More than one live journal claims the same component.
    #[error(
        "multiple pending RPM installs exist for component '{component}': {operation_ids}; refusing ambiguous recovery"
    )]
    Ambiguous {
        /// Component named by the conflicting journals.
        component: String,
        /// Sorted operation ids for diagnostics.
        operation_ids: String,
    },
    /// A live journal resembles the RPM contract but violates its invariants.
    #[error("invalid RPM recovery journal at {path}: {reason}")]
    InvalidContract {
        /// Journal whose contract could not be trusted.
        path: PathBuf,
        /// Violated contract invariant.
        reason: String,
    },
}

/// Parsed journal contract for one delegated RPM install.
#[derive(Debug)]
pub(crate) struct RpmInstallRecovery {
    transaction: Transaction,
    component: String,
    package: String,
    install_step: usize,
    persist_step: usize,
}

impl RpmInstallRecovery {
    /// Component whose ANOLISA state must be finalized.
    pub(crate) fn component(&self) -> &str {
        &self.component
    }

    /// Backend-native RPM package passed to dnf.
    pub(crate) fn package(&self) -> &str {
        &self.package
    }

    /// Operation id shared by the journal and installed state.
    pub(crate) fn operation_id(&self) -> &str {
        &self.transaction.operation_id
    }

    /// Timestamp captured before dnf was invoked.
    pub(crate) fn started_at(&self) -> &str {
        &self.transaction.started_at
    }

    /// On-disk journal path used to revalidate recovery under the lock.
    pub(crate) fn journal_path(&self) -> &Path {
        &self.transaction.journal_path
    }

    /// Mark the dnf transaction as durably completed.
    pub(crate) fn mark_install_done(&mut self) -> Result<(), TransactionError> {
        self.transaction.mark_done(self.install_step)
    }

    /// Record that the dnf step failed before its result could be committed.
    pub(crate) fn mark_install_failed(&mut self, reason: &str) -> Result<(), TransactionError> {
        self.transaction.mark_failed(self.install_step, reason)
    }

    /// Mark the state-persistence step as durably completed.
    pub(crate) fn mark_persist_done(&mut self) -> Result<(), TransactionError> {
        self.transaction.mark_done(self.persist_step)
    }

    /// Record why the state-persistence step requires repair.
    pub(crate) fn mark_persist_failed(&mut self, reason: &str) -> Result<(), TransactionError> {
        self.transaction.mark_failed(self.persist_step, reason)
    }

    /// Finish the journal with the supplied aggregate outcome.
    pub(crate) fn finish(
        &mut self,
        status: TransactionOutcomeStatus,
    ) -> Result<(), TransactionError> {
        self.transaction.finish(status)
    }
}

/// Create and persist both planned steps before dnf may mutate the host.
pub(crate) fn begin_rpm_install(
    state_path: PathBuf,
    journal_dir: &Path,
    component: &str,
    package: &str,
) -> Result<RpmInstallRecovery, RpmRecoveryError> {
    let mut transaction =
        Transaction::begin(OPERATION, state_path, journal_dir).map_err(|source| {
            RpmRecoveryError::Journal {
                path: journal_dir.to_path_buf(),
                source,
            }
        })?;
    let journal_path = transaction.journal_path.clone();

    let install_step = transaction.steps.len();
    if let Err(source) = transaction.record_step(TransactionStep::planned(
        INSTALL_PHASE,
        package,
        INSTALL_ACTION,
        None,
    )) {
        let _ = transaction.finish(TransactionOutcomeStatus::Failed);
        return Err(RpmRecoveryError::Journal {
            path: journal_path,
            source,
        });
    }

    let persist_step = transaction.steps.len();
    if let Err(source) = transaction.record_step(TransactionStep::planned(
        PERSIST_PHASE,
        component,
        PERSIST_ACTION,
        None,
    )) {
        let _ = transaction.finish(TransactionOutcomeStatus::Failed);
        return Err(RpmRecoveryError::Journal {
            path: journal_path,
            source,
        });
    }

    Ok(RpmInstallRecovery {
        transaction,
        component: component.to_string(),
        package: package.to_string(),
        install_step,
        persist_step,
    })
}

/// Find the unique live RPM install journal for `component`.
///
/// Journals whose operation id is already committed as `ok` in installed
/// state are ignored. This prevents a post-commit journal-write failure from
/// blocking a later reinstall after the original component is removed.
pub(crate) fn find_pending_rpm_install(
    journal_dir: &Path,
    state_path: &Path,
    state: &InstalledState,
    component: &str,
) -> Result<Option<RpmInstallRecovery>, RpmRecoveryError> {
    let entries = match fs::read_dir(journal_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RpmRecoveryError::Io {
                path: journal_dir.to_path_buf(),
                source,
            });
        }
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| RpmRecoveryError::Io {
            path: journal_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".journal.toml"))
        {
            paths.push(path);
        }
    }
    paths.sort();

    let mut matches = Vec::new();
    for path in paths {
        let transaction = load_validated_journal(&path, state_path)?;
        let committed = state
            .operations
            .iter()
            .any(|operation| operation.id == transaction.operation_id && operation.status == "ok");
        if committed {
            continue;
        }
        let Some(recovery) = parse_pending(transaction)? else {
            continue;
        };
        if recovery.component() == component {
            matches.push(recovery);
        }
    }

    if matches.len() > 1 {
        let operation_ids = matches
            .iter()
            .map(|recovery| recovery.operation_id().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(RpmRecoveryError::Ambiguous {
            component: component.to_string(),
            operation_ids,
        });
    }
    Ok(matches.pop())
}

/// Reload one pending journal by path for lock-protected revalidation.
pub(crate) fn load_pending_rpm_install(
    path: &Path,
    state_path: &Path,
) -> Result<Option<RpmInstallRecovery>, RpmRecoveryError> {
    let transaction = load_validated_journal(path, state_path)?;
    parse_pending(transaction)
}

fn load_validated_journal(
    path: &Path,
    expected_state_path: &Path,
) -> Result<Transaction, RpmRecoveryError> {
    let transaction =
        Transaction::load_journal(path).map_err(|source| RpmRecoveryError::Journal {
            path: path.to_path_buf(),
            source,
        })?;
    if transaction.journal_path != path {
        return Err(RpmRecoveryError::InvalidContract {
            path: path.to_path_buf(),
            reason: format!(
                "embedded journal_path '{}' does not match the loaded file",
                transaction.journal_path.display()
            ),
        });
    }
    if transaction.state_path != expected_state_path {
        return Err(RpmRecoveryError::InvalidContract {
            path: path.to_path_buf(),
            reason: format!(
                "embedded state_path '{}' does not match current state '{}'; refusing recovery across state roots",
                transaction.state_path.display(),
                expected_state_path.display()
            ),
        });
    }
    let expected_name = format!("{}.journal.toml", transaction.operation_id);
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(RpmRecoveryError::InvalidContract {
            path: path.to_path_buf(),
            reason: format!(
                "file name does not match operation id '{}'",
                transaction.operation_id
            ),
        });
    }
    Ok(transaction)
}

/// Add the state object and operation shared by install and journal recovery.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_rpm_managed_install(
    state: &mut InstalledState,
    layout: &FsLayout,
    component: &str,
    info: &PackageInfo,
    source_repo: Option<&str>,
    operation_id: &str,
    started_at: &str,
    command: &str,
    finished_at: &str,
) {
    let evr = info.version.to_string();
    state.install_mode = StateInstallMode::System;
    state.prefix = layout.prefix.clone();
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        version: evr.clone(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmManaged),
        rpm_metadata: Some(RpmMetadata {
            package_name: info.name.clone(),
            evr: Some(evr),
            arch: Some(info.arch.clone()),
            source_repo: source_repo.map(str::to_string),
        }),
        installed_at: started_at.to_string(),
        last_operation_id: Some(operation_id.to_string()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
        provisioned_packages: Vec::new(),
    });
    state.operations.push(OperationRecord {
        id: operation_id.to_string(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
    });
}

fn parse_pending(transaction: Transaction) -> Result<Option<RpmInstallRecovery>, RpmRecoveryError> {
    if transaction.operation != OPERATION
        || !matches!(
            transaction.status,
            TransactionOutcomeStatus::InFlight | TransactionOutcomeStatus::Partial
        )
    {
        return Ok(None);
    }

    let resembles_rpm_contract = transaction.steps.iter().any(|step| {
        step.phase == INSTALL_PHASE
            || step.action == INSTALL_ACTION
            || step.action == PERSIST_ACTION
    });
    if !resembles_rpm_contract {
        return Ok(None);
    }
    let invalid = |reason: &str| RpmRecoveryError::InvalidContract {
        path: transaction.journal_path.clone(),
        reason: reason.to_string(),
    };
    if transaction.steps.len() != 2 {
        return Err(invalid("expected exactly two ordered steps"));
    }
    let install = &transaction.steps[0];
    if install.phase != INSTALL_PHASE
        || install.action != INSTALL_ACTION
        || install.target.is_empty()
    {
        return Err(invalid(
            "first step must identify a non-empty dnf package target",
        ));
    }
    let persist = &transaction.steps[1];
    if persist.phase != PERSIST_PHASE
        || persist.action != PERSIST_ACTION
        || persist.target.is_empty()
    {
        return Err(invalid(
            "second step must identify a non-empty component state target",
        ));
    }
    let install_step = 0;
    let persist_step = 1;
    let package = transaction.steps[install_step].target.clone();
    let component = transaction.steps[persist_step].target.clone();

    Ok(Some(RpmInstallRecovery {
        transaction,
        component,
        package,
        install_step,
        persist_step,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::state::OperationRecord;
    use tempfile::tempdir;

    #[test]
    fn pending_round_trip_preserves_component_and_package() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        let recovery = begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("begin");

        let found = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect("scan")
        .expect("pending journal");
        assert_eq!(found.operation_id(), recovery.operation_id());
        assert_eq!(found.component(), "cosh");
        assert_eq!(found.package(), "copilot-shell");
    }

    #[test]
    fn committed_operation_suppresses_stale_pending_journal() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        let recovery = begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("begin");
        let mut state = InstalledState::default();
        state.operations.push(OperationRecord {
            id: recovery.operation_id().to_string(),
            command: "install cosh".to_string(),
            status: "ok".to_string(),
            started_at: recovery.started_at().to_string(),
            finished_at: Some(recovery.started_at().to_string()),
        });

        assert!(
            find_pending_rpm_install(&journal_dir, &state_path, &state, "cosh")
                .expect("scan")
                .is_none()
        );
    }

    #[test]
    fn multiple_pending_journals_are_rejected() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("first marker");
        begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("second marker");

        let err = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect_err("ambiguous markers must fail closed");
        assert!(matches!(err, RpmRecoveryError::Ambiguous { .. }));
    }

    #[test]
    fn corrupt_journal_is_reported_deterministically() {
        let tmp = tempdir().expect("tmpdir");
        let journal_dir = tmp.path().join("journal");
        let state_path = tmp.path().join("installed.toml");
        fs::create_dir_all(&journal_dir).expect("journal dir");
        fs::write(
            journal_dir.join("broken.journal.toml"),
            "this is not valid toml = [",
        )
        .expect("write corrupt marker");

        let err = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect_err("corrupt marker must fail closed");
        assert!(matches!(err, RpmRecoveryError::Journal { .. }));
    }

    #[test]
    fn partial_rpm_contract_is_not_silently_ignored() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        let recovery = begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("marker");
        let path = recovery.journal_path().to_path_buf();
        let mut transaction = Transaction::load_journal(&path).expect("load marker");
        transaction.steps.pop();
        fs::write(
            &path,
            toml::to_string_pretty(&transaction).expect("serialize malformed marker"),
        )
        .expect("write malformed marker");

        let err = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect_err("partial contract must fail closed");
        assert!(matches!(err, RpmRecoveryError::InvalidContract { .. }));
    }

    #[test]
    fn embedded_journal_path_cannot_redirect_recovery_writes() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        let recovery = begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("marker");
        let path = recovery.journal_path().to_path_buf();
        let redirected = tmp.path().join("redirected.journal.toml");
        let mut transaction = Transaction::load_journal(&path).expect("load marker");
        transaction.journal_path = redirected.clone();
        fs::write(
            &path,
            toml::to_string_pretty(&transaction).expect("serialize tampered marker"),
        )
        .expect("write tampered marker");

        let err = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect_err("embedded path mismatch must fail closed");
        assert!(matches!(err, RpmRecoveryError::InvalidContract { .. }));
        assert!(
            !redirected.exists(),
            "scanner must not follow embedded path"
        );
    }

    #[test]
    fn journal_from_another_state_root_is_rejected() {
        let tmp = tempdir().expect("tmpdir");
        let state_path = tmp.path().join("installed.toml");
        let journal_dir = tmp.path().join("journal");
        let recovery = begin_rpm_install(state_path.clone(), &journal_dir, "cosh", "copilot-shell")
            .expect("marker");
        let path = recovery.journal_path().to_path_buf();
        let mut transaction = Transaction::load_journal(&path).expect("load marker");
        transaction.state_path = tmp.path().join("other-state/installed.toml");
        fs::write(
            &path,
            toml::to_string_pretty(&transaction).expect("serialize tampered marker"),
        )
        .expect("write tampered marker");

        let err = find_pending_rpm_install(
            &journal_dir,
            &state_path,
            &InstalledState::default(),
            "cosh",
        )
        .expect_err("foreign state root must fail closed");
        assert!(matches!(err, RpmRecoveryError::InvalidContract { .. }));
    }
}
