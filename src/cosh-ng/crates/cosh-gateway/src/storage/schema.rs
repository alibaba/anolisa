//! Checksummed SQLite schema migrations for Gateway Task storage.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::StoreError;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 2;

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
];

pub(super) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY NOT NULL CHECK (version > 0),
             checksum TEXT NOT NULL,
             applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
         ) STRICT;
         COMMIT;",
    )?;

    let found = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get::<_, u32>(0),
    )?;
    if found > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::NewerSchema {
            found,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    for migration in MIGRATIONS {
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

    let integrity: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StoreError::Corrupt {
            message: format!("SQLite quick_check failed: {integrity}"),
        });
    }
    Ok(())
}

fn apply_migration(connection: &mut Connection, migration: &Migration) -> Result<(), StoreError> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute_batch(migration.sql)?;
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
                "command_receipts",
                "execution_receipts",
                "executions",
                "ledger_receipts",
                "outbox",
                "permits",
                "run_leases",
                "runtime_bindings",
                "schema_migrations",
                "task_events",
                "tasks"
            ]
        );
    }

    #[test]
    fn existing_v1_database_migrates_to_v2_without_rewriting_v1() {
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
        assert_eq!(versions, [1, 2]);
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
}
