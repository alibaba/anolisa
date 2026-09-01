use asc_foundation_types::{ResourceId, Revision};
use asc_pap::Page;
use asc_policy_runtime::{
    AdapterAccepted, AdapterCommand, AdapterDispatchError, BindingDesiredState, OperationState,
    PreparedBinding, ReconcileOperation, RuntimeError, RuntimeRepository,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::sql::{
    BoundedListError, append_audit, count, decode, list_json_bounded, serialize, to_i64,
};
use crate::store::SqlitePolicyStore;

impl RuntimeRepository for SqlitePolicyStore {
    fn accept_binding(
        &self,
        binding: &PreparedBinding,
        operation: &ReconcileOperation,
        expected_binding_revision: Option<Revision>,
    ) -> Result<ReconcileOperation, RuntimeError> {
        let mut connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| RuntimeError::Persistence)?;
        if let Some(existing) = select_operation(&transaction, &operation.operation_id)? {
            if existing.request_digest == operation.request_digest {
                return Ok(existing);
            }
            return Err(RuntimeError::IdempotencyConflict);
        }
        let current_revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM bindings WHERE binding_id = ?1",
                [binding.binding_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| RuntimeError::Persistence)?;
        match (current_revision, expected_binding_revision) {
            (None, None) => {}
            (Some(current), Some(expected))
                if current == expected.get() && binding.binding_revision.get() > current => {}
            _ => return Err(RuntimeError::PreconditionFailed),
        }
        transaction
            .execute(
                "UPDATE operations SET state = 'SUPERSEDED', error_code = NULL
                 WHERE binding_id = ?1 AND state IN ('QUEUED', 'RETRY_WAIT')",
                [binding.binding_id.as_str()],
            )
            .map_err(|_| RuntimeError::Persistence)?;
        let binding_json = serialize(binding).map_err(|()| RuntimeError::Persistence)?;
        transaction
            .execute(
                "INSERT INTO bindings
                 (binding_id, revision, policy_id, policy_revision, scope_id, scope_revision,
                  desired_state, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(binding_id) DO UPDATE SET
                   revision=excluded.revision,
                   policy_id=excluded.policy_id,
                   policy_revision=excluded.policy_revision,
                   scope_id=excluded.scope_id,
                   scope_revision=excluded.scope_revision,
                   desired_state=excluded.desired_state,
                   record_json=excluded.record_json",
                params![
                    binding.binding_id.as_str(),
                    to_i64(binding.binding_revision).map_err(|()| RuntimeError::Persistence)?,
                    binding.policy.policy_id.as_str(),
                    to_i64(binding.policy.revision).map_err(|()| RuntimeError::Persistence)?,
                    binding.scope.scope_id.as_str(),
                    to_i64(binding.scope.revision).map_err(|()| RuntimeError::Persistence)?,
                    desired_state_name(binding.desired_state),
                    binding_json
                ],
            )
            .map_err(|_| RuntimeError::Persistence)?;
        insert_operation(&transaction, operation)?;
        transaction
            .execute(
                "INSERT INTO outbox(operation_id, state) VALUES (?1, 'QUEUED')",
                [operation.operation_id.as_str()],
            )
            .map_err(|_| RuntimeError::Persistence)?;
        let audit_method = match binding.desired_state {
            BindingDesiredState::Ready => "policy.bindings.put",
            BindingDesiredState::Absent => "policy.bindings.delete",
        };
        append_audit(
            &transaction,
            audit_method,
            binding.binding_id.as_str(),
            Some(operation.operation_id.as_str()),
            "accepted",
        )
        .map_err(|()| RuntimeError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| RuntimeError::Persistence)?;
        Ok(operation.clone())
    }

    fn get_binding(&self, id: &ResourceId) -> Result<PreparedBinding, RuntimeError> {
        let connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        select_json(
            &connection,
            "SELECT record_json FROM bindings WHERE binding_id = ?1",
            id.as_str(),
        )?
        .ok_or(RuntimeError::NotFound)
    }

    fn list_bindings(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedBinding>, RuntimeError> {
        let connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        let total = count(&connection, "bindings").map_err(|()| RuntimeError::Persistence)?;
        let items = list_json_bounded(
            &connection,
            "SELECT length(CAST(record_json AS BLOB)), record_json
             FROM bindings ORDER BY binding_id LIMIT ?1 OFFSET ?2",
            limit,
            offset,
            max_item_bytes,
        )
        .map_err(runtime_list_error)?;
        Ok(Page { items, total })
    }

    fn get_operation(&self, id: &ResourceId) -> Result<ReconcileOperation, RuntimeError> {
        let connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        select_operation(&connection, id)?.ok_or(RuntimeError::NotFound)
    }

    fn claim_next(&self) -> Result<Option<AdapterCommand>, RuntimeError> {
        let mut connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| RuntimeError::Persistence)?;
        let selected: Option<(String, String)> = transaction
            .query_row(
                "SELECT o.operation_id, b.record_json
                 FROM operations o
                 JOIN bindings b ON b.binding_id = o.binding_id
                 JOIN outbox x ON x.operation_id = o.operation_id
                 WHERE o.state = 'QUEUED' AND x.state = 'QUEUED'
                   AND b.revision = o.binding_revision
                 ORDER BY o.rowid LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| RuntimeError::Persistence)?;
        let Some((operation_id, binding_json)) = selected else {
            return Ok(None);
        };
        let operation_id = ResourceId::new(operation_id).map_err(|_| RuntimeError::Persistence)?;
        let binding: PreparedBinding =
            serde_json::from_str(&binding_json).map_err(|_| RuntimeError::Persistence)?;
        update_operation_state(
            &transaction,
            &operation_id,
            OperationState::Dispatching,
            None,
        )?;
        transaction
            .execute(
                "UPDATE outbox SET state = 'DISPATCHING' WHERE operation_id = ?1",
                [operation_id.as_str()],
            )
            .map_err(|_| RuntimeError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| RuntimeError::Persistence)?;
        Ok(Some(AdapterCommand {
            operation_id,
            binding,
        }))
    }

    fn finish_dispatch(
        &self,
        operation_id: &ResourceId,
        outcome: Result<AdapterAccepted, AdapterDispatchError>,
    ) -> Result<ReconcileOperation, RuntimeError> {
        let mut connection = self.connection().map_err(|()| RuntimeError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| RuntimeError::Persistence)?;
        let (state, code) = match outcome {
            Ok(AdapterAccepted) => (OperationState::Dispatched, None),
            Err(AdapterDispatchError::Unavailable) => {
                (OperationState::Blocked, Some("adapter_unavailable"))
            }
            Err(AdapterDispatchError::Retryable) => {
                (OperationState::RetryWait, Some("adapter_retryable"))
            }
            Err(AdapterDispatchError::Rejected) => {
                (OperationState::Failed, Some("adapter_rejected"))
            }
        };
        update_operation_state(&transaction, operation_id, state, code)?;
        transaction
            .execute(
                "UPDATE outbox SET state = ?2 WHERE operation_id = ?1",
                params![operation_id.as_str(), operation_state_name(state)],
            )
            .map_err(|_| RuntimeError::Persistence)?;
        let operation =
            select_operation(&transaction, operation_id)?.ok_or(RuntimeError::NotFound)?;
        transaction
            .commit()
            .map_err(|_| RuntimeError::Persistence)?;
        Ok(operation)
    }
}

fn insert_operation(
    transaction: &Transaction<'_>,
    operation: &ReconcileOperation,
) -> Result<(), RuntimeError> {
    transaction
        .execute(
            "INSERT INTO operations
             (operation_id, binding_id, binding_revision, request_digest, state, stage,
              error_code, record_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                operation.operation_id.as_str(),
                operation.binding_id.as_str(),
                to_i64(operation.binding_revision).map_err(|()| RuntimeError::Persistence)?,
                operation.request_digest,
                operation_state_name(operation.state),
                operation.stage,
                operation.error_code,
                serialize(operation).map_err(|()| RuntimeError::Persistence)?
            ],
        )
        .map_err(|_| RuntimeError::Persistence)?;
    Ok(())
}

fn update_operation_state(
    transaction: &Transaction<'_>,
    operation_id: &ResourceId,
    state: OperationState,
    error_code: Option<&str>,
) -> Result<(), RuntimeError> {
    let mut operation =
        select_operation(transaction, operation_id)?.ok_or(RuntimeError::NotFound)?;
    operation.state = state;
    operation.error_code = error_code.map(str::to_owned);
    transaction
        .execute(
            "UPDATE operations SET state = ?2, error_code = ?3, record_json = ?4
             WHERE operation_id = ?1",
            params![
                operation_id.as_str(),
                operation_state_name(state),
                error_code,
                serialize(&operation).map_err(|()| RuntimeError::Persistence)?
            ],
        )
        .map_err(|_| RuntimeError::Persistence)?;
    Ok(())
}

fn select_operation(
    connection: &Connection,
    id: &ResourceId,
) -> Result<Option<ReconcileOperation>, RuntimeError> {
    connection
        .query_row(
            "SELECT record_json, state, error_code FROM operations WHERE operation_id = ?1",
            [id.as_str()],
            decode_operation_row,
        )
        .optional()
        .map_err(|_| RuntimeError::Persistence)
}

fn decode_operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReconcileOperation> {
    let json: String = row.get(0)?;
    let state: String = row.get(1)?;
    let error_code: Option<String> = row.get(2)?;
    let mut operation: ReconcileOperation = decode(&json)?;
    operation.state = operation_state_from_name(&state).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            state.len(),
            rusqlite::types::Type::Text,
            "invalid operation state".into(),
        )
    })?;
    operation.error_code = error_code;
    Ok(operation)
}

