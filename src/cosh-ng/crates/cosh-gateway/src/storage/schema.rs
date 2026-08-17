//! Checksummed SQLite schema migrations for Gateway Task storage.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::StoreError;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 9;

struct Migration {
    version: u32,
    checksum: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        checksum: "cosh-gateway-task-schema-v1-20260813-causation-nullable",
        sql: r#"
CREATE TABLE tasks (
    task_id TEXT PRIMARY KEY NOT NULL,
    owner_actor_id TEXT NOT NULL,
    target_ref TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    state TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE TABLE task_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    event_type TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    payload_json TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    causation_id TEXT,
    correlation_id TEXT,
    UNIQUE(task_id, revision)
) STRICT;

CREATE TABLE command_receipts (
    actor_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    command_digest TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    task_revision INTEGER NOT NULL CHECK (task_revision >= 0),
    receipt_json TEXT NOT NULL,
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    PRIMARY KEY(actor_id, idempotency_key)
) STRICT;

CREATE TABLE outbox (
    delivery_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    event_id TEXT NOT NULL REFERENCES task_events(event_id) ON DELETE RESTRICT,
    delivery_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'leased', 'delivered', 'dead_letter')),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    next_attempt_at_ms INTEGER NOT NULL CHECK (next_attempt_at_ms >= 0),
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    delivered_at_ms INTEGER,
    CHECK ((state = 'leased') = (lease_owner IS NOT NULL)),
    CHECK ((state = 'leased') = (lease_expires_at_ms IS NOT NULL))
) STRICT;

CREATE INDEX task_events_task_revision
    ON task_events(task_id, revision);
CREATE INDEX outbox_ready
    ON outbox(state, next_attempt_at_ms, created_at_ms);
"#,
    },
    Migration {
        version: 2,
        checksum: "cosh-gateway-ledger-schema-v2-20260814-fenced",
        sql: r#"
CREATE TABLE ledger_receipts (
    actor_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    command_digest TEXT NOT NULL,
    operation TEXT NOT NULL,
    result_json TEXT NOT NULL,
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    PRIMARY KEY(actor_id, idempotency_key)
) STRICT;

CREATE TABLE approvals (
    approval_id TEXT PRIMARY KEY NOT NULL,
    request_id TEXT NOT NULL UNIQUE,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    target_json TEXT NOT NULL,
    operation_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'approved', 'denied', 'expired', 'cancelled')
    ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
    decided_by_actor_id TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    CHECK ((state IN ('approved', 'denied')) = (decided_by_actor_id IS NOT NULL))
) STRICT;

CREATE TABLE executions (
    execution_id TEXT PRIMARY KEY NOT NULL,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    target_json TEXT NOT NULL,
    operation_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('planned', 'started', 'succeeded', 'failed', 'uncertain')
    ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    CHECK ((state = 'planned') = (started_at_ms IS NULL)),
    CHECK ((state IN ('succeeded', 'failed', 'uncertain')) = (completed_at_ms IS NOT NULL))
) STRICT;

CREATE TABLE permits (
    permit_id TEXT PRIMARY KEY NOT NULL,
    request_id TEXT NOT NULL UNIQUE,
    approval_id TEXT REFERENCES approvals(approval_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    execution_id TEXT NOT NULL UNIQUE REFERENCES executions(execution_id) ON DELETE RESTRICT,
    target_json TEXT NOT NULL,
    operation_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    policy_revision INTEGER NOT NULL CHECK (policy_revision >= 0),
    state TEXT NOT NULL CHECK (state IN ('issued', 'consumed', 'expired', 'revoked')),
    single_use INTEGER NOT NULL CHECK (single_use = 1),
    valid_until_ms INTEGER NOT NULL CHECK (valid_until_ms >= 0),
    consumed_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    CHECK ((state = 'consumed') = (consumed_at_ms IS NOT NULL))
) STRICT;

CREATE TABLE execution_receipts (
    execution_id TEXT PRIMARY KEY NOT NULL REFERENCES executions(execution_id) ON DELETE RESTRICT,
    state TEXT NOT NULL CHECK (state IN ('succeeded', 'failed')),
    receipt_digest TEXT NOT NULL,
    safe_detail TEXT,
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0)
) STRICT;

CREATE TABLE runtime_bindings (
    binding_id TEXT PRIMARY KEY NOT NULL,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    runtime_instance_id TEXT NOT NULL,
    runtime_generation INTEGER NOT NULL CHECK (runtime_generation > 0),
    binding_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'closed', 'lost')),
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(run_id, runtime_generation)
) STRICT;

