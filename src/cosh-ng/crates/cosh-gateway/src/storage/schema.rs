//! Checksummed SQLite schema migrations for Gateway Task storage.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::StoreError;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 1;

struct Migration {
    version: u32,
    checksum: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[Migration {
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
}];

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
                "command_receipts",
                "outbox",
                "schema_migrations",
                "task_events",
                "tasks"
            ]
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