fn select_json<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Option<T>, RuntimeError> {
    connection
        .query_row(sql, [parameter], |row| {
            let json: String = row.get(0)?;
            decode(&json)
        })
        .optional()
        .map_err(|_| RuntimeError::Persistence)
}

const fn runtime_list_error(error: BoundedListError) -> RuntimeError {
    match error {
        BoundedListError::Database => RuntimeError::Persistence,
        BoundedListError::ItemTooLarge => RuntimeError::ResponseTooLarge,
    }
}

const fn desired_state_name(state: BindingDesiredState) -> &'static str {
    match state {
        BindingDesiredState::Ready => "READY",
        BindingDesiredState::Absent => "ABSENT",
    }
}

const fn operation_state_name(state: OperationState) -> &'static str {
    match state {
        OperationState::Queued => "QUEUED",
        OperationState::Dispatching => "DISPATCHING",
        OperationState::Dispatched => "DISPATCHED",
        OperationState::RetryWait => "RETRY_WAIT",
        OperationState::Blocked => "BLOCKED",
        OperationState::Failed => "FAILED",
        OperationState::Superseded => "SUPERSEDED",
    }
}

fn operation_state_from_name(value: &str) -> Option<OperationState> {
    match value {
        "QUEUED" => Some(OperationState::Queued),
        "DISPATCHING" => Some(OperationState::Dispatching),
        "DISPATCHED" => Some(OperationState::Dispatched),
        "RETRY_WAIT" => Some(OperationState::RetryWait),
        "BLOCKED" => Some(OperationState::Blocked),
        "FAILED" => Some(OperationState::Failed),
        "SUPERSEDED" => Some(OperationState::Superseded),
        _ => None,
    }
}