CREATE TABLE run_leases (
    run_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    lease_owner TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    revision INTEGER NOT NULL CHECK (revision > 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT;

CREATE INDEX approvals_pending ON approvals(state, expires_at_ms);
CREATE INDEX permits_issued ON permits(state, valid_until_ms);
CREATE INDEX executions_recovery ON executions(state, updated_at_ms);
CREATE INDEX runtime_bindings_run ON runtime_bindings(run_id, state);
CREATE INDEX run_leases_expiry ON run_leases(expires_at_ms);

CREATE TRIGGER command_receipts_reserve_idempotency_namespace
BEFORE INSERT ON command_receipts
WHEN EXISTS (
    SELECT 1 FROM ledger_receipts
    WHERE actor_id = NEW.actor_id AND idempotency_key = NEW.idempotency_key
)
BEGIN
    SELECT RAISE(ABORT, 'idempotency namespace conflict');
END;

CREATE TRIGGER ledger_receipts_reserve_idempotency_namespace
BEFORE INSERT ON ledger_receipts
WHEN EXISTS (
    SELECT 1 FROM command_receipts
    WHERE actor_id = NEW.actor_id AND idempotency_key = NEW.idempotency_key
)
BEGIN
    SELECT RAISE(ABORT, 'idempotency namespace conflict');
END;
"#,
    },
    Migration {
        version: 3,
        checksum: "cosh-gateway-identity-schema-v3-20260813",
        sql: r#"
CREATE TABLE gateway_identity (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    installation_id TEXT NOT NULL UNIQUE
) STRICT;
"#,
    },
    Migration {
        version: 4,
        checksum: "cosh-gateway-provider-permission-schema-v4-20260816",
        sql: r#"
ALTER TABLE approvals ADD COLUMN permission_ref_json TEXT;

CREATE TABLE provider_permission_dispatches (
    approval_id TEXT PRIMARY KEY NOT NULL
        REFERENCES approvals(approval_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    permission_ref_json TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('allow_once', 'deny')),
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'started', 'delivered', 'unknown')
    ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX provider_permission_dispatches_recovery
    ON provider_permission_dispatches(state, updated_at_ms);
"#,
    },
    Migration {
        version: 5,
        checksum: "cosh-gateway-legacy-runtime-recovery-v5-20260816-admin-receipt",
        sql: r#"
CREATE TABLE legacy_runtime_start_recoveries (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (reason = 'missing_runtime_start_intent'),
    state TEXT NOT NULL CHECK (state IN ('pending', 'settled')),
    detected_at_ms INTEGER NOT NULL CHECK (detected_at_ms >= 0),
    settled_revision INTEGER CHECK (settled_revision > 0),
    settled_at_ms INTEGER CHECK (settled_at_ms >= detected_at_ms),
    settlement_digest TEXT,
    settlement_event_ids_json TEXT,
    CHECK ((state = 'settled') = (settled_revision IS NOT NULL)),
    CHECK ((state = 'settled') = (settled_at_ms IS NOT NULL)),
    CHECK ((state = 'settled') = (settlement_digest IS NOT NULL)),
    CHECK ((state = 'settled') = (settlement_event_ids_json IS NOT NULL))
) STRICT;

INSERT INTO legacy_runtime_start_recoveries(
    task_id, run_id, reason, state, detected_at_ms
)
SELECT
    t.task_id,
    json_extract(t.snapshot_json, '$.active_run_id'),
    'missing_runtime_start_intent',
    'pending',
    CAST(unixepoch('subsec') * 1000 AS INTEGER)
FROM tasks t
WHERE t.state = 'queued'
  AND json_type(t.snapshot_json, '$.active_run_id') = 'text'
  AND NOT EXISTS (
      SELECT 1
      FROM outbox o
      WHERE o.task_id = t.task_id
        AND o.delivery_kind = 'runtime_start'
        AND json_extract(o.payload_json, '$.run_id') =
            json_extract(t.snapshot_json, '$.active_run_id')
  );
"#,
    },
    Migration {
        version: 6,
        checksum: "cosh-gateway-brokered-execution-v6-20260816-fenced-claim-audit",
        sql: r#"
ALTER TABLE approvals ADD COLUMN target_identity_digest TEXT;
ALTER TABLE approvals ADD COLUMN runtime_fence_json TEXT;

ALTER TABLE permits ADD COLUMN target_identity_digest TEXT;
ALTER TABLE permits ADD COLUMN runtime_fence_json TEXT;

-- Pre-v6 permits cannot prove immutable target or Runtime authority. They are
-- retained for audit but must never remain executable after upgrade.
UPDATE permits SET state = 'revoked' WHERE state = 'issued';

ALTER TABLE executions ADD COLUMN target_identity_digest TEXT;
ALTER TABLE executions ADD COLUMN runtime_fence_json TEXT;
ALTER TABLE executions ADD COLUMN broker_state TEXT CHECK (
    broker_state IS NULL OR broker_state IN ('planned', 'claimed', 'started', 'known_no_effect')
);
ALTER TABLE executions ADD COLUMN claimed_at_ms INTEGER CHECK (claimed_at_ms >= 0);
ALTER TABLE executions ADD COLUMN start_audit_proof_digest TEXT;

CREATE TABLE brokered_requests (
    request_id TEXT PRIMARY KEY NOT NULL,
    approval_id TEXT UNIQUE REFERENCES approvals(approval_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    operation_json TEXT NOT NULL,
    typed_operation_digest TEXT NOT NULL,
    operation_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    target_identity_digest TEXT NOT NULL,
    runtime_fence_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE security_audit_proofs (
    execution_id TEXT PRIMARY KEY NOT NULL
        REFERENCES executions(execution_id) ON DELETE RESTRICT,
    proof_digest TEXT NOT NULL,
    durability TEXT NOT NULL CHECK (durability = 'security_boundary'),
    persisted_at_ms INTEGER NOT NULL CHECK (persisted_at_ms >= 0)
) STRICT;

CREATE INDEX brokered_requests_task_run
    ON brokered_requests(task_id, run_id, created_at_ms);
CREATE INDEX executions_broker_recovery
    ON executions(broker_state, updated_at_ms);
"#,
    },
    Migration {
        version: 7,
        checksum: "cosh-gateway-brokered-callback-dispatch-v7-20260816-fenced",
        sql: r#"
CREATE TABLE brokered_runtime_dispatches (
    request_id TEXT NOT NULL
        REFERENCES brokered_requests(request_id) ON DELETE RESTRICT,
    dispatch_kind TEXT NOT NULL CHECK (dispatch_kind IN ('acknowledgement', 'result')),
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    brokered_ref_json TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('approval_pending', 'approval_denied', 'execution')),
    source_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'started', 'delivered', 'unknown')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    PRIMARY KEY(request_id, dispatch_kind),
    CHECK ((dispatch_kind = 'acknowledgement') = (source_kind = 'approval_pending')),
    CHECK ((dispatch_kind = 'result') = (source_kind IN ('approval_denied', 'execution')))
) STRICT;

CREATE INDEX brokered_runtime_dispatches_recovery
    ON brokered_runtime_dispatches(state, updated_at_ms);
"#,
    },
    Migration {
        version: 8,
        checksum: "cosh-gateway-brokered-result-v8-20260816-typed-atomic",
        sql: r#"
ALTER TABLE executions ADD COLUMN typed_result_state TEXT CHECK (
    typed_result_state IN ('not_applicable', 'available', 'legacy_unavailable')
);

UPDATE executions
SET typed_result_state = CASE
    WHEN state = 'succeeded' THEN 'legacy_unavailable'
    ELSE 'not_applicable'
END;

CREATE TABLE brokered_execution_results (
    execution_id TEXT PRIMARY KEY NOT NULL
        REFERENCES executions(execution_id) ON DELETE RESTRICT,
    request_id TEXT NOT NULL
        REFERENCES brokered_requests(request_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    result_json TEXT NOT NULL,
    result_digest TEXT NOT NULL,
    operation_json TEXT NOT NULL,
    operation_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    target_identity_digest TEXT NOT NULL,
    runtime_fence_json TEXT NOT NULL,
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0)
) STRICT;

CREATE INDEX brokered_execution_results_request
    ON brokered_execution_results(request_id, committed_at_ms);
"#,
    },
    Migration {
        version: 9,
        checksum: "cosh-gateway-runtime-input-v9-20260816-fenced-private-response",
        sql: r#"
CREATE TABLE runtime_input_requests (
    request_id TEXT PRIMARY KEY NOT NULL,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    binding_id TEXT NOT NULL REFERENCES runtime_bindings(binding_id) ON DELETE RESTRICT,
    runtime_instance_id TEXT NOT NULL,
    runtime_generation INTEGER NOT NULL CHECK (runtime_generation > 0),
    runtime_sequence INTEGER NOT NULL CHECK (runtime_sequence > 0),
    lease_generation INTEGER NOT NULL CHECK (lease_generation > 0),
    lease_revision INTEGER NOT NULL CHECK (lease_revision > 0),
    request_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'resolved', 'expired', 'cancelled')),
    response_digest TEXT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    CHECK ((state = 'resolved') = (response_digest IS NOT NULL))
) STRICT;

