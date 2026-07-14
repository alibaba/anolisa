//! Durable intent and recovery helpers for fresh delegated RPM installs.

use std::fs;
use std::path::{Path, PathBuf};

use anolisa_core::state::{
    InstalledObject, InstalledState, ObjectKind, ObjectStatus, OperationRecord, Ownership,
    RpmMetadata,
};
use anolisa_core::transaction::{
    Transaction, TransactionError, TransactionOutcomeStatus, TransactionStep, TransactionStepStatus,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::PackageInfo;

use crate::response::CliError;

const INSTALL_PHASE: &str = "rpm-install";
const INSTALL_ACTION: &str = "dnf-install";
const STATE_PHASE: &str = "rpm-state";
const STATE_ACTION: &str = "commit-rpm-managed";

/// Validated pending intent for a fresh RPM install.
#[derive(Debug, Clone)]
pub(crate) struct PendingRpmInstall {
    pub(crate) transaction: Transaction,
    pub(crate) component: String,
    pub(crate) package: String,
    pub(crate) install_step: usize,
    pub(crate) state_step: usize,
}

impl PendingRpmInstall {
    pub(crate) fn mark_install_done(&mut self, command: &str) -> Result<(), CliError> {
        if self.transaction.steps[self.install_step].status != TransactionStepStatus::Done {
            self.transaction
                .mark_done(self.install_step)
                .map_err(|err| journal_error(command, "record completed dnf install", err))?;
        }
        Ok(())
    }

    pub(crate) fn mark_state_done(&mut self, command: &str) -> Result<(), CliError> {
        if self.transaction.steps[self.state_step].status != TransactionStepStatus::Done {
            self.transaction
                .mark_done(self.state_step)
                .map_err(|err| journal_error(command, "record committed RPM state", err))?;
        }
        Ok(())
    }

    pub(crate) fn finish_ok(&mut self, command: &str) -> Result<(), CliError> {
        self.transaction
            .finish(TransactionOutcomeStatus::Ok)
            .map_err(|err| journal_error(command, "finish RPM install journal", err))
    }

    pub(crate) fn finish_partial(
        &mut self,
        failed_step: usize,
        reason: &str,
        command: &str,
    ) -> Result<(), CliError> {
        self.transaction
            .mark_failed(failed_step, reason)
            .map_err(|err| journal_error(command, "record incomplete RPM install", err))?;
        self.transaction
            .finish(TransactionOutcomeStatus::Partial)
            .map_err(|err| journal_error(command, "finish incomplete RPM install", err))
    }

    pub(crate) fn finish_failed(
        &mut self,
        failed_step: usize,
        reason: &str,
        command: &str,
    ) -> Result<(), CliError> {
        self.transaction
            .mark_failed(failed_step, reason)
            .map_err(|err| journal_error(command, "record failed RPM install", err))?;
        self.transaction
            .finish(TransactionOutcomeStatus::Failed)
            .map_err(|err| journal_error(command, "finish failed RPM install", err))
    }

    pub(crate) fn journal_update_failure_detail(&self, err: &CliError) -> String {
        format!(
            "recovery journal operation '{}' at '{}' could not be updated: {}; it may remain live (InFlight or Partial)",
            self.transaction.operation_id,
            self.transaction.journal_path.display(),
            err.reason()
        )
    }
}

pub(crate) fn journal_dir(layout: &FsLayout) -> PathBuf {
    layout.state_dir.join("journal")
}

pub(crate) fn begin_fresh_install(
    layout: &FsLayout,
    component: &str,
    package: &str,
    command: &str,
) -> Result<PendingRpmInstall, CliError> {
    let state_path = layout.state_dir.join("installed.toml");
    let mut transaction = Transaction::begin("install", state_path, &journal_dir(layout))
        .map_err(|err| journal_error(command, "create pending RPM install", err))?;
    // Component and package together define the recovery claim. Persist both
    // steps in one revision so a crash cannot expose a half-formed contract
    // that neither repair nor a later install can interpret safely.
    if let Err(err) = transaction.record_steps([
        TransactionStep::planned(INSTALL_PHASE, package, INSTALL_ACTION, None),
        TransactionStep::planned(STATE_PHASE, component, STATE_ACTION, None),
    ]) {
        let _ = transaction.finish(TransactionOutcomeStatus::Failed);
        return Err(journal_error(
            command,
            "record pending RPM install steps",
            err,
        ));
    }
    Ok(PendingRpmInstall {
        transaction,
        component: component.to_string(),
        package: package.to_string(),
        install_step: 0,
        state_step: 1,
    })
}

/// Find one live RPM claim matching a component or package alias.
pub(crate) fn find_pending_claim(
    layout: &FsLayout,
    state: &InstalledState,
    claims: &[&str],
    command: &str,
) -> Result<Option<PendingRpmInstall>, CliError> {
    let dir = journal_dir(layout);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "failed to scan RPM recovery journals in {}: {err}",
                    dir.display()
                ),
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to read an RPM recovery journal entry in {}: {err}",
                dir.display()
            ),
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
        let transaction = Transaction::load_journal(&path).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "cannot read recovery journal {}: {err}; automatic recovery is unsafe — inspect the journal and cross-check installed.toml with rpmdb before removing any recovery marker",
                path.display()
            ),
        })?;
        let Some(pending) = parse_pending(transaction, &path, layout, state, command)? else {
            continue;
        };
        if claims.is_empty()
            || claims
                .iter()
                .any(|claim| *claim == pending.component || *claim == pending.package)
        {
            matches.push(pending);
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => {
            let journals = matches
                .iter()
                .map(|pending| {
                    format!(
                        "{} (component '{}', package '{}', path {})",
                        pending.transaction.operation_id,
                        pending.component,
                        pending.package,
                        pending.transaction.journal_path.display()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "multiple pending RPM installs match '{}': {journals}; refusing to choose an owner automatically — verify each package in rpmdb and inspect the listed journals before removing any recovery marker",
                    claims.join("', '")
                ),
            })
        }
    }
}

