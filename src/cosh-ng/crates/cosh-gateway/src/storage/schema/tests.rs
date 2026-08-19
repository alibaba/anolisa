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