CREATE INDEX runtime_input_requests_run_state
    ON runtime_input_requests(run_id, state, updated_at_ms);

CREATE TABLE runtime_input_dispatches (
    request_id TEXT PRIMARY KEY NOT NULL
        REFERENCES runtime_input_requests(request_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    response_json TEXT NOT NULL,
    response_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'started', 'delivered', 'unknown')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX runtime_input_dispatches_recovery
    ON runtime_input_dispatches(state, updated_at_ms);
"#,
    },
];

pub(super) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    migrate_through(connection, CURRENT_SCHEMA_VERSION)
}

pub(super) fn preflight_existing(connection: &Connection) -> Result<(), StoreError> {
    validate_schema_history(connection, CURRENT_SCHEMA_VERSION)?;
    validate_integrity(connection)
}

#[cfg(test)]
pub(super) fn migrate_to_for_test(
    connection: &mut Connection,
    target_version: u32,
) -> Result<(), StoreError> {
    migrate_through(connection, target_version)
}

fn migrate_through(connection: &mut Connection, target_version: u32) -> Result<(), StoreError> {
    validate_schema_history(connection, target_version)?;
    validate_integrity(connection)?;

    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY NOT NULL CHECK (version > 0),
             checksum TEXT NOT NULL,
             applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
         ) STRICT;
         COMMIT;",
    )?;

    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= target_version)
    {
        let existing = connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                params![migration.version],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match existing {
            Some(checksum) if checksum == migration.checksum => continue,
            Some(_) => {
                return Err(StoreError::MigrationChecksum {
                    version: migration.version,
                });
            }
            None => apply_migration(connection, migration)?,
        }
    }

    validate_integrity(connection)
}

