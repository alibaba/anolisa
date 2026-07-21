//! SQLite persistence for desired bindings and violation facts.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{Binding, BindingState, ViolationEvent};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;

const MIN_REASONABLE_UNIX_EPOCH_NS: u64 = 946_684_800_000_000_000;

/// Persistence failures for enforcement state.
#[derive(Debug, Error)]
pub enum EnforcementStoreError {
    /// The shared SQLite helper could not open or configure the database.
    #[error("open enforcement database: {0}")]
    Open(String),
    /// SQLite open, schema, or query failure.
    #[error("enforcement SQLite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Binding or violation JSON could not be encoded or decoded.
    #[error("enforcement JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Another thread panicked while holding the connection.
    #[error("enforcement database mutex is poisoned")]
    Poisoned,
    /// A requested binding is not persisted.
    #[error("binding {0} is not persisted")]
    MissingBinding(Uuid),
    /// A stable idempotency key names a different desired request.
    #[error("binding conflict for {0}")]
    BindingConflict(Uuid),
}

/// Thread-safe local enforcement state.
#[derive(Clone)]
pub struct EnforcementStore {
    connection: Arc<Mutex<Connection>>,
}

impl EnforcementStore {
    /// Opens a database and creates backward-compatible tables and indexes.
    ///
    /// # Errors
    ///
    /// Returns a SQLite or JSON error when initialization or legacy migration fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EnforcementStoreError> {
        let path = path.as_ref();
        let mut connection = if path == Path::new(":memory:") {
            Connection::open_in_memory()?
        } else {
            crate::storage::sqlite::create_connection(path)
                .map_err(|error| EnforcementStoreError::Open(error.to_string()))?
        };
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS enforcement_bindings (
                binding_id TEXT PRIMARY KEY,
                desired_json TEXT NOT NULL,
                state TEXT NOT NULL,
                message TEXT,
                domain_id INTEGER,
                updated_at_ns INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS enforcement_violations (
                event_id TEXT PRIMARY KEY,
                binding_id TEXT NOT NULL,
                occurred_at_ns INTEGER NOT NULL,
                event_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_enforcement_violations_time
                ON enforcement_violations(occurred_at_ns DESC);",
        )?;
        migrate_legacy_violation_timestamps(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Inserts or updates the latest binding state.
    ///
    /// # Errors
    ///
    /// Returns a mutex, serialization, SQLite, or immutable-request conflict error.
    pub fn upsert_binding(&self, binding: &Binding) -> Result<(), EnforcementStoreError> {
        let json = serde_json::to_string(binding)?;
        let connection = self.connection()?;
        let existing_json: Option<String> = connection
            .query_row(
                "SELECT desired_json FROM enforcement_bindings WHERE binding_id = ?1",
                params![binding.request.binding_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing_json) = existing_json {
            let existing: Binding = serde_json::from_str(&existing_json)?;
            if existing.request != binding.request {
                return Err(EnforcementStoreError::BindingConflict(
                    binding.request.binding_id,
                ));
            }
        }
        connection.execute(
            "INSERT INTO enforcement_bindings
               (binding_id, desired_json, state, message, domain_id, updated_at_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(binding_id) DO UPDATE SET
               desired_json=excluded.desired_json,
               state=excluded.state,
               message=excluded.message,
               domain_id=excluded.domain_id,
               updated_at_ns=excluded.updated_at_ns",
            params![
                binding.request.binding_id.to_string(),
                json,
                state_name(binding.state),
                binding.message.as_deref(),
                binding.domain_id.map(i64::from),
                sqlite_i64(now_ns()),
            ],
        )?;
        Ok(())
    }

    /// Reads one binding by ID.
    ///
    /// # Errors
    ///
    /// Returns a mutex, deserialization, or SQLite error.
    pub fn binding(&self, binding_id: Uuid) -> Result<Option<Binding>, EnforcementStoreError> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT desired_json FROM enforcement_bindings WHERE binding_id = ?1",
                params![binding_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    /// Lists all persisted bindings in stable ID order.
    ///
    /// # Errors
    ///
    /// Returns a mutex, deserialization, or SQLite error.
    pub fn bindings(&self) -> Result<Vec<Binding>, EnforcementStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT desired_json FROM enforcement_bindings ORDER BY binding_id ASC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(serde_json::from_str(&row?)?);
        }
        Ok(bindings)
    }

    /// Inserts a violation once by event ID.
    ///
    /// # Errors
    ///
    /// Returns a mutex, serialization, or SQLite error.
    pub fn insert_violation(&self, event: &ViolationEvent) -> Result<bool, EnforcementStoreError> {
        let changed = self.connection()?.execute(
            "INSERT OR IGNORE INTO enforcement_violations
               (event_id, binding_id, occurred_at_ns, event_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event.event_id.to_string(),
                event.binding_id.to_string(),
                sqlite_i64(event.occurred_at_ns),
                serde_json::to_string(event)?,
            ],
        )?;
        Ok(changed == 1)
    }

    /// Lists newest violations with a limit clamped to `1..=1000`.
    ///
    /// # Errors
    ///
    /// Returns a mutex, deserialization, or SQLite error.
    pub fn violations(&self, limit: usize) -> Result<Vec<ViolationEvent>, EnforcementStoreError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_json FROM enforcement_violations
             ORDER BY occurred_at_ns DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 1000) as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(serde_json::from_str(&row?)?);
        }
        Ok(events)
    }

