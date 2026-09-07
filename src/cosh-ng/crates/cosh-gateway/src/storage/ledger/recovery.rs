impl SqliteTaskStore {

    /// Recovers stale brokered executions after a Run-lease generation takeover.
    ///
    /// The current unexpired lease must have a newer generation than every
    /// recovered execution. Repeated calls are read-only because only exact
    /// `Claimed` and `Started` states are eligible.
    pub fn recover_brokered_executions_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<BrokeredExecutionRecoveryReport, StoreError> {
        let transaction = immediate(self)?;
        let current = load_run_lease_optional(&transaction, run_id)?
            .ok_or_else(|| not_found("run lease", run_id.as_str()))?;
        if current.expires_at_ms <= now_ms {
            return Err(conflict(
                "brokered execution recovery requires an unexpired takeover lease",
            ));
        }
        require_task_run(
            &transaction,
            &current.task_id,
            &current.run_id,
            &current.actor_id,
        )?;
        let claimed = load_brokered_recovery_candidates_for_run(
            &transaction,
            run_id,
            "claimed",
            ExecutionState::Planned,
            current.generation,
        )?;
        let started = load_brokered_recovery_candidates_for_run(
            &transaction,
            run_id,
            "started",
            ExecutionState::Started,
            current.generation,
        )?;
        let report = apply_brokered_execution_recovery(&transaction, &claimed, &started, now_ms)?;
        transaction.commit()?;
        Ok(report)
    }

    /// Recovers durable state conservatively without retrying side effects.
    pub fn recover_gateway(&mut self, now_ms: u64) -> Result<RecoveryReport, StoreError> {
        let transaction = immediate(self)?;
        let now = integer(now_ms, "recovery timestamp")?;
        validate_all_execution_receipts(&transaction)?;
        let claimed =
            load_brokered_recovery_candidates(&transaction, "claimed", ExecutionState::Planned)?;
        let started =
            load_brokered_recovery_candidates(&transaction, "started", ExecutionState::Started)?;
        let execution_recovery =
            apply_brokered_execution_recovery(&transaction, &claimed, &started, now_ms)?;
        let (runtime_input_requests_cancelled, runtime_input_dispatches_unknown) =
            recover_runtime_inputs_after_restart(&transaction, now_ms)?;
        let approvals_expired = transaction.execute(
            "UPDATE approvals SET state='expired', revision=revision+1, updated_at_ms=?1
             WHERE state='pending' AND expires_at_ms <= ?1",
            params![now],
        )?;
        let approvals_cancelled = transaction.execute(
            "UPDATE approvals SET state='cancelled', revision=revision+1, updated_at_ms=?1
             WHERE state='pending'",
            params![now],
        )?;
        let permission_dispatches_unknown = transaction.execute(
            "UPDATE provider_permission_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?1
             WHERE state IN ('prepared', 'started')",
            params![now],
        )?;
        let brokered_dispatches_unknown = transaction.execute(
            "UPDATE brokered_runtime_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?1
             WHERE state='started'",
            params![now],
        )?;
        let permits_expired = transaction.execute(
            "UPDATE permits SET state='expired' WHERE state='issued' AND valid_until_ms <= ?1",
            params![now],
        )?;
        let legacy_executions_uncertain = transaction.execute(
            "UPDATE executions SET state='uncertain', revision=revision+1, completed_at_ms=?1,
             updated_at_ms=?1 WHERE state='started' AND broker_state IS NULL",
            params![now],
        )?;
        let runtime_bindings_lost = transaction.execute(
            "UPDATE runtime_bindings SET state='lost', updated_at_ms=?1 WHERE state='active'",
            params![now],
        )?;
        transaction.commit()?;
        Ok(RecoveryReport {
            approvals_expired: approvals_expired as u64,
            approvals_cancelled: approvals_cancelled as u64,
            permission_dispatches_unknown: permission_dispatches_unknown as u64,
            brokered_dispatches_unknown: brokered_dispatches_unknown as u64,
            runtime_input_requests_cancelled,
            runtime_input_dispatches_unknown,
            permits_expired: permits_expired as u64,
            executions_uncertain: execution_recovery.executions_uncertain
                + legacy_executions_uncertain as u64,
            executions_known_no_effect: execution_recovery.executions_known_no_effect,
            runtime_bindings_lost: runtime_bindings_lost as u64,
        })
    }
}