fn validate_schema_history(
    connection: &Connection,
    supported_version: u32,
) -> Result<(), StoreError> {
    let has_migration_table = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_schema
             WHERE type = 'table' AND name = 'schema_migrations'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_migration_table {
        let object_count = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        if object_count == 0 {
            return Ok(());
        }
        return Err(StoreError::Corrupt {
            message: "database objects exist without schema migration history".to_owned(),
        });
    }

    let found = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get::<_, u32>(0),
    )?;
    if found > supported_version {
        return Err(StoreError::NewerSchema {
            found,
            supported: supported_version,
        });
    }

    let mut statement =
        connection.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
    })?;
    let history = rows.collect::<Result<Vec<_>, _>>()?;

    for (index, (version, checksum)) in history.iter().enumerate() {
        let expected_version = u32::try_from(index + 1).map_err(|_| StoreError::Corrupt {
            message: "schema migration history exceeds supported integer range".to_owned(),
        })?;
        if *version != expected_version {
            return Err(StoreError::Corrupt {
                message: format!(
                    "schema migration history is not contiguous: expected version {expected_version}, found {version}"
                ),
            });
        }
        let migration = MIGRATIONS
            .iter()
            .find(|migration| migration.version == *version)
            .ok_or(StoreError::NewerSchema {
                found: *version,
                supported: supported_version,
            })?;
        if checksum != migration.checksum {
            return Err(StoreError::MigrationChecksum { version: *version });
        }
    }

    if history.is_empty() {
        let object_count = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' AND name != 'schema_migrations'",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        if object_count != 0 {
            return Err(StoreError::Corrupt {
                message: "database objects exist without recorded migrations".to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_integrity(connection: &Connection) -> Result<(), StoreError> {
    let integrity: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StoreError::Corrupt {
            message: format!("SQLite quick_check failed: {integrity}"),
        });
    }
    validate_foreign_keys(connection)?;
    Ok(())
}

fn validate_foreign_keys(connection: &Connection) -> Result<(), StoreError> {
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .optional()?;
    if let Some((table, row_id, parent, constraint)) = violation {
        return Err(StoreError::Corrupt {
            message: format!(
                "SQLite foreign_key_check failed: table={table}, rowid={row_id:?}, parent={parent}, constraint={constraint}"
            ),
        });
    }
    Ok(())
}

fn apply_migration(connection: &mut Connection, migration: &Migration) -> Result<(), StoreError> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute_batch(migration.sql)?;
    validate_foreign_keys(&transaction)?;
    record_migration(&transaction, migration)?;
    transaction.commit()?;
    Ok(())
}