pub(crate) fn reject_pending_claim(
    layout: &FsLayout,
    state: &InstalledState,
    claims: &[&str],
    command: &str,
) -> Result<(), CliError> {
    if let Some(pending) = find_pending_claim(layout, state, claims, command)? {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "a previous RPM install for component '{}' (package '{}') is pending recovery; run `anolisa repair {}` before retrying",
                pending.component, pending.package, pending.component
            ),
        });
    }
    Ok(())
}

pub(crate) fn state_claim_owner<'a>(
    state: &'a InstalledState,
    component: &str,
    package: &str,
) -> Option<&'a InstalledObject> {
    state.objects.iter().find(|object| {
        object.kind == ObjectKind::Component
            && (object.name == component
                || object.name == package
                || object
                    .rpm_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.package_name.trim() == package))
    })
}

fn parse_pending(
    transaction: Transaction,
    path: &Path,
    layout: &FsLayout,
    state: &InstalledState,
    command: &str,
) -> Result<Option<PendingRpmInstall>, CliError> {
    let install_steps = transaction
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.phase == INSTALL_PHASE || step.action == INSTALL_ACTION)
        .collect::<Vec<_>>();
    let state_steps = transaction
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.phase == STATE_PHASE || step.action == STATE_ACTION)
        .collect::<Vec<_>>();
    // `Transaction::begin` persists an empty revision before the initial step
    // batch. An interruption in that window is known to precede dnf, so the
    // empty journal owns nothing and is safe to ignore.
    if install_steps.is_empty() && state_steps.is_empty() {
        return Ok(None);
    }
    if state
        .operations
        .iter()
        .any(|operation| operation.id == transaction.operation_id && operation.status == "ok")
    {
        return Ok(None);
    }
    if !matches!(
        transaction.status,
        TransactionOutcomeStatus::InFlight | TransactionOutcomeStatus::Partial
    ) {
        return Ok(None);
    }
    if transaction.operation != "install"
        || install_steps.len() != 1
        || state_steps.len() != 1
        || install_steps[0].1.phase != INSTALL_PHASE
        || install_steps[0].1.action != INSTALL_ACTION
        || state_steps[0].1.phase != STATE_PHASE
        || state_steps[0].1.action != STATE_ACTION
        || install_steps[0].0 >= state_steps[0].0
        || install_steps[0].1.target.trim().is_empty()
        || !valid_component_name(state_steps[0].1.target.trim())
    {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "malformed live RPM recovery journal {} (operation '{}'); automatic recovery is unsafe — cross-check this operation in installed.toml and verify the package in rpmdb before removing or editing the recovery marker",
                path.display(),
                transaction.operation_id
            ),
        });
    }
    let expected_state = layout.state_dir.join("installed.toml");
    if transaction.state_path != expected_state || transaction.journal_path != path {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "pending RPM journal {} references an unexpected state or journal path",
                path.display()
            ),
        });
    }

    let component = state_steps[0].1.target.trim().to_string();
    let package = install_steps[0].1.target.trim().to_string();
    let install_step = install_steps[0].0;
    let state_step = state_steps[0].0;
    drop(install_steps);
    drop(state_steps);

    Ok(Some(PendingRpmInstall {
        component,
        package,
        install_step,
        state_step,
        transaction,
    }))
}

