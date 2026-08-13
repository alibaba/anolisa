//! Atomic Task event, projection, receipt, and Outbox persistence.

use std::collections::BTreeSet;

use cosh_gateway_contracts::common::{BoundedName, Digest, IdempotencyKey};
use cosh_gateway_contracts::ids::{ActorId, DeliveryId, MessageId, TaskId};
use cosh_gateway_contracts::task::{TaskEventEnvelope, TaskState};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::task::TaskAggregate;

use super::{SqliteTaskStore, StoreError};

/// One durable delivery intent created by a Task event transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Complete unit of work admitted by the single Task writer.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn commit_task(&mut self, commit: &TaskCommit) -> Result<CommitOutcome, StoreError> {
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

    /// Loads the latest durable Task projection.
    ///
    /// # Errors
    ///
    /// Returns `TaskNotFound` or rejects a corrupt or divergent projection.
    pub fn load_task(&self, task_id: &TaskId) -> Result<TaskAggregate, StoreError> {
        load_verified_projection(self.connection(), task_id)?.ok_or(StoreError::TaskNotFound)
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

fn load_verified_projection(
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