fn record_migration(
    transaction: &Transaction<'_>,
    migration: &Migration,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO schema_migrations(version, checksum, applied_at_ms)
         VALUES (?1, ?2, CAST(unixepoch('subsec') * 1000 AS INTEGER))",
        params![migration.version, migration.checksum],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_repeatable_and_enables_all_tables() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate(&mut connection).unwrap();
        migrate(&mut connection).unwrap();

        let tables = connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            tables,
            [
                "approvals",
                "brokered_execution_results",
                "brokered_requests",
                "brokered_runtime_dispatches",
                "command_receipts",
                "execution_receipts",
                "executions",
                "gateway_identity",
                "ledger_receipts",
                "legacy_runtime_start_recoveries",
                "outbox",
                "permits",
                "provider_permission_dispatches",
                "run_leases",
                "runtime_bindings",
                "runtime_input_dispatches",
                "runtime_input_requests",
                "schema_migrations",
                "security_audit_proofs",
                "task_events",
                "tasks"
            ]
        );
    }

    #[test]
    fn existing_v1_database_migrates_without_rewriting_v1() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                     version INTEGER PRIMARY KEY NOT NULL CHECK (version > 0),
                     checksum TEXT NOT NULL,
                     applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
                 ) STRICT;",
            )
            .unwrap();
        apply_migration(&mut connection, &MIGRATIONS[0]).unwrap();

        migrate(&mut connection).unwrap();

        let versions = connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| row.get::<_, u32>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(versions, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let v1_checksum: String = connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v1_checksum, MIGRATIONS[0].checksum);
    }

    #[test]
    fn existing_v8_database_adds_private_runtime_input_tables() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate_to_for_test(&mut connection, 8).unwrap();
        let before: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type='table' AND name LIKE 'runtime_input_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 0);

        migrate(&mut connection).unwrap();

        let version: u32 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 9);
        let tables = connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type='table' AND name LIKE 'runtime_input_%' ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            tables,
            ["runtime_input_dispatches", "runtime_input_requests"]
        );
    }

    #[test]
    fn newer_schema_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version, checksum, applied_at_ms)
                 VALUES (?1, 'future', 0)",
                [CURRENT_SCHEMA_VERSION + 1],
            )
            .unwrap();

        assert!(matches!(
            migrate(&mut connection),
            Err(StoreError::NewerSchema { .. })
        ));
    }

    #[test]
    fn v5_provider_approval_migrates_without_manufacturing_brokered_authority() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate_to_for_test(&mut connection, 5).unwrap();
        connection
            .execute_batch(
                "INSERT INTO tasks(
                     task_id, owner_actor_id, target_ref, revision, state,
                     snapshot_json, created_at_ms, updated_at_ms)
                 VALUES ('task', 'actor', '{}', 1, 'running', '{}', 1, 1);
                 INSERT INTO approvals(
                     approval_id, request_id, actor_id, task_id, run_id, target_json,
                     operation_digest, input_digest, state, revision, expires_at_ms,
                     created_at_ms, updated_at_ms, permission_ref_json)
                 VALUES (
                     'approval', 'request', 'actor', 'task', 'run', '{}',
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     'pending', 1, 100, 1, 1, '{\"runtime_generation\":1}'
                 );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let row = connection
            .query_row(
                "SELECT permission_ref_json, target_identity_digest, runtime_fence_json
                 FROM approvals WHERE approval_id='approval'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("{\"runtime_generation\":1}"));
        assert_eq!(row.1, None);
        assert_eq!(row.2, None);
    }

    #[test]
    fn checksum_mismatch_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = 'changed' WHERE version = 1",
                [],
            )
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(StoreError::MigrationChecksum { version: 1 })
        ));
    }

    #[test]
    fn migration_history_must_be_contiguous() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate_to_for_test(&mut connection, 3).unwrap();
        connection
            .execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();

        let error = migrate(&mut connection).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Corrupt { message }
                if message.contains("not contiguous")
        ));
    }

    #[test]
    fn migration_fk_failure_rolls_back_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrate_to_for_test(&mut connection, 1).unwrap();
        let invalid_migration = Migration {
            version: 2,
            checksum: "test-invalid-foreign-key",
            sql: r#"
PRAGMA defer_foreign_keys = ON;
INSERT INTO outbox(
    delivery_id, task_id, event_id, delivery_kind, payload_json, state,
    attempt, next_attempt_at_ms, lease_owner, lease_expires_at_ms,
    created_at_ms, delivered_at_ms
) VALUES (
    'orphan-delivery', 'missing-task', 'missing-event', 'runtime_start', '{}',
    'pending', 0, 0, NULL, NULL, 0, NULL
);
"#,
        };

        let error = apply_migration(&mut connection, &invalid_migration).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Corrupt { message }
                if message.contains("foreign_key_check")
        ));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM outbox WHERE delivery_id = 'orphan-delivery'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap(),
            0
        );
    }
}