    /// Marks active desired state degraded without deleting it.
    ///
    /// # Errors
    ///
    /// Returns a mutex, deserialization, serialization, or SQLite error.
    pub fn mark_active_degraded(&self, message: &str) -> Result<(), EnforcementStoreError> {
        let mut bindings = self.bindings()?;
        for binding in &mut bindings {
            if matches!(
                binding.state,
                BindingState::Pending | BindingState::Enforced | BindingState::Degraded
            ) {
                binding.state = BindingState::Degraded;
                binding.message = Some(message.to_string());
                self.upsert_binding(binding)?;
            }
        }
        Ok(())
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, EnforcementStoreError> {
        self.connection
            .lock()
            .map_err(|_| EnforcementStoreError::Poisoned)
    }
}

fn migrate_legacy_violation_timestamps(
    connection: &mut Connection,
) -> Result<(), EnforcementStoreError> {
    let transaction = connection.transaction()?;
    let candidates = {
        let mut statement = transaction.prepare(
            "SELECT event_id, event_json FROM enforcement_violations
             WHERE occurred_at_ns < ?1",
        )?;
        let rows = statement
            .query_map(params![sqlite_i64(MIN_REASONABLE_UNIX_EPOCH_NS)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(row?);
        }
        candidates
    };

    for (event_id, event_json) in candidates {
        let mut event: ViolationEvent = serde_json::from_str(&event_json)?;
        if event.occurred_at_ns >= MIN_REASONABLE_UNIX_EPOCH_NS
            || !is_reasonable_unix_epoch_ns(event.observed_at_ns)
        {
            continue;
        }
        event.occurred_at_ns = event.observed_at_ns;
        transaction.execute(
            "UPDATE enforcement_violations
             SET occurred_at_ns = ?1, event_json = ?2
             WHERE event_id = ?3 AND occurred_at_ns < ?4",
            params![
                event.observed_at_ns as i64,
                serde_json::to_string(&event)?,
                event_id,
                sqlite_i64(MIN_REASONABLE_UNIX_EPOCH_NS),
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn is_reasonable_unix_epoch_ns(value: u64) -> bool {
    (MIN_REASONABLE_UNIX_EPOCH_NS..=i64::MAX as u64).contains(&value)
}

fn state_name(state: BindingState) -> &'static str {
    match state {
        BindingState::Pending => "pending",
        BindingState::Enforced => "enforced",
        BindingState::Failed => "failed",
        BindingState::Degraded => "degraded",
        BindingState::Detaching => "detaching",
        BindingState::Detached => "detached",
    }
}

fn now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.min(u64::MAX as u128) as u64
}

fn sqlite_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use agentsight_enforcement_protocol::Effect;

    use super::*;

    const LEGACY_OCCURRED_AT_NS: u64 = 270_000_000_000_000;
    const LEGACY_OBSERVED_AT_NS: u64 = 1_784_000_000_000_000_000;
    const VALID_OCCURRED_AT_NS: u64 = 1_783_000_000_000_000_000;

    struct TestDatabase {
        path: PathBuf,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "agentsight-enforcement-store-{}.db",
                    Uuid::new_v4()
                )),
            }
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(format!("{}-wal", self.path.display()));
            let _ = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    fn violation(occurred_at_ns: u64, observed_at_ns: u64) -> ViolationEvent {
        ViolationEvent {
            event_id: Uuid::new_v4(),
            binding_id: Uuid::new_v4(),
            agent_id: "test-agent".into(),
            session_id: Some("test-session".into()),
            policy_id: "test-policy".into(),
            policy_revision: "revision-1".into(),
            pid: 42,
            ppid: Some(1),
            process_start_time: 99,
            operation: "open".into(),
            target: "/tmp/secret".into(),
            effect: Effect::Block,
            blocked: true,
            killed: false,
            rule_id: Some("block-secret".into()),
            reason: Some("test fixture".into()),
            occurred_at_ns,
            observed_at_ns,
            actplane_revision: "test-revision".into(),
        }
    }

    fn raw_violation(path: &Path, event_id: Uuid) -> (i64, String) {
        Connection::open(path)
            .expect("test database should open")
            .query_row(
                "SELECT occurred_at_ns, event_json
                 FROM enforcement_violations WHERE event_id = ?1",
                params![event_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("test violation should exist")
    }

    #[test]
    fn open_migrates_legacy_violation_idempotently_and_repairs_ordering() {
        let database = TestDatabase::new();
        let legacy = violation(LEGACY_OCCURRED_AT_NS, LEGACY_OBSERVED_AT_NS);
        let valid = violation(VALID_OCCURRED_AT_NS, VALID_OCCURRED_AT_NS + 10);
        let store = EnforcementStore::open(&database.path).expect("test store should open");
        store
            .insert_violation(&legacy)
            .expect("legacy violation should insert");
        store
            .insert_violation(&valid)
            .expect("valid violation should insert");
        drop(store);

        let valid_before = raw_violation(&database.path, valid.event_id);
        let reopened = EnforcementStore::open(&database.path).expect("test store should reopen");
        let events = reopened
            .violations(10)
            .expect("migrated violations should load");
        assert_eq!(events[0].event_id, legacy.event_id);
        assert_eq!(events[0].occurred_at_ns, LEGACY_OBSERVED_AT_NS);
        assert_eq!(events[1], valid);
        drop(reopened);

        let legacy_after_first_open = raw_violation(&database.path, legacy.event_id);
        assert_eq!(legacy_after_first_open.0, LEGACY_OBSERVED_AT_NS as i64);
        let migrated_json: ViolationEvent = serde_json::from_str(&legacy_after_first_open.1)
            .expect("migrated event JSON should deserialize");
        assert_eq!(migrated_json.occurred_at_ns, LEGACY_OBSERVED_AT_NS);
        assert_eq!(raw_violation(&database.path, valid.event_id), valid_before);

        drop(EnforcementStore::open(&database.path).expect("test store should reopen twice"));
        assert_eq!(
            raw_violation(&database.path, legacy.event_id),
            legacy_after_first_open
        );
        assert_eq!(raw_violation(&database.path, valid.event_id), valid_before);
    }

    #[test]
    fn open_propagates_malformed_legacy_event_json() {
        let database = TestDatabase::new();
        let legacy = violation(LEGACY_OCCURRED_AT_NS, LEGACY_OBSERVED_AT_NS);
        let store = EnforcementStore::open(&database.path).expect("test store should open");
        store
            .insert_violation(&legacy)
            .expect("legacy violation should insert");
        drop(store);
        Connection::open(&database.path)
            .expect("test database should open")
            .execute(
                "UPDATE enforcement_violations SET event_json = '{' WHERE event_id = ?1",
                params![legacy.event_id.to_string()],
            )
            .expect("legacy JSON should be corrupted");

        match EnforcementStore::open(&database.path) {
            Err(EnforcementStoreError::Json(_)) => {}
            Err(error) => panic!("expected JSON error, got {error}"),
            Ok(_) => panic!("malformed legacy JSON should fail store open"),
        }
    }
}
