//! Atomic Task event, projection, receipt, and Outbox persistence.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use cosh_gateway_contracts::common::{
    BoundedName, BoundedOpaque, ContractHeader, ContractSchema, Correlation, Digest, IdempotencyKey,
};
use cosh_gateway_contracts::ids::{ActorId, DeliveryId, MessageId, RunId, TaskId};
use cosh_gateway_contracts::task::{
    CancelReason, CancellationStage, TaskEvent, TaskEventEnvelope, TaskState,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::task::TaskAggregate;

use super::{SqliteTaskStore, StoreError};

pub(crate) const MAX_TASK_EVENTS_PER_COMMIT: usize = 64;
pub(crate) const MAX_OUTBOX_INTENTS_PER_COMMIT: usize = 64;
pub(crate) const MAX_TASK_PAYLOAD_BYTES: usize = 256 * 1024;
pub(crate) const MAX_TASK_COMMIT_SERIALIZED_BYTES: usize = 1024 * 1024;

/// One durable delivery intent created by a Task event transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutboxIntent {
    /// Stable identity used to deduplicate downstream delivery.
    pub delivery_id: DeliveryId,
    /// Event in the same commit that caused this delivery.
    pub event_id: MessageId,
    /// Stable bounded delivery route.
    pub delivery_kind: BoundedName,
    /// Versioned delivery payload.
    pub payload: serde_json::Value,
    /// Earliest delivery attempt time in Unix milliseconds.
    pub next_attempt_at_ms: u64,
}

/// Fenced claim for one durable Outbox delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxClaim {
    /// Stable delivery identity.
    pub delivery_id: DeliveryId,
    /// Task that caused the delivery.
    pub task_id: TaskId,
    /// Event that caused the delivery.
    pub event_id: MessageId,
    /// Stable delivery route.
    pub delivery_kind: BoundedName,
    /// Versioned delivery payload.
    pub payload: serde_json::Value,
    /// Monotonic delivery attempt used to fence a stale worker.
    pub attempt: u64,
    /// Worker holding this delivery lease.
    pub lease_owner: BoundedOpaque,
    /// Delivery lease deadline in Unix milliseconds.
    pub lease_expires_at_ms: u64,
}

/// Complete unit of work admitted by the single Task writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCommit {
    /// Authenticated actor that owns the replay namespace.
    pub actor_id: ActorId,
    /// Caller-scoped command replay key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical digest of the admitted command.
    pub command_digest: Digest,
    /// Optional optimistic revision precondition.
    pub expected_revision: Option<u64>,
    /// Consecutive Task events produced by the command.
    pub events: Vec<TaskEventEnvelope>,
    /// Delivery intents caused by events in this commit.
    pub outbox: Vec<OutboxIntent>,
    /// Durable commit timestamp in Unix milliseconds.
    pub committed_at_ms: u64,
}

/// Stable response persisted for exact idempotent replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitReceipt {
    /// Task changed by the command.
    pub task_id: TaskId,
    /// Latest Task revision after the command.
    pub revision: u64,
    /// Task event identities committed by the command.
    pub event_ids: Vec<MessageId>,
    /// Outbox identities committed by the command.
    pub delivery_ids: Vec<DeliveryId>,
}

/// Result of admitting a command at the durable writer boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The command produced a new atomic commit.
    Applied(CommitReceipt),
    /// The same actor, key, and digest returned its durable receipt.
    Replayed(CommitReceipt),
}

impl SqliteTaskStore {
    pub(super) fn settle_legacy_runtime_start_recoveries(&mut self) -> Result<(), StoreError> {
        let candidates = self.legacy_runtime_start_recovery_candidates()?;
        for (task_id, run_id) in candidates {
            self.settle_legacy_runtime_start_recovery(&task_id, &run_id)?;
        }
        Ok(())
    }

