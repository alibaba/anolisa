//! Durable intent and recovery helpers for fresh delegated RPM installs.

use std::fs;
use std::path::{Path, PathBuf};

use anolisa_core::state::OperationRecord;
use anolisa_core::transaction::{
    Transaction, TransactionError, TransactionOutcomeStatus, TransactionStepStatus,
};
use anolisa_platform::fs_layout::FsLayout;

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
}

pub(crate) fn journal_dir(layout: &FsLayout) -> PathBuf {
    layout.state_dir.join("journal")
}

// Test-only since the delegated install moved to the planner pipeline's
// subject journals: tests use this to fabricate the legacy two-step journal
// shape that repair's R1 recovery still consumes from disk.
#[cfg(test)]
pub(crate) fn begin_fresh_install(
    layout: &FsLayout,
    component: &str,
    package: &str,
    command: &str,
) -> Result<PendingRpmInstall, CliError> {
    use anolisa_core::transaction::TransactionStep;

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
///
/// `operations` is the operation history the claims are checked against; the
/// v4 `InstalledState` and the v5 `StateStore` carry the same record shape,
/// so callers on either model pass their history slice.
pub(crate) fn find_pending_claim(
    layout: &FsLayout,
    operations: &[OperationRecord],
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
        let Some(pending) = parse_pending(transaction, &path, layout, operations, command)? else {
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
    operations: &[OperationRecord],
    claims: &[&str],
    command: &str,
) -> Result<(), CliError> {
    if let Some(pending) = find_pending_claim(layout, operations, claims, command)? {
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

/// Interpret one journal as a legacy pending RPM install claim.
///
/// `operations` is the operation history the claim is checked against — the
/// v4 `InstalledState` and the v5 `StateStore` both carry the same record
/// shape, so callers on either model pass their history slice. Returns
/// `Ok(None)` for journals that are not this shape (settled, already
/// committed, or written by another pipeline); a *live* journal that matches
/// this shape but is malformed fails closed.
pub(crate) fn parse_pending(
    transaction: Transaction,
    path: &Path,
    layout: &FsLayout,
    operations: &[OperationRecord],
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
    if operations
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
            let found = find_pending_claim(&layout, &[], &[claim], "test")
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

        let err = find_pending_claim(&layout, &[], &["cosh"], "test")
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
        let operations = vec![OperationRecord {
            id: pending.transaction.operation_id,
            command: "install cosh".to_string(),
            status: "ok".to_string(),
            started_at: "2026-07-14T00:00:00Z".to_string(),
            finished_at: Some("2026-07-14T00:00:01Z".to_string()),
            parent_operation_id: None,
        }];

        assert!(
            find_pending_claim(&layout, &operations, &["cosh"], "test")
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

        let err = find_pending_claim(&layout, &[], &["cosh"], "test")
            .expect_err("live malformed journal must fail closed");
        assert!(err.reason().contains(&pending.transaction.operation_id));
        assert!(err.reason().contains("installed.toml"));
        assert!(err.reason().contains("rpmdb"));
        assert!(err.reason().contains("before removing or editing"));
    }
}