fn valid_component_name(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains('/')
        && !component.contains('\\')
}

pub(crate) fn fresh_rpm_object(
    component: &str,
    info: &PackageInfo,
    source_repo: Option<&str>,
    operation_id: &str,
    installed_at: &str,
) -> InstalledObject {
    let evr = info.version.to_string();
    InstalledObject {
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
        installed_at: installed_at.to_string(),
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
    }
}

pub(crate) fn install_operation(
    operation_id: &str,
    command: &str,
    started_at: &str,
    finished_at: String,
) -> OperationRecord {
    OperationRecord {
        id: operation_id.to_string(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at),
    }
}

pub(crate) fn journal_error(command: &str, action: &str, err: TransactionError) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to {action}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn layout() -> (tempfile::TempDir, FsLayout) {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        (tmp, layout)
    }

    #[test]
    fn claim_lookup_matches_component_and_package_alias() {
        let (_tmp, layout) = layout();
        let pending = begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin journal");

        for claim in ["cosh", "copilot-shell"] {
            let found = find_pending_claim(&layout, &InstalledState::default(), &[claim], "test")
                .expect("find claim")
                .expect("pending claim");
            assert_eq!(
                found.transaction.operation_id,
                pending.transaction.operation_id
            );
        }
    }

    #[test]
    fn claim_lookup_rejects_multiple_matching_journals() {
        let (_tmp, layout) = layout();
        let first = begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("first journal");
        let second = begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("second journal");

        let err = find_pending_claim(&layout, &InstalledState::default(), &["cosh"], "test")
            .expect_err("ambiguous claim must fail");
        assert!(err.reason().contains("multiple pending RPM installs"));
        assert!(err.reason().contains(&first.transaction.operation_id));
        assert!(err.reason().contains(&second.transaction.operation_id));
        assert!(err.reason().contains("verify each package in rpmdb"));
    }

    #[test]
    fn committed_operation_makes_malformed_stale_journal_ignorable() {
        let (_tmp, layout) = layout();
        let mut pending = begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin journal");
        pending.transaction.steps.pop();
        fs::write(
            &pending.transaction.journal_path,
            toml::to_string_pretty(&pending.transaction).expect("serialize journal"),
        )
        .expect("rewrite journal");
        let mut state = InstalledState::default();
        state.operations.push(OperationRecord {
            id: pending.transaction.operation_id,
            command: "install cosh".to_string(),
            status: "ok".to_string(),
            started_at: "2026-07-14T00:00:00Z".to_string(),
            finished_at: Some("2026-07-14T00:00:01Z".to_string()),
        });

        assert!(
            find_pending_claim(&layout, &state, &["cosh"], "test")
                .expect("scan stale journal")
                .is_none()
        );
    }

    #[test]
    fn malformed_live_journal_reports_safe_inspection_steps() {
        let (_tmp, layout) = layout();
        let mut pending = begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin journal");
        pending.transaction.steps.pop();
        fs::write(
            &pending.transaction.journal_path,
            toml::to_string_pretty(&pending.transaction).expect("serialize journal"),
        )
        .expect("rewrite journal");

        let err = find_pending_claim(&layout, &InstalledState::default(), &["cosh"], "test")
            .expect_err("live malformed journal must fail closed");
        assert!(err.reason().contains(&pending.transaction.operation_id));
        assert!(err.reason().contains("installed.toml"));
        assert!(err.reason().contains("rpmdb"));
        assert!(err.reason().contains("before removing or editing"));
    }
}