    fn legacy_runtime_start_recovery_candidates(&self) -> Result<Vec<(TaskId, RunId)>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT task_id, run_id
             FROM legacy_runtime_start_recoveries
             WHERE state = 'pending'
             ORDER BY detected_at_ms, task_id",
        )?;
        let candidates = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (task_id, run_id) = row?;
                Ok((
                    TaskId::parse(task_id).map_err(|error| {
                        corrupt(&format!("invalid legacy recovery Task identity: {error}"))
                    })?,
                    RunId::parse(run_id).map_err(|error| {
                        corrupt(&format!("invalid legacy recovery Run identity: {error}"))
                    })?,
                ))
            })
            .collect();
        candidates
    }

    fn settle_legacy_runtime_start_recovery(
        &mut self,
        task_id: &TaskId,
        run_id: &RunId,
    ) -> Result<(), StoreError> {
        let now_ms = current_time_ms()?;
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let marker_state = transaction
            .query_row(
                "SELECT state FROM legacy_runtime_start_recoveries
                 WHERE task_id=?1 AND run_id=?2",
                params![task_id.as_str(), run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if marker_state.as_deref() != Some("pending") {
            return Err(corrupt(
                "legacy Runtime recovery marker lost its pending precondition",
            ));
        }
        let task =
            load_verified_projection(&transaction, task_id)?.ok_or(StoreError::TaskNotFound)?;
        if task.state() != TaskState::Queued || task.active_run_id() != Some(run_id) {
            return Err(corrupt(
                "legacy Runtime recovery marker no longer matches its queued Task",
            ));
        }

        let existing_events = load_events(&transaction, task_id)?;
        let previous = existing_events
            .last()
            .ok_or_else(|| corrupt("legacy queued Task has no immutable history"))?;
        let mut correlation = Correlation::new(previous.header.correlation.installation_id.clone());
        correlation.actor_id = Some(task.owner_actor_id().clone());
        correlation.task_id = Some(task_id.clone());
        correlation.run_id = Some(run_id.clone());
        correlation.causation_message_id = Some(previous.header.message_id.clone());
        let cancellation_revision = task
            .revision()
            .checked_add(1)
            .ok_or_else(|| corrupt("legacy recovery Task revision overflow"))?;
        let run_cancelled_revision = task
            .revision()
            .checked_add(2)
            .ok_or_else(|| corrupt("legacy recovery Task revision overflow"))?;
        let task_cancelled_revision = task
            .revision()
            .checked_add(3)
            .ok_or_else(|| corrupt("legacy recovery Task revision overflow"))?;

        let cancellation_requested = migration_event(
            task_id,
            cancellation_revision,
            now_ms,
            correlation.clone(),
            TaskEvent::CancellationRequested {
                run_id: run_id.clone(),
                cause: CancelReason::RuntimeShutdown,
            },
        );
        correlation.causation_message_id = Some(cancellation_requested.header.message_id.clone());
        let run_cancelled = migration_event(
            task_id,
            run_cancelled_revision,
            now_ms,
            correlation.clone(),
            TaskEvent::RunCancelled {
                run_id: run_id.clone(),
                stage: CancellationStage::BeforeRuntime,
            },
        );
        correlation.causation_message_id = Some(run_cancelled.header.message_id.clone());
        let task_cancelled = migration_event(
            task_id,
            task_cancelled_revision,
            now_ms,
            correlation,
            TaskEvent::TaskCancelled,
        );
        let events = vec![cancellation_requested, run_cancelled, task_cancelled];
        let aggregate = reduce_commit(Some(task.clone()), &events)?;
        persist_projection(&transaction, &aggregate, task.revision(), now_ms)?;
        append_events(&transaction, &events)?;
        let settlement_digest = legacy_recovery_digest(task_id, run_id)?;
        let event_ids = events
            .iter()
            .map(|event| event.header.message_id.clone())
            .collect::<Vec<_>>();
        let changed = transaction.execute(
            "UPDATE legacy_runtime_start_recoveries
             SET state='settled', settled_revision=?3,
                 settled_at_ms=MAX(?4, detected_at_ms), settlement_digest=?5,
                 settlement_event_ids_json=?6
             WHERE task_id=?1 AND run_id=?2 AND state='pending'",
            params![
                task_id.as_str(),
                run_id.as_str(),
                sqlite_integer(aggregate.revision(), "legacy recovery revision")?,
                sqlite_integer(now_ms, "legacy recovery timestamp")?,
                settlement_digest.as_str(),
                serde_json::to_string(&event_ids)?,
            ],
        )?;
        if changed != 1 {
            return Err(corrupt(
                "legacy Runtime recovery marker lost its settlement precondition",
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    /// Claims the oldest ready delivery of one kind in an immediate transaction.
    ///
    /// Expired claims are eligible for takeover. The incremented attempt is a
    /// fencing token, so a stale worker cannot acknowledge a later claim even
    /// when the same worker identity is reused.
    ///
    /// # Errors
    ///
    /// Returns a validation, corruption, or SQLite transaction error.
    pub fn claim_outbox(
        &mut self,
        delivery_kind: &BoundedName,
        lease_owner: &BoundedOpaque,
        now_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<Option<OutboxClaim>, StoreError> {
        if lease_expires_at_ms <= now_ms {
            return Err(invalid("Outbox lease deadline must be in the future"));
        }
        let now = sqlite_integer(now_ms, "Outbox claim timestamp")?;
        let lease_deadline = sqlite_integer(lease_expires_at_ms, "Outbox lease deadline")?;
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = transaction
            .query_row(
                "SELECT delivery_id, task_id, event_id, payload_json, attempt
                 FROM outbox
                 WHERE delivery_kind = ?1 AND next_attempt_at_ms <= ?2
                   AND (state = 'pending'
                        OR (state = 'leased' AND lease_expires_at_ms <= ?2))
                 ORDER BY next_attempt_at_ms, created_at_ms, delivery_id
                 LIMIT 1",
                params![delivery_kind.as_str(), now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((delivery_id, task_id, event_id, payload_json, attempt)) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };
        let next_attempt = attempt
            .checked_add(1)
            .ok_or_else(|| corrupt("Outbox attempt overflow"))?;
        let changed = transaction.execute(
            "UPDATE outbox
             SET state='leased', attempt=?2, lease_owner=?3, lease_expires_at_ms=?4
             WHERE delivery_id=?1 AND attempt=?5
               AND (state='pending' OR (state='leased' AND lease_expires_at_ms <= ?6))",
            params![
                delivery_id,
                next_attempt,
                lease_owner.as_str(),
                lease_deadline,
                attempt,
                now,
            ],
        )?;
        if changed != 1 {
            return Err(corrupt(
                "Outbox claim lost its immediate-transaction precondition",
            ));
        }
        let claim = OutboxClaim {
            delivery_id: DeliveryId::parse(&delivery_id)
                .map_err(|error| corrupt(&format!("invalid Outbox delivery identity: {error}")))?,
            task_id: TaskId::parse(&task_id)
                .map_err(|error| corrupt(&format!("invalid Outbox Task identity: {error}")))?,
            event_id: MessageId::parse(&event_id)
                .map_err(|error| corrupt(&format!("invalid Outbox event identity: {error}")))?,
            delivery_kind: delivery_kind.clone(),
            payload: serde_json::from_str(&payload_json)?,
            attempt: u64::try_from(next_attempt).map_err(|_| corrupt("negative Outbox attempt"))?,
            lease_owner: lease_owner.clone(),
            lease_expires_at_ms,
        };
        transaction.commit()?;
        Ok(Some(claim))
    }

    /// Marks an exact, unexpired Outbox claim delivered.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::GenerationFenced`] for a stale claim.
    pub fn complete_outbox(
        &mut self,
        claim: &OutboxClaim,
        completed_at_ms: u64,
    ) -> Result<(), StoreError> {
        let completed_at = sqlite_integer(completed_at_ms, "Outbox completion timestamp")?;
        let attempt = sqlite_integer(claim.attempt, "Outbox attempt")?;
        let changed = self.connection_mut().execute(
            "UPDATE outbox
             SET state='delivered', lease_owner=NULL, lease_expires_at_ms=NULL,
                 delivered_at_ms=?2
             WHERE delivery_id=?1 AND state='leased' AND attempt=?3
               AND lease_owner=?4 AND lease_expires_at_ms > ?2",
            params![
                claim.delivery_id.as_str(),
                completed_at,
                attempt,
                claim.lease_owner.as_str(),
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::GenerationFenced {
                expected: claim.attempt,
                actual: self.outbox_attempt(&claim.delivery_id)?.unwrap_or(0),
            })
        }
    }

    /// Releases an exact claim for a bounded retry after a failed delivery.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::GenerationFenced`] for a stale claim.
    pub fn retry_outbox(
        &mut self,
        claim: &OutboxClaim,
        released_at_ms: u64,
        next_attempt_at_ms: u64,
    ) -> Result<(), StoreError> {
        if next_attempt_at_ms < released_at_ms {
            return Err(invalid("Outbox retry cannot precede its release"));
        }
        let attempt = sqlite_integer(claim.attempt, "Outbox attempt")?;
        let released_at = sqlite_integer(released_at_ms, "Outbox release timestamp")?;
        let next_attempt = sqlite_integer(next_attempt_at_ms, "Outbox retry timestamp")?;
        let changed = self.connection_mut().execute(
            "UPDATE outbox
             SET state='pending', next_attempt_at_ms=?2,
                 lease_owner=NULL, lease_expires_at_ms=NULL
             WHERE delivery_id=?1 AND state='leased' AND attempt=?3 AND lease_owner=?4
               AND lease_expires_at_ms > ?5",
            params![
                claim.delivery_id.as_str(),
                next_attempt,
                attempt,
                claim.lease_owner.as_str(),
                released_at,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::GenerationFenced {
                expected: claim.attempt,
                actual: self.outbox_attempt(&claim.delivery_id)?.unwrap_or(0),
            })
        }
    }

    fn outbox_attempt(&self, delivery_id: &DeliveryId) -> Result<Option<u64>, StoreError> {
        let attempt = self
            .connection()
            .query_row(
                "SELECT attempt FROM outbox WHERE delivery_id=?1",
                params![delivery_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        attempt
            .map(|value| u64::try_from(value).map_err(|_| corrupt("negative Outbox attempt")))
            .transpose()
    }

    /// Returns a durable task-command receipt for exact replay.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict when the key belongs to another
    /// command, or a corruption error for an invalid stored receipt.
    pub fn load_command_receipt(
        &self,
        actor_id: &ActorId,
        idempotency_key: &IdempotencyKey,
        command_digest: &Digest,
    ) -> Result<Option<CommitReceipt>, StoreError> {
        let existing = self
            .connection()
            .query_row(
                "SELECT command_digest, receipt_json FROM command_receipts
                 WHERE actor_id = ?1 AND idempotency_key = ?2",
                params![actor_id.as_str(), idempotency_key.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((stored_digest, receipt_json)) = existing else {
            return Ok(None);
        };
        if stored_digest != command_digest.as_str() {
            return Err(StoreError::IdempotencyConflict);
        }
        Ok(Some(serde_json::from_str::<CommitReceipt>(&receipt_json)?))
    }

    /// Atomically persists an already-authenticated and authorized coordinator
    /// decision. This storage boundary does not replace caller-side ingress
    /// authentication or authorization policy.
    ///
    /// Idempotency replay is checked before the optimistic revision, so a
    /// retried command returns its original receipt after the Task advances.
    ///
    /// # Errors
    ///
    /// Returns a conflict for key or revision reuse, a reducer error for an
    /// illegal transition, or a storage error. No partial rows are committed.
    pub(crate) fn commit_task(&mut self, commit: &TaskCommit) -> Result<CommitOutcome, StoreError> {
        self.commit_task_with_run_guard(commit, None)
    }

    pub(crate) fn commit_retry_task(
        &mut self,
        commit: &TaskCommit,
        previous_run_id: &RunId,
    ) -> Result<CommitOutcome, StoreError> {
        self.commit_task_with_run_guard(commit, Some(RunCommitGuard::Retry(previous_run_id)))
    }

    pub(crate) fn commit_suspended_cancel(
        &mut self,
        commit: &TaskCommit,
        run_id: &RunId,
    ) -> Result<CommitOutcome, StoreError> {
        self.commit_task_with_run_guard(commit, Some(RunCommitGuard::SuspendedCancel(run_id)))
    }

    fn commit_task_with_run_guard(
        &mut self,
        commit: &TaskCommit,
        guard: Option<RunCommitGuard<'_>>,
    ) -> Result<CommitOutcome, StoreError> {
        validate_commit_resource_bounds(commit)?;
        let (task_id, event_ids) = validate_commit_shape(commit)?;
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(outcome) = replay_receipt(&transaction, commit)? {
            let task_id = match &outcome {
                CommitOutcome::Replayed(receipt) | CommitOutcome::Applied(receipt) => {
                    &receipt.task_id
                }
            };
            load_verified_projection(&transaction, task_id)?
                .ok_or_else(|| corrupt("idempotency receipt references a missing Task"))?;
            transaction.commit()?;
            return Ok(outcome);
        }

        if let Some(guard) = guard {
            match guard {
                RunCommitGuard::Retry(previous_run_id) => {
                    require_retry_run_quiescent(&transaction, commit, previous_run_id)?;
                }
                RunCommitGuard::SuspendedCancel(run_id) => {
                    require_suspended_cancel_quiescent(&transaction, commit, run_id)?;
                }
            }
        }

        let current = load_verified_projection(&transaction, task_id)?;
        if current
            .as_ref()
            .is_some_and(|aggregate| aggregate.owner_actor_id() != &commit.actor_id)
        {
            return Err(invalid("commit actor does not own the existing Task"));
        }
        let actual_revision = current.as_ref().map_or(0, TaskAggregate::revision);
        if let Some(expected) = commit.expected_revision {
            if expected != actual_revision {
                return Err(StoreError::RevisionConflict {
                    expected,
                    actual: actual_revision,
                });
            }
        }

        let aggregate = reduce_commit(current, &commit.events)?;
        if aggregate.owner_actor_id() != &commit.actor_id {
            return Err(invalid("commit actor does not own the created Task"));
        }
        persist_projection(
            &transaction,
            &aggregate,
            actual_revision,
            commit.committed_at_ms,
        )?;
        append_events(&transaction, &commit.events)?;
        append_outbox(&transaction, task_id, commit)?;

        let receipt = CommitReceipt {
            task_id: task_id.clone(),
            revision: aggregate.revision(),
            event_ids,
            delivery_ids: commit
                .outbox
                .iter()
                .map(|intent| intent.delivery_id.clone())
                .collect(),
        };
        insert_receipt(&transaction, commit, &receipt)?;
        transaction.commit()?;
        Ok(CommitOutcome::Applied(receipt))
    }

    /// Exposes the raw Task writer only to debug integration fault fixtures.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn commit_task_for_test(
        &mut self,
        commit: &TaskCommit,
    ) -> Result<CommitOutcome, StoreError> {
        self.commit_task(commit)
    }

    pub(crate) fn load_runtime_start_intent_for_retry(
        &self,
        actor_id: &ActorId,
        task_id: &TaskId,
        previous_run_id: &RunId,
    ) -> Result<serde_json::Value, StoreError> {
        let task = self.load_task(task_id)?;
        if task.owner_actor_id() != actor_id {
            return Err(StoreError::LedgerConflict {
                message: "retry actor does not own the Task".to_owned(),
            });
        }

        let malformed_count = self.connection().query_row(
            "SELECT COUNT(*) FROM outbox
             WHERE task_id=?1 AND delivery_kind='runtime_start'
               AND json_valid(payload_json)=0",
            params![task_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        if malformed_count != 0 {
            return Err(corrupt(
                "runtime start Outbox contains malformed JSON for the retry Task",
            ));
        }

        let mut statement = self.connection().prepare(
            "SELECT state, length(payload_json),
                    CASE WHEN length(payload_json) <= ?3 THEN payload_json ELSE NULL END
             FROM outbox
             WHERE task_id=?1 AND delivery_kind='runtime_start'
               AND json_extract(payload_json, '$.run_id')=?2
             ORDER BY created_at_ms, delivery_id LIMIT 2",
        )?;
        let rows = statement
            .query_map(
                params![
                    task_id.as_str(),
                    previous_run_id.as_str(),
                    i64::try_from(MAX_TASK_PAYLOAD_BYTES)
                        .map_err(|_| corrupt("runtime start payload bound exceeds SQLite range"))?,
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let [(state, payload_bytes, bounded_payload_json)] = rows.as_slice() else {
            if rows.is_empty() {
                return Err(StoreError::LedgerNotFound {
                    entity: format!(
                        "delivered runtime start intent for Run {}",
                        previous_run_id.as_str()
                    ),
                });
            }
            return Err(corrupt(
                "retry Run matches multiple runtime start Outbox intents",
            ));
        };
        if state != "delivered" {
            return Err(StoreError::LedgerConflict {
                message: "retry requires a delivered runtime start intent".to_owned(),
            });
        }
        let payload_bytes = usize::try_from(*payload_bytes)
            .map_err(|_| corrupt("runtime start Outbox has a negative payload length"))?;
        if payload_bytes > MAX_TASK_PAYLOAD_BYTES {
            return Err(corrupt(
                "runtime start Outbox payload exceeds the durable payload bound",
            ));
        }
        let payload_json = bounded_payload_json
            .as_ref()
            .ok_or_else(|| corrupt("bounded runtime start Outbox payload was not materialized"))?;
        let payload = serde_json::from_str::<serde_json::Value>(payload_json)
            .map_err(|error| corrupt(&format!("runtime start payload cannot decode: {error}")))?;
        if payload
            .pointer("/actor/actor_id")
            .and_then(|value| value.as_str())
            != Some(actor_id.as_str())
            || payload.get("task_id").and_then(|value| value.as_str()) != Some(task_id.as_str())
            || payload.get("run_id").and_then(|value| value.as_str())
                != Some(previous_run_id.as_str())
        {
            return Err(corrupt(
                "runtime start Outbox payload does not match retry identities",
            ));
        }
        Ok(payload)
    }

    /// Loads the latest durable Task projection.
    ///
    /// # Errors
    ///
    /// Returns `TaskNotFound` or rejects a corrupt or divergent projection.
    pub fn load_task(&self, task_id: &TaskId) -> Result<TaskAggregate, StoreError> {
        load_verified_projection(self.connection(), task_id)?.ok_or(StoreError::TaskNotFound)
    }

    /// Returns whether cancellation was requested for the active Run and has
    /// not yet reached a durable Run-cancelled event.
    ///
    /// # Errors
    ///
    /// Returns a mismatch, corruption, or SQLite read error.
    pub fn run_cancellation_requested(
        &self,
        task_id: &TaskId,
        run_id: &cosh_gateway_contracts::ids::RunId,
    ) -> Result<bool, StoreError> {
        let task = self.load_task(task_id)?;
        if task.active_run_id() != Some(run_id) {
            return Err(invalid("cancellation query does not match the active Run"));
        }
        let mut statement = self.connection().prepare(
            "SELECT payload_json FROM task_events
             WHERE task_id=?1 AND event_type IN ('cancellation_requested', 'run_cancelled')
             ORDER BY revision",
        )?;
        let payloads = statement
            .query_map(params![task_id.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut requested = false;
        for payload in payloads {
            let event = serde_json::from_str::<TaskEventEnvelope>(&payload)
                .map_err(|error| corrupt(&format!("cancellation event cannot decode: {error}")))?;
            match event.event {
                cosh_gateway_contracts::task::TaskEvent::CancellationRequested {
                    run_id: event_run,
                    ..
                } if &event_run == run_id => requested = true,
                cosh_gateway_contracts::task::TaskEvent::RunCancelled {
                    run_id: event_run, ..
                } if &event_run == run_id => requested = false,
                _ => {}
            }
        }
        Ok(requested)
    }

    /// Rebuilds a Task from its immutable events and verifies the stored
    /// projection matches the deterministic reducer result.
    ///
    /// # Errors
    ///
    /// Returns `TaskNotFound` or rejects corrupt, incomplete, or divergent data.
    pub fn recover_task(&self, task_id: &TaskId) -> Result<TaskAggregate, StoreError> {
        self.load_task(task_id)
    }

    /// Loads a bounded page of immutable Task events after a revision cursor.
    ///
    /// # Errors
    ///
    /// Returns `TaskNotFound` when the stream is absent or rejects corrupt
    /// stored events. Authorization remains the coordinator's responsibility.
    pub fn load_task_events_for_owner(
        &self,
        task_id: &TaskId,
        actor_id: &ActorId,
        after_revision: Option<u64>,
        limit: u16,
    ) -> Result<(Vec<TaskEventEnvelope>, u64), StoreError> {
        if limit == 0 || limit > 64 {
            return Err(invalid("Task event page limit must be between 1 and 64"));
        }
        let revision = self
            .connection()
            .query_row(
                "SELECT revision FROM tasks WHERE task_id = ?1 AND owner_actor_id = ?2",
                params![task_id.as_str(), actor_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(revision) = revision else {
            return Err(StoreError::TaskNotFound);
        };
        let revision = u64::try_from(revision).map_err(|_| corrupt("negative Task revision"))?;
        let after_revision = after_revision.unwrap_or(0);
        let after_sql = sqlite_integer(after_revision, "Task event cursor")?;
        let limit_sql = i64::from(limit);
        let mut statement = self.connection().prepare(
            "SELECT revision, payload_json FROM task_events
             WHERE task_id = ?1 AND revision > ?2
             ORDER BY revision ASC LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![task_id.as_str(), after_sql, limit_sql], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let events = rows
            .into_iter()
            .map(|(stored_revision, payload)| {
                let event = serde_json::from_str::<TaskEventEnvelope>(&payload)
                    .map_err(|error| corrupt(&format!("Task event cannot be decoded: {error}")))?;
                let stored_revision = u64::try_from(stored_revision)
                    .map_err(|_| corrupt("negative Task event revision"))?;
                if event.revision != stored_revision
                    || &event.task_id != task_id
                    || event.header.correlation.actor_id.as_ref() != Some(actor_id)
                {
                    return Err(corrupt(
                        "Task event page row diverges from its identity or owner",
                    ));
                }
                Ok(event)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((events, revision))
    }
}

#[derive(Clone, Copy)]
enum RunCommitGuard<'a> {
    Retry(&'a RunId),
    SuspendedCancel(&'a RunId),
}

fn validate_commit_shape(commit: &TaskCommit) -> Result<(&TaskId, Vec<MessageId>), StoreError> {
    let first = commit
        .events
        .first()
        .ok_or_else(|| invalid("event batch is empty"))?;
    if commit
        .events
        .iter()
        .any(|event| event.task_id != first.task_id)
    {
        return Err(invalid("event batch contains multiple Task identities"));
    }
    if commit
        .events
        .iter()
        .any(|event| event.header.correlation.actor_id.as_ref() != Some(&commit.actor_id))
    {
        return Err(invalid(
            "every event actor correlation must match the admitted commit actor",
        ));
    }
    let event_ids = commit
        .events
        .iter()
        .map(|event| event.header.message_id.clone())
        .collect::<Vec<_>>();
    let unique_event_ids = event_ids.iter().collect::<BTreeSet<_>>();
    if unique_event_ids.len() != event_ids.len() {
        return Err(invalid("event batch reuses a message identity"));
    }
    if commit.outbox.iter().any(|intent| {
        !event_ids
            .iter()
            .any(|event_id| event_id == &intent.event_id)
    }) {
        return Err(invalid(
            "Outbox intent references an event outside the commit",
        ));
    }
    Ok((&first.task_id, event_ids))
}

fn require_retry_run_quiescent(
    transaction: &Transaction<'_>,
    commit: &TaskCommit,
    previous_run_id: &RunId,
) -> Result<(), StoreError> {
    let [event] = commit.events.as_slice() else {
        return Err(StoreError::LedgerConflict {
            message: "retry commit must contain the exact previous Run transition".to_owned(),
        });
    };
    let TaskEvent::RunRetryQueued {
        previous_run_id: event_previous_run_id,
        next_run_id,
    } = &event.event
    else {
        return Err(StoreError::LedgerConflict {
            message: "retry commit must contain the exact previous Run transition".to_owned(),
        });
    };
    if event_previous_run_id != previous_run_id {
        return Err(StoreError::LedgerConflict {
            message: "retry event does not match the guarded previous Run".to_owned(),
        });
    }
    let [intent] = commit.outbox.as_slice() else {
        return Err(StoreError::LedgerConflict {
            message: "retry commit must contain one Runtime start intent".to_owned(),
        });
    };
    if intent.delivery_kind.as_str() != "runtime_start"
        || intent
            .payload
            .pointer("/actor/actor_id")
            .and_then(serde_json::Value::as_str)
            != Some(commit.actor_id.as_str())
        || intent
            .payload
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            != Some(event.task_id.as_str())
        || intent
            .payload
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            != Some(next_run_id.as_str())
    {
        return Err(StoreError::LedgerConflict {
            message: "retry Runtime start intent does not match the next Run".to_owned(),
        });
    }
    require_run_quiescent(transaction, commit, &event.task_id, previous_run_id)
}

fn require_suspended_cancel_quiescent(
    transaction: &Transaction<'_>,
    commit: &TaskCommit,
    run_id: &RunId,
) -> Result<(), StoreError> {
    let [requested, run_cancelled, task_cancelled] = commit.events.as_slice() else {
        return Err(StoreError::LedgerConflict {
            message: "suspended cancel commit must contain its exact terminal transitions"
                .to_owned(),
        });
    };
    if !matches!(
        &requested.event,
        TaskEvent::CancellationRequested { run_id: event_run_id, .. } if event_run_id == run_id
    ) || !matches!(
        &run_cancelled.event,
        TaskEvent::RunCancelled { run_id: event_run_id, .. } if event_run_id == run_id
    ) || !matches!(task_cancelled.event, TaskEvent::TaskCancelled)
        || requested.task_id != run_cancelled.task_id
        || requested.task_id != task_cancelled.task_id
        || !commit.outbox.is_empty()
    {
        return Err(StoreError::LedgerConflict {
            message: "suspended cancel commit does not match the guarded Run".to_owned(),
        });
    }
    require_run_quiescent(transaction, commit, &requested.task_id, run_id)
}

fn require_run_quiescent(
    transaction: &Transaction<'_>,
    commit: &TaskCommit,
    task_id: &TaskId,
    run_id: &RunId,
) -> Result<(), StoreError> {
    let lease = transaction
        .query_row(
            "SELECT task_id, actor_id, expires_at_ms FROM run_leases WHERE run_id=?1",
            params![run_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((lease_task_id, lease_actor_id, expires_at_ms)) = lease else {
        return Err(StoreError::LedgerConflict {
            message: "retry requires the previous Run lease".to_owned(),
        });
    };
    let expires_at_ms = u64::try_from(expires_at_ms)
        .map_err(|_| corrupt("previous Run lease has a negative deadline"))?;
    if lease_task_id != task_id.as_str()
        || lease_actor_id != commit.actor_id.as_str()
        || expires_at_ms > commit.committed_at_ms
    {
        return Err(StoreError::LedgerConflict {
            message: "previous Run lease is live or does not match retry ownership".to_owned(),
        });
    }
    let active_bindings = transaction.query_row(
        "SELECT COUNT(*) FROM runtime_bindings WHERE run_id=?1 AND state='active'",
        params![run_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?;
    if active_bindings != 0 {
        return Err(StoreError::LedgerConflict {
            message: "previous Run still has an active Runtime binding".to_owned(),
        });
    }
    let pending_inputs = transaction.query_row(
        "SELECT COUNT(*) FROM runtime_input_requests
         WHERE run_id=?1 AND state='pending'",
        params![run_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?;
    let unsettled_dispatches = transaction.query_row(
        "SELECT COUNT(*) FROM runtime_input_dispatches
         WHERE run_id=?1 AND state IN ('prepared', 'started')",
        params![run_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?;
    if pending_inputs != 0 || unsettled_dispatches != 0 {
        return Err(StoreError::LedgerConflict {
            message: "previous Run still has unsettled Runtime input".to_owned(),
        });
    }
    Ok(())
}

fn validate_commit_resource_bounds(commit: &TaskCommit) -> Result<(), StoreError> {
    if commit.events.len() > MAX_TASK_EVENTS_PER_COMMIT {
        return Err(invalid(&format!(
            "event batch exceeds {MAX_TASK_EVENTS_PER_COMMIT} entries"
        )));
    }
    if commit.outbox.len() > MAX_OUTBOX_INTENTS_PER_COMMIT {
        return Err(invalid(&format!(
            "Outbox batch exceeds {MAX_OUTBOX_INTENTS_PER_COMMIT} entries"
        )));
    }

    for event in &commit.events {
        validate_serialized_payload_bytes(serde_json::to_vec(event)?.len(), "Task event")?;
    }
    for intent in &commit.outbox {
        validate_serialized_payload_bytes(serde_json::to_vec(&intent.payload)?.len(), "Outbox")?;
    }
    if serde_json::to_vec(commit)?.len() > MAX_TASK_COMMIT_SERIALIZED_BYTES {
        return Err(invalid(&format!(
            "serialized commit exceeds {MAX_TASK_COMMIT_SERIALIZED_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_serialized_payload_bytes(
    payload_bytes: usize,
    payload_kind: &str,
) -> Result<(), StoreError> {
    if payload_bytes > MAX_TASK_PAYLOAD_BYTES {
        return Err(invalid(&format!(
            "{payload_kind} payload exceeds {MAX_TASK_PAYLOAD_BYTES} serialized bytes"
        )));
    }
    Ok(())
}

fn replay_receipt(
    transaction: &Transaction<'_>,
    commit: &TaskCommit,
) -> Result<Option<CommitOutcome>, StoreError> {
    let existing = transaction
        .query_row(
            "SELECT command_digest, receipt_json FROM command_receipts
             WHERE actor_id = ?1 AND idempotency_key = ?2",
            params![commit.actor_id.as_str(), commit.idempotency_key.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((digest, receipt_json)) = existing else {
        return Ok(None);
    };
    if digest != commit.command_digest.as_str() {
        return Err(StoreError::IdempotencyConflict);
    }
    let receipt = serde_json::from_str::<CommitReceipt>(&receipt_json)?;
    Ok(Some(CommitOutcome::Replayed(receipt)))
}

fn load_snapshot(
    connection: &rusqlite::Connection,
    task_id: &TaskId,
) -> Result<Option<TaskAggregate>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT revision, snapshot_json FROM tasks WHERE task_id = ?1",
            params![task_id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((revision, snapshot_json)) = stored else {
        return Ok(None);
    };
    let revision = u64::try_from(revision).map_err(|_| corrupt("negative Task revision"))?;
    let aggregate = serde_json::from_str::<TaskAggregate>(&snapshot_json)
        .map_err(|error| corrupt(&format!("Task snapshot cannot be decoded: {error}")))?;
    if aggregate.task_id() != task_id || aggregate.revision() != revision {
        return Err(corrupt(
            "Task snapshot identity or revision does not match its row",
        ));
    }
    Ok(Some(aggregate))
}

pub(super) fn load_verified_projection(
    connection: &rusqlite::Connection,
    task_id: &TaskId,
) -> Result<Option<TaskAggregate>, StoreError> {
    let snapshot = load_snapshot(connection, task_id)?;
    let events = load_events(connection, task_id)?;
    match (snapshot, events.is_empty()) {
        (None, true) => Ok(None),
        (None, false) => Err(corrupt("Task event stream has no projection")),
        (Some(_), true) => Err(corrupt("Task projection has no event stream")),
        (Some(snapshot), false) => {
            let recovered = TaskAggregate::replay(&events)?;
            if recovered != snapshot {
                return Err(corrupt("stored projection diverges from event replay"));
            }
            Ok(Some(recovered))
        }
    }
}

fn load_events(
    connection: &rusqlite::Connection,
    task_id: &TaskId,
) -> Result<Vec<TaskEventEnvelope>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT payload_json FROM task_events
         WHERE task_id = ?1 ORDER BY revision ASC",
    )?;
    let payloads = statement
        .query_map(params![task_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    payloads
        .into_iter()
        .map(|payload| {
            serde_json::from_str::<TaskEventEnvelope>(&payload)
                .map_err(|error| corrupt(&format!("Task event cannot be decoded: {error}")))
        })
        .collect()
}

fn reduce_commit(
    current: Option<TaskAggregate>,
    events: &[TaskEventEnvelope],
) -> Result<TaskAggregate, StoreError> {
    match current {
        Some(mut aggregate) => {
            for event in events {
                aggregate.apply(event)?;
            }
            Ok(aggregate)
        }
        None => Ok(TaskAggregate::replay(events)?),
    }
}

fn persist_projection(
    transaction: &Transaction<'_>,
    aggregate: &TaskAggregate,
    previous_revision: u64,
    committed_at_ms: u64,
) -> Result<(), StoreError> {
    let revision = sqlite_integer(aggregate.revision(), "Task revision")?;
    let previous_revision = sqlite_integer(previous_revision, "previous Task revision")?;
    let committed_at_ms = sqlite_integer(committed_at_ms, "commit timestamp")?;
    let snapshot_json = serde_json::to_string(aggregate)?;
    let target_ref = serde_json::to_string(aggregate.target())?;
    let state = task_state_name(aggregate.state())?;
    if previous_revision == 0 {
        transaction.execute(
            "INSERT INTO tasks(
                 task_id, owner_actor_id, target_ref, revision, state,
                 snapshot_json, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                aggregate.task_id().as_str(),
                aggregate.owner_actor_id().as_str(),
                target_ref,
                revision,
                state,
                snapshot_json,
                committed_at_ms,
            ],
        )?;
    } else {
        let changed = transaction.execute(
            "UPDATE tasks SET revision = ?2, state = ?3, snapshot_json = ?4,
                 updated_at_ms = ?5
             WHERE task_id = ?1 AND revision = ?6",
            params![
                aggregate.task_id().as_str(),
                revision,
                state,
                snapshot_json,
                committed_at_ms,
                previous_revision,
            ],
        )?;
        if changed != 1 {
            return Err(corrupt("Task projection compare-and-swap changed no row"));
        }
    }
    Ok(())
}

fn append_events(
    transaction: &Transaction<'_>,
    events: &[TaskEventEnvelope],
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "INSERT INTO task_events(
             event_id, task_id, revision, event_type, schema_version,
             payload_json, occurred_at_ms, causation_id, correlation_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for event in events {
        let revision = sqlite_integer(event.revision, "event revision")?;
        let occurred_at_ms = sqlite_integer(event.header.occurred_at_ms, "event timestamp")?;
        let payload_json = serde_json::to_string(event)?;
        let event_type = serde_json::to_value(event.event.kind())?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| corrupt("Task event kind is not a string"))?;
        statement.execute(params![
            event.header.message_id.as_str(),
            event.task_id.as_str(),
            revision,
            event_type,
            i64::from(event.header.schema_version),
            payload_json,
            occurred_at_ms,
            event
                .header
                .correlation
                .causation_message_id
                .as_ref()
                .map(MessageId::as_str),
            Option::<&str>::None,
        ])?;
    }
    Ok(())
}

pub(super) fn append_internal_task_event(
    transaction: &Transaction<'_>,
    task_id: &TaskId,
    actor_id: &ActorId,
    committed_at_ms: u64,
    event: TaskEvent,
    outbox: Option<(BoundedName, serde_json::Value)>,
) -> Result<TaskEventEnvelope, StoreError> {
    let serialized_outbox = outbox
        .as_ref()
        .map(|(_, payload)| {
            let payload_json = serde_json::to_string(payload)?;
            validate_serialized_payload_bytes(payload_json.len(), "internal Outbox")?;
            Ok::<_, StoreError>(payload_json)
        })
        .transpose()?;
    let task = load_verified_projection(transaction, task_id)?.ok_or(StoreError::TaskNotFound)?;
    if task.owner_actor_id() != actor_id {
        return Err(invalid("internal Task event actor does not own the Task"));
    }
    let previous = load_events(transaction, task_id)?
        .into_iter()
        .last()
        .ok_or_else(|| corrupt("internal Task event has no immutable predecessor"))?;
    let mut correlation = previous.header.correlation.clone();
    correlation.actor_id = Some(actor_id.clone());
    correlation.task_id = Some(task_id.clone());
    correlation.run_id = task.active_run_id().cloned();
    correlation.causation_message_id = Some(previous.header.message_id);
    let revision = task
        .revision()
        .checked_add(1)
        .ok_or_else(|| corrupt("internal Task event revision overflow"))?;
    let envelope = migration_event(task_id, revision, committed_at_ms, correlation, event);
    let aggregate = reduce_commit(Some(task.clone()), std::slice::from_ref(&envelope))?;
    persist_projection(transaction, &aggregate, task.revision(), committed_at_ms)?;
    append_events(transaction, std::slice::from_ref(&envelope))?;
    if let Some(((delivery_kind, _), payload_json)) = outbox.zip(serialized_outbox) {
        transaction.execute(
            "INSERT INTO outbox(
                 delivery_id, task_id, event_id, delivery_kind, payload_json,
                 state, next_attempt_at_ms, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?6)",
            params![
                DeliveryId::new().as_str(),
                task_id.as_str(),
                envelope.header.message_id.as_str(),
                delivery_kind.as_str(),
                payload_json,
                sqlite_integer(committed_at_ms, "internal Outbox timestamp")?,
            ],
        )?;
    }
    Ok(envelope)
}

fn append_outbox(
    transaction: &Transaction<'_>,
    task_id: &TaskId,
    commit: &TaskCommit,
) -> Result<(), StoreError> {
    let created_at_ms = sqlite_integer(commit.committed_at_ms, "Outbox timestamp")?;
    let mut statement = transaction.prepare(
        "INSERT INTO outbox(
             delivery_id, task_id, event_id, delivery_kind, payload_json,
             state, next_attempt_at_ms, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
    )?;
    for intent in &commit.outbox {
        let next_attempt_at_ms =
            sqlite_integer(intent.next_attempt_at_ms, "Outbox next-attempt timestamp")?;
        statement.execute(params![
            intent.delivery_id.as_str(),
            task_id.as_str(),
            intent.event_id.as_str(),
            intent.delivery_kind.as_str(),
            serde_json::to_string(&intent.payload)?,
            next_attempt_at_ms,
            created_at_ms,
        ])?;
    }
    Ok(())
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    commit: &TaskCommit,
    receipt: &CommitReceipt,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO command_receipts(
             actor_id, idempotency_key, command_digest, task_id,
             task_revision, receipt_json, committed_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            commit.actor_id.as_str(),
            commit.idempotency_key.as_str(),
            commit.command_digest.as_str(),
            receipt.task_id.as_str(),
            sqlite_integer(receipt.revision, "receipt Task revision")?,
            serde_json::to_string(receipt)?,
            sqlite_integer(commit.committed_at_ms, "receipt timestamp")?,
        ],
    )?;
    Ok(())
}

fn task_state_name(state: TaskState) -> Result<String, StoreError> {
    serde_json::to_value(state)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| corrupt("Task state is not a string"))
}

fn legacy_recovery_digest(task_id: &TaskId, run_id: &RunId) -> Result<Digest, StoreError> {
    let digest = Sha256::digest(
        format!(
            "cosh.gateway.legacy-runtime-start-recovery.v1\0{}\0{}",
            task_id.as_str(),
            run_id.as_str()
        )
        .as_bytes(),
    );
    Digest::parse(format!("{digest:x}"))
        .map_err(|error| invalid(&format!("legacy recovery digest is invalid: {error}")))
}

fn migration_event(
    task_id: &TaskId,
    revision: u64,
    occurred_at_ms: u64,
    correlation: Correlation,
    event: TaskEvent,
) -> TaskEventEnvelope {
    TaskEventEnvelope {
        header: ContractHeader::new(
            ContractSchema::TaskEvent,
            MessageId::new(),
            occurred_at_ms,
            correlation,
        ),
        task_id: task_id.clone(),
        revision,
        event,
    }
}

fn current_time_ms() -> Result<u64, StoreError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| corrupt("system clock predates Unix epoch during legacy recovery"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| invalid("legacy recovery timestamp exceeds u64 range"))
}

fn sqlite_integer(value: u64, field: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| invalid(&format!("{field} exceeds SQLite INTEGER range")))
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidCommit {
        message: message.to_string(),
    }
}

fn corrupt(message: &str) -> StoreError {
    StoreError::Corrupt {
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests;
