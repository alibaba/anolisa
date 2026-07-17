//! Forward-only executor for the delegated step family.
//!
//! Interprets the delegated subset of the planner's [`Step`] vocabulary —
//! [`Step::NativeTransaction`], [`Step::Observe`], [`Step::WriteRecord`],
//! [`Step::DropRecord`] — against a [`DelegatedProvider`], journaling every
//! step. The failure semantics differ from the owned family on purpose: a
//! native transaction that ran is never undone. On failure the executor
//! re-observes the native authority, records what it saw, and returns; the
//! way forward is a new plan over the fresh facts, not compensation.
//!
//! Owned-family steps (file placement, services, hooks) and
//! [`Step::RecoverJournal`] are rejected up front as [`ExecutionError::UnsupportedStep`]
//! — they belong to the owned executor and the repair flow respectively.

use thiserror::Error;

use crate::domain::Observation;
use crate::planner::{NativeProbe, RecordWrite, Step};
use crate::providers::{DelegatedProvider, ProviderError};
use crate::transaction::{
    Transaction, TransactionError, TransactionOutcomeStatus, TransactionStep,
};

/// Journal phase label for native package-manager transactions.
pub const PHASE_NATIVE_TXN: &str = "delegated-txn";
/// Journal phase label for post-transaction observation.
pub const PHASE_OBSERVE: &str = "delegated-observe";
/// Journal phase label for state-record commits.
pub const PHASE_RECORD: &str = "delegated-record";

/// Persistence port for the final record commit. The executor decides *when*
/// a record is written or dropped; the store behind this trait decides *how*
/// (today `installed.toml`, injected by the CLI).
pub trait RecordSink {
    /// Persist the installation record described by `write`, absorbing the
    /// freshest `observation` when one was taken this run.
    fn write_record(
        &mut self,
        write: RecordWrite,
        observation: Option<&Observation>,
    ) -> Result<(), RecordSinkError>;

    /// Remove the installation record.
    fn drop_record(&mut self) -> Result<(), RecordSinkError>;
}

/// Opaque record-store failure. The executor only needs to know that the
/// commit did not happen; the store's own error carries the detail.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct RecordSinkError(pub String);

/// What execution left behind on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutcome {
    /// Freshest observation taken by an [`Step::Observe`] step, if the plan
    /// contained one. Already absorbed into the record by the sink.
    pub observation: Option<Observation>,
}

