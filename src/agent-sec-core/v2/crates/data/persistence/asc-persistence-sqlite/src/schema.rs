use rusqlite::{Connection, Transaction};

use crate::store::StoreOpenError;

const CURRENT_SCHEMA_VERSION: i64 = 3;

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), StoreOpenError> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| StoreOpenError)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| StoreOpenError)?;
    let transaction = connection.transaction().map_err(|_| StoreOpenError)?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO schema_version(singleton, version) VALUES (1, 3);",
        )
        .map_err(|_| StoreOpenError)?;

    let version: i64 = transaction
        .query_row(
            "SELECT version FROM schema_version WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StoreOpenError)?;
    match version {
        1 => {
            migrate_v1_to_v2(&transaction)?;
            create_v2_schema(&transaction)?;
            migrate_v2_to_v3(&transaction)?;
        }
        2 => {
            create_v2_schema(&transaction)?;
            migrate_v2_to_v3(&transaction)?;
        }
        CURRENT_SCHEMA_VERSION => {}
        _ => return Err(StoreOpenError),
    }
    create_v3_schema(&transaction)?;

    transaction
        .execute(
            "UPDATE operations SET state = 'QUEUED', error_code = NULL
             WHERE state = 'DISPATCHING'",
            [],
        )
        .map_err(|_| StoreOpenError)?;
    transaction
        .execute(
            "UPDATE outbox SET state = 'QUEUED' WHERE state = 'DISPATCHING'",
            [],
        )
        .map_err(|_| StoreOpenError)?;
    transaction.commit().map_err(|_| StoreOpenError)
}

