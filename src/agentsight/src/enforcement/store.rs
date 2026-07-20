//! SQLite persistence for desired bindings and violation facts.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{Binding, BindingState, ViolationEvent};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;

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
    /// Returns a SQLite error when the file or schema cannot be initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EnforcementStoreError> {
        let path = path.as_ref();
        let connection = if path == Path::new(":memory:") {
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
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Inserts or updates the latest binding state.
    ///
    /// # Errors
    ///
    /// Returns a mutex, serialization, or SQLite error.
    pub fn upsert_binding(&self, binding: &Binding) -> Result<(), EnforcementStoreError> {
        let json = serde_json::to_string(binding)?;
        self.connection()?.execute(
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
                BindingState::Pending | BindingState::Enforced | BindingState::Detaching
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