/// How execution of a delegated plan failed.
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// The plan contains a step this executor does not interpret (owned
    /// family or journal recovery). Rejected before any side effect runs.
    #[error("delegated executor cannot run step {step:?}")]
    UnsupportedStep {
        /// The offending step.
        step: Step,
    },
    /// The native transaction failed. Forward-only: nothing was undone;
    /// `reobserved` carries the post-failure native facts per package so the
    /// caller can render reality and plan again.
    #[error("native transaction failed: {source}")]
    TransactionFailed {
        /// The provider failure.
        source: ProviderError,
        /// Post-failure probe per package (best-effort: probes that
        /// themselves failed are omitted).
        reobserved: Vec<(String, NativeProbe)>,
    },
    /// A post-transaction probe contradicts the plan's expectation — the
    /// package should be present exactly once but is not.
    #[error("post-transaction observation contradicts the plan: {package} is {found}")]
    FactsChanged {
        /// Package whose probe surprised us.
        package: String,
        /// What the probe found instead (`absent`, `multiple installed
        /// versions`).
        found: &'static str,
    },
    /// An observation step failed outright (query error).
    #[error("native observation failed: {0}")]
    ObserveFailed(#[source] ProviderError),
    /// The record commit failed after the native transaction succeeded. The
    /// package state is real but untracked; the journal is left `Partial`
    /// and `repair` reconciles.
    #[error("record commit failed: {0}")]
    RecordCommitFailed(#[source] RecordSinkError),
    /// The journal itself could not be persisted.
    #[error(transparent)]
    Journal(#[from] TransactionError),
}

/// Execute the delegated steps of a plan in order, journaling each one.
///
/// `journal` must already be begun by the caller (it owns the operation id
/// and the state snapshot); the executor appends its steps after whatever
/// the journal already holds. `observed_at` stamps every observation taken
/// during this run — the caller owns the clock.
///
/// On any failure the journal is finished as `Failed` — or `Partial` once a
/// native transaction has committed, since real side effects then exist that
/// the record does not reflect.
pub fn execute_delegated_steps(
    steps: &[Step],
    provider: &DelegatedProvider<'_>,
    sink: &mut dyn RecordSink,
    journal: &mut Transaction,
    observed_at: &str,
) -> Result<ExecutionOutcome, ExecutionError> {
    execute_delegated_steps_resumed(steps, provider, sink, journal, observed_at, false)
}

/// Like [`execute_delegated_steps`], for plans whose native transaction has
/// already committed outside this call.
///
/// A merged batch runs one native transaction covering several components,
/// then each component's remaining steps (observe, record) go through here
/// with `native_txn_committed: true`. The flag seeds the failure
/// classification: real side effects already exist on the host, so any
/// failure finishes the journal as `Partial`, never as a clean `Failed`.
pub fn execute_delegated_steps_resumed(
    steps: &[Step],
    provider: &DelegatedProvider<'_>,
    sink: &mut dyn RecordSink,
    journal: &mut Transaction,
    observed_at: &str,
    native_txn_committed: bool,
) -> Result<ExecutionOutcome, ExecutionError> {
    // Reject foreign steps before any side effect or journal write: a plan
    // that mixes families reaches the wrong executor, and half-running it
    // would corrupt the journal's story.
    if let Some(step) = steps.iter().find(|step| !is_delegated_step(step)) {
        return Err(ExecutionError::UnsupportedStep { step: step.clone() });
    }

    let base = journal.steps.len();
    journal.record_steps(steps.iter().map(journal_step))?;

    let mut observation: Option<Observation> = None;
    let mut txn_ran = native_txn_committed;
    // Journal status once side effects may exist: before the native
    // transaction commits a failure is clean (`Failed`), after it the record
    // no longer matches reality (`Partial`).
    let fail_status = |txn_ran: bool| {
        if txn_ran {
            TransactionOutcomeStatus::Partial
        } else {
            TransactionOutcomeStatus::Failed
        }
    };

    for (offset, step) in steps.iter().enumerate() {
        let idx = base + offset;
        match step {
            Step::NativeTransaction {
                action, packages, ..
            } => match provider.transact(*action, packages) {
                Ok(()) => {
                    journal.mark_done(idx)?;
                    txn_ran = true;
                }
                Err(source) => {
                    journal.mark_failed(idx, &source.to_string())?;
                    journal.finish(fail_status(txn_ran))?;
                    // Forward-only: re-observe instead of undoing. A probe
                    // that fails too is dropped — this is diagnostics, not
                    // a second chance to fail.
                    let reobserved = packages
                        .iter()
                        .filter_map(|package| {
                            provider
                                .observe(package, observed_at)
                                .ok()
                                .map(|probe| (package.clone(), probe))
                        })
                        .collect();
                    return Err(ExecutionError::TransactionFailed { source, reobserved });
                }
            },
            Step::Observe { packages } => {
                for package in packages {
                    match provider.observe(package, observed_at) {
                        Ok(NativeProbe::Present {
                            observation: fresh, ..
                        }) => observation = Some(fresh),
                        Ok(probe) => {
                            let found = match probe {
                                NativeProbe::MultipleVersions { .. } => {
                                    "multiple installed versions"
                                }
                                _ => "absent",
                            };
                            journal.mark_failed(idx, &format!("{package} is {found}"))?;
                            journal.finish(fail_status(txn_ran))?;
                            return Err(ExecutionError::FactsChanged {
                                package: package.clone(),
                                found,
                            });
                        }
                        Err(err) => {
                            journal.mark_failed(idx, &err.to_string())?;
                            journal.finish(fail_status(txn_ran))?;
                            return Err(ExecutionError::ObserveFailed(err));
                        }
                    }
                }
                journal.mark_done(idx)?;
            }
            Step::WriteRecord(write) => match sink.write_record(*write, observation.as_ref()) {
                Ok(()) => journal.mark_done(idx)?,
                Err(err) => {
                    journal.mark_failed(idx, &err.to_string())?;
                    journal.finish(fail_status(txn_ran))?;
                    return Err(ExecutionError::RecordCommitFailed(err));
                }
            },
            Step::DropRecord => match sink.drop_record() {
                Ok(()) => journal.mark_done(idx)?,
                Err(err) => {
                    journal.mark_failed(idx, &err.to_string())?;
                    journal.finish(fail_status(txn_ran))?;
                    return Err(ExecutionError::RecordCommitFailed(err));
                }
            },
            // Unreachable: the pre-validation pass rejected foreign steps.
            other => {
                return Err(ExecutionError::UnsupportedStep {
                    step: other.clone(),
                });
            }
        }
    }

    journal.finish(TransactionOutcomeStatus::Ok)?;
    Ok(ExecutionOutcome { observation })
}

/// Whether this executor interprets `step`.
fn is_delegated_step(step: &Step) -> bool {
    matches!(
        step,
        Step::NativeTransaction { .. }
            | Step::Observe { .. }
            | Step::WriteRecord(_)
            | Step::DropRecord
    )
}

/// Journal entry for a delegated step, initialised to `Planned`.
fn journal_step(step: &Step) -> TransactionStep {
    match step {
        Step::NativeTransaction {
            action, packages, ..
        } => TransactionStep::planned(PHASE_NATIVE_TXN, packages.join(","), action.verb(), None),
        Step::Observe { packages } => {
            TransactionStep::planned(PHASE_OBSERVE, packages.join(","), "observe", None)
        }
        Step::WriteRecord(write) => {
            TransactionStep::planned(PHASE_RECORD, "state", write.label(), None)
        }
        Step::DropRecord => TransactionStep::planned(PHASE_RECORD, "state", "drop-record", None),
        // Foreign steps never reach journaling; give them an honest label
        // anyway so a future bug is visible in the journal, not hidden.
        other => TransactionStep::planned("unsupported", format!("{other:?}"), "unsupported", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NativePm;
    use crate::planner::NativeAction;
    use crate::providers::test_fakes::{FakeQuery, FakeTxn, InstalledOutcome, pkg_info};
    use crate::transaction::TransactionStepStatus;

    const NOW: &str = "2026-07-16T00:00:00Z";

    /// In-memory sink recording commits; optionally failing.
    #[derive(Default)]
    struct MemSink {
        writes: Vec<(RecordWrite, Option<Observation>)>,
        drops: usize,
        fail: bool,
    }

    impl RecordSink for MemSink {
        fn write_record(
            &mut self,
            write: RecordWrite,
            observation: Option<&Observation>,
        ) -> Result<(), RecordSinkError> {
            if self.fail {
                return Err(RecordSinkError("state file locked".to_string()));
            }
            self.writes.push((write, observation.cloned()));
            Ok(())
        }

        fn drop_record(&mut self) -> Result<(), RecordSinkError> {
            if self.fail {
                return Err(RecordSinkError("state file locked".to_string()));
            }
            self.drops += 1;
            Ok(())
        }
    }

    fn journal(dir: &std::path::Path) -> Transaction {
        Transaction::begin("install", dir.join("installed.toml"), dir).expect("begin journal")
    }

    fn query_present(package: &str, version: &str) -> FakeQuery {
        let mut q = FakeQuery::default();
        q.installed.insert(
            package.to_string(),
            InstalledOutcome::Present(pkg_info(package, version, Some("1.al4"), "x86_64")),
        );
        q
    }

    fn install_steps(package: &str) -> Vec<Step> {
        vec![
            Step::NativeTransaction {
                pm: NativePm::Rpm,
                action: NativeAction::Install,
                packages: vec![package.to_string()],
            },
            Step::Observe {
                packages: vec![package.to_string()],
            },
            Step::WriteRecord(RecordWrite::DelegatedManaged),
        ]
    }

    #[test]
    fn delegated_install_happy_path_runs_txn_observe_write() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let outcome = execute_delegated_steps(
            &install_steps("cosh"),
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .expect("execution ok");

        // The dnf call ran, the sink got the fresh observation, and the
        // returned outcome carries the same observation.
        assert_eq!(
            txn.calls.borrow().as_slice(),
            &[("install".to_string(), "cosh".to_string())]
        );
        assert_eq!(sink.writes.len(), 1);
        let (write, observation) = &sink.writes[0];
        assert_eq!(*write, RecordWrite::DelegatedManaged);
        let observation = observation.as_ref().expect("observation absorbed");
        assert_eq!(observation.evr.as_deref(), Some("2.7.0-1.al4"));
        assert_eq!(outcome.observation.as_ref(), Some(observation));

        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
        assert_eq!(journal.steps.len(), 3);
        assert!(
            journal
                .steps
                .iter()
                .all(|s| s.status == TransactionStepStatus::Done)
        );
        assert_eq!(journal.steps[0].phase, PHASE_NATIVE_TXN);
        assert_eq!(journal.steps[0].action, "install");
    }

    #[test]
    fn txn_failure_is_forward_only_and_reobserves() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // dnf fails; the package stays absent on re-observe.
        let mut query = FakeQuery::default();
        query
            .installed
            .insert("cosh".to_string(), InstalledOutcome::Absent);
        let txn = FakeTxn {
            fail: vec!["install"],
            ..FakeTxn::default()
        };
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let err = execute_delegated_steps(
            &install_steps("cosh"),
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .unwrap_err();

        match err {
            ExecutionError::TransactionFailed { reobserved, .. } => {
                assert_eq!(reobserved, vec![("cosh".to_string(), NativeProbe::Absent)]);
            }
            other => panic!("expected TransactionFailed, got {other:?}"),
        }
        // Forward-only: exactly the one failed call, no compensating remove.
        assert_eq!(txn.calls.borrow().len(), 1);
        assert!(sink.writes.is_empty());
        assert_eq!(journal.status, TransactionOutcomeStatus::Failed);
        assert_eq!(journal.steps[0].status, TransactionStepStatus::Failed);
        // The remaining steps never ran and stay planned.
        assert_eq!(journal.steps[1].status, TransactionStepStatus::Planned);
    }

    #[test]
    fn record_commit_failure_after_txn_leaves_partial_journal() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink {
            fail: true,
            ..MemSink::default()
        };
        let mut journal = journal(tmp.path());

        let err = execute_delegated_steps(
            &install_steps("cosh"),
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .unwrap_err();

        assert!(matches!(err, ExecutionError::RecordCommitFailed(_)));
        // dnf committed but the record did not: Partial, not Failed — the
        // pending journal then routes the next intent to repair.
        assert_eq!(journal.status, TransactionOutcomeStatus::Partial);
        assert_eq!(journal.steps[0].status, TransactionStepStatus::Done);
        assert_eq!(journal.steps[2].status, TransactionStepStatus::Failed);
    }

    #[test]
    fn post_txn_absence_is_facts_changed_partial() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // dnf reports success but the probe cannot see the package.
        let mut query = FakeQuery::default();
        query
            .installed
            .insert("cosh".to_string(), InstalledOutcome::Absent);
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let err = execute_delegated_steps(
            &install_steps("cosh"),
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .unwrap_err();

        match err {
            ExecutionError::FactsChanged { package, found } => {
                assert_eq!(package, "cosh");
                assert_eq!(found, "absent");
            }
            other => panic!("expected FactsChanged, got {other:?}"),
        }
        assert!(sink.writes.is_empty());
        assert_eq!(journal.status, TransactionOutcomeStatus::Partial);
    }

    #[test]
    fn delegated_remove_drops_record_after_txn() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = FakeQuery::default();
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let steps = vec![
            Step::NativeTransaction {
                pm: NativePm::Rpm,
                action: NativeAction::Remove,
                packages: vec!["cosh".to_string()],
            },
            Step::DropRecord,
        ];
        execute_delegated_steps(&steps, &provider, &mut sink, &mut journal, NOW)
            .expect("execution ok");

        assert_eq!(
            txn.calls.borrow().as_slice(),
            &[("remove".to_string(), "cosh".to_string())]
        );
        assert_eq!(sink.drops, 1);
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
    }

    #[test]
    fn record_only_plan_needs_no_native_calls() {
        // X3/X4: the package is already gone or stays by design; the plan is
        // just DropRecord.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = FakeQuery::default();
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        execute_delegated_steps(&[Step::DropRecord], &provider, &mut sink, &mut journal, NOW)
            .expect("execution ok");

        assert!(txn.calls.borrow().is_empty());
        assert_eq!(sink.drops, 1);
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
    }

    #[test]
    fn adopt_plan_observes_then_writes_adopted() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let steps = vec![
            Step::Observe {
                packages: vec!["cosh".to_string()],
            },
            Step::WriteRecord(RecordWrite::DelegatedAdopted),
        ];
        execute_delegated_steps(&steps, &provider, &mut sink, &mut journal, NOW)
            .expect("execution ok");

        assert!(txn.calls.borrow().is_empty());
        let (write, observation) = &sink.writes[0];
        assert_eq!(*write, RecordWrite::DelegatedAdopted);
        assert!(observation.is_some());
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
    }

    #[test]
    fn reinstall_verb_reaches_the_backend() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let steps = vec![
            Step::NativeTransaction {
                pm: NativePm::Rpm,
                action: NativeAction::Reinstall,
                packages: vec!["cosh".to_string()],
            },
            Step::Observe {
                packages: vec!["cosh".to_string()],
            },
            Step::WriteRecord(RecordWrite::RefreshObservation),
        ];
        execute_delegated_steps(&steps, &provider, &mut sink, &mut journal, NOW)
            .expect("execution ok");

        assert_eq!(
            txn.calls.borrow().as_slice(),
            &[("reinstall".to_string(), "cosh".to_string())]
        );
        assert_eq!(journal.steps[0].action, "reinstall");
    }

    #[test]
    fn resumed_failure_after_external_txn_is_partial_not_failed() {
        // A merged batch already committed the native transaction before the
        // per-component tail runs; a failure here must not report a clean
        // `Failed` journal — side effects exist that the record does not
        // reflect.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let mut query = FakeQuery::default();
        query
            .installed
            .insert("cosh".to_string(), InstalledOutcome::Absent);
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let steps = vec![
            Step::Observe {
                packages: vec!["cosh".to_string()],
            },
            Step::WriteRecord(RecordWrite::DelegatedManaged),
        ];
        let err =
            execute_delegated_steps_resumed(&steps, &provider, &mut sink, &mut journal, NOW, true)
                .unwrap_err();

        assert!(matches!(err, ExecutionError::FactsChanged { .. }));
        assert_eq!(journal.status, TransactionOutcomeStatus::Partial);
    }

    #[test]
    fn resumed_happy_tail_records_the_observation() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let steps = vec![
            Step::Observe {
                packages: vec!["cosh".to_string()],
            },
            Step::WriteRecord(RecordWrite::DelegatedManaged),
        ];
        let outcome =
            execute_delegated_steps_resumed(&steps, &provider, &mut sink, &mut journal, NOW, true)
                .expect("execution ok");

        // No native call from the tail itself; the record absorbed the fresh
        // observation and the journal closed clean.
        assert!(txn.calls.borrow().is_empty());
        assert_eq!(sink.writes.len(), 1);
        assert!(outcome.observation.is_some());
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
    }

    #[test]
    fn owned_steps_are_rejected_before_any_side_effect() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        // A mixed plan reaching this executor is a routing bug; the native
        // transaction that precedes the owned step must NOT run.
        let steps = vec![
            Step::NativeTransaction {
                pm: NativePm::Rpm,
                action: NativeAction::Install,
                packages: vec!["cosh".to_string()],
            },
            Step::PlaceFiles,
        ];
        let err =
            execute_delegated_steps(&steps, &provider, &mut sink, &mut journal, NOW).unwrap_err();

        assert!(matches!(
            err,
            ExecutionError::UnsupportedStep {
                step: Step::PlaceFiles
            }
        ));
        assert!(txn.calls.borrow().is_empty());
        assert!(journal.steps.is_empty());
        assert_eq!(journal.status, TransactionOutcomeStatus::InFlight);
    }

    #[test]
    fn recover_journal_is_not_this_executors_job() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = FakeQuery::default();
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());

        let err = execute_delegated_steps(
            &[Step::RecoverJournal],
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ExecutionError::UnsupportedStep {
                step: Step::RecoverJournal
            }
        ));
    }

    #[test]
    fn executor_appends_after_existing_journal_steps() {
        // The caller may have journaled its own pre-steps (e.g. lock
        // acquisition); executor indices must not clobber them.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let query = query_present("cosh", "2.7.0");
        let txn = FakeTxn::default();
        let provider = DelegatedProvider::new(&query, &txn);
        let mut sink = MemSink::default();
        let mut journal = journal(tmp.path());
        journal
            .record_step(TransactionStep::planned("pre", "lock", "acquire", None))
            .expect("record pre-step");
        journal.mark_done(0).expect("mark pre-step");

        execute_delegated_steps(
            &install_steps("cosh"),
            &provider,
            &mut sink,
            &mut journal,
            NOW,
        )
        .expect("execution ok");

        assert_eq!(journal.steps.len(), 4);
        assert_eq!(journal.steps[0].phase, "pre");
        assert!(
            journal
                .steps
                .iter()
                .all(|s| s.status == TransactionStepStatus::Done)
        );
    }
}