fn create_v2_schema(transaction: &Transaction<'_>) -> Result<(), StoreOpenError> {
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS policy_revision_heads (
                policy_id TEXT PRIMARY KEY,
                last_allocated_revision INTEGER NOT NULL CHECK (last_allocated_revision > 0)
            );
            CREATE TABLE IF NOT EXISTS policy_revisions (
                policy_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(policy_id, revision)
            );
            CREATE TABLE IF NOT EXISTS scope_revisions (
                scope_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                retired INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
                PRIMARY KEY(scope_id, revision)
            );
            CREATE TABLE IF NOT EXISTS bindings (
                binding_id TEXT PRIMARY KEY,
                revision INTEGER NOT NULL CHECK (revision > 0),
                policy_id TEXT NOT NULL,
                policy_revision INTEGER NOT NULL,
                scope_id TEXT NOT NULL,
                scope_revision INTEGER NOT NULL,
                desired_state TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS operations (
                operation_id TEXT PRIMARY KEY,
                binding_id TEXT NOT NULL,
                binding_revision INTEGER NOT NULL,
                request_digest TEXT NOT NULL,
                state TEXT NOT NULL,
                stage TEXT NOT NULL,
                error_code TEXT,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_operations_binding
                ON operations(binding_id, binding_revision);
            CREATE TABLE IF NOT EXISTS outbox (
                operation_id TEXT PRIMARY KEY REFERENCES operations(operation_id),
                state TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS policy_admin_audit (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                principal TEXT NOT NULL,
                method TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                operation_id TEXT,
                outcome TEXT NOT NULL
            );",
        )
        .map_err(|_| StoreOpenError)
}

fn create_v3_schema(transaction: &Transaction<'_>) -> Result<(), StoreOpenError> {
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS policy_revision_heads (
                policy_id TEXT PRIMARY KEY,
                last_allocated_revision INTEGER NOT NULL CHECK (last_allocated_revision > 0)
            );
            CREATE TABLE IF NOT EXISTS policy_revisions (
                policy_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(policy_id, revision)
            );
            CREATE TABLE IF NOT EXISTS scope_revision_heads (
                scope_id TEXT PRIMARY KEY,
                last_allocated_revision INTEGER NOT NULL CHECK (last_allocated_revision > 0)
            );
            CREATE TABLE IF NOT EXISTS scope_revisions (
                scope_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(scope_id, revision)
            );
            CREATE TABLE IF NOT EXISTS bindings (
                binding_id TEXT PRIMARY KEY,
                revision INTEGER NOT NULL CHECK (revision > 0),
                policy_id TEXT NOT NULL,
                policy_revision INTEGER NOT NULL,
                scope_id TEXT NOT NULL,
                scope_revision INTEGER NOT NULL,
                desired_state TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS operations (
                operation_id TEXT PRIMARY KEY,
                binding_id TEXT NOT NULL,
                binding_revision INTEGER NOT NULL,
                request_digest TEXT NOT NULL,
                state TEXT NOT NULL,
                stage TEXT NOT NULL,
                error_code TEXT,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_operations_binding
                ON operations(binding_id, binding_revision);
            CREATE TABLE IF NOT EXISTS outbox (
                operation_id TEXT PRIMARY KEY REFERENCES operations(operation_id),
                state TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS policy_admin_audit (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                principal TEXT NOT NULL,
                method TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                operation_id TEXT,
                outcome TEXT NOT NULL
            );",
        )
        .map_err(|_| StoreOpenError)
}

fn migrate_v1_to_v2(transaction: &Transaction<'_>) -> Result<(), StoreOpenError> {
    transaction
        .execute_batch(
            "CREATE TABLE policy_revision_heads (
                policy_id TEXT PRIMARY KEY,
                last_allocated_revision INTEGER NOT NULL CHECK (last_allocated_revision > 0)
            );
            INSERT INTO policy_revision_heads(policy_id, last_allocated_revision)
                SELECT policy_id, MAX(revision)
                FROM policy_revisions
                GROUP BY policy_id;

            CREATE TABLE policy_revisions_v2 (
                policy_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(policy_id, revision)
            );
            INSERT INTO policy_revisions_v2(policy_id, revision, template_digest, record_json)
                SELECT policy_id, revision, template_digest, record_json
                FROM policy_revisions
                WHERE retired = 0;
            DROP TABLE policy_revisions;
            ALTER TABLE policy_revisions_v2 RENAME TO policy_revisions;
            UPDATE schema_version SET version = 2 WHERE singleton = 1;",
        )
        .map_err(|_| StoreOpenError)
}

fn migrate_v2_to_v3(transaction: &Transaction<'_>) -> Result<(), StoreOpenError> {
    transaction
        .execute_batch(
            "CREATE TABLE scope_revision_heads (
                scope_id TEXT PRIMARY KEY,
                last_allocated_revision INTEGER NOT NULL CHECK (last_allocated_revision > 0)
            );
            INSERT INTO scope_revision_heads(scope_id, last_allocated_revision)
                SELECT scope_id, MAX(revision)
                FROM scope_revisions
                GROUP BY scope_id;

            CREATE TABLE scope_revisions_v3 (
                scope_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                template_digest TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(scope_id, revision)
            );
            INSERT INTO scope_revisions_v3(scope_id, revision, template_digest, record_json)
                SELECT scope_id, revision, template_digest, record_json
                FROM scope_revisions
                WHERE retired = 0;
            DROP TABLE scope_revisions;
            ALTER TABLE scope_revisions_v3 RENAME TO scope_revisions;
            UPDATE schema_version SET version = 3 WHERE singleton = 1;",
        )
        .map_err(|_| StoreOpenError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_retired_rows_become_deleted_but_preserve_the_allocation_head() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_version (
                    singleton INTEGER PRIMARY KEY,
                    version INTEGER NOT NULL
                );
                INSERT INTO schema_version VALUES (1, 1);
                CREATE TABLE policy_revisions (
                    policy_id TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    template_digest TEXT NOT NULL,
                    record_json TEXT NOT NULL,
                    retired INTEGER NOT NULL,
                    PRIMARY KEY(policy_id, revision)
                );
                INSERT INTO policy_revisions VALUES ('policy-1', 1, 'digest-1', '{}', 0);
                INSERT INTO policy_revisions VALUES ('policy-1', 2, 'digest-2', '{}', 1);
                CREATE TABLE operations (
                    operation_id TEXT PRIMARY KEY,
                    binding_id TEXT NOT NULL,
                    binding_revision INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    error_code TEXT
                );
                CREATE TABLE outbox (
                    operation_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL
                );",
            )
            .unwrap();

        initialize(&mut connection).unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_version WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM policy_revisions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT last_allocated_revision FROM policy_revision_heads
                     WHERE policy_id = 'policy-1'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        let has_retired_column = connection
            .prepare("PRAGMA table_info(policy_revisions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .any(|name| name == "retired");
        assert!(!has_retired_column);
    }

    #[test]
    fn v2_retired_scopes_become_deleted_but_preserve_the_allocation_head() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_version (
                    singleton INTEGER PRIMARY KEY,
                    version INTEGER NOT NULL
                );
                INSERT INTO schema_version VALUES (1, 2);
                CREATE TABLE scope_revisions (
                    scope_id TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    template_digest TEXT NOT NULL,
                    record_json TEXT NOT NULL,
                    retired INTEGER NOT NULL,
                    PRIMARY KEY(scope_id, revision)
                );
                INSERT INTO scope_revisions VALUES ('scope-1', 1, 'digest-1', '{}', 0);
                INSERT INTO scope_revisions VALUES ('scope-1', 2, 'digest-2', '{}', 1);
                CREATE TABLE operations (
                    operation_id TEXT PRIMARY KEY,
                    binding_id TEXT NOT NULL,
                    binding_revision INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    error_code TEXT
                );
                CREATE TABLE outbox (
                    operation_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL
                );",
            )
            .unwrap();

        initialize(&mut connection).unwrap();

        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM scope_revisions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT last_allocated_revision FROM scope_revision_heads
                     WHERE scope_id = 'scope-1'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        let has_retired_column = connection
            .prepare("PRAGMA table_info(scope_revisions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .any(|name| name == "retired");
        assert!(!has_retired_column);
    }
}
