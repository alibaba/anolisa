//! SQLite persistence for immutable normalized security events.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use agentsight_enforcement_protocol::{
    DestinationClass, SecurityEvent, SecurityEventKind,
};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;

use super::{SecurityCountBy, SecurityEventFilter, SecurityEventPage, SecuritySummary};
use crate::storage::sqlite::{create_connection, default_base_path};

const EVENT_QUERY: &str =
    "SELECT event_json FROM security_events
     WHERE (?1 IS NULL OR occurred_at_ns >= ?1)
       AND (?2 IS NULL OR occurred_at_ns <= ?2)
       AND (?3 IS NULL OR event_type = ?3)
       AND (?4 IS NULL OR result = ?4)
       AND (?5 IS NULL OR policy_id = ?5)
       AND (?6 IS NULL OR agent_id = ?6)
       AND (?7 IS NULL OR session_id = ?7)
       AND (?8 IS NULL OR binding_id = ?8)
     ORDER BY occurred_at_ns DESC, event_id ASC
     LIMIT ?9 OFFSET ?10";

/// Typed local-security persistence failures.
#[derive(Debug, Error)]
pub enum SecurityStoreError {
    /// Opening the configured database through the shared helper failed.
    #[error("failed to open security database: {0}")]
    Open(String),
    /// SQLite schema, write, or query failed.
    #[error("security database failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Event JSON encoding or decoding failed.
    #[error("security event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// A caller supplied an unsupported grouping or query field.
    #[error("invalid security filter: {0}")]
    InvalidFilter(String),
    /// A requested risk case does not exist.
    #[error("risk case {0} does not exist")]
    MissingCase(Uuid),
    /// A persisted timestamp cannot fit SQLite's signed integer representation.
    #[error("timestamp {0} exceeds SQLite integer range")]
    TimestampOutOfRange(u64),
    /// Another thread poisoned the database connection lock.
    #[error("security database connection lock is poisoned")]
    Poisoned,
}

/// Unified interface used by coordinators and API handlers.
pub trait SecurityEventStore {
    /// Inserts one immutable event, returning false for a duplicate ID.
    fn insert_event(&self, event: &SecurityEvent) -> Result<bool, SecurityStoreError>;

    /// Loads one event by its stable ID.
    fn event(&self, event_id: Uuid) -> Result<Option<SecurityEvent>, SecurityStoreError>;

    /// Lists a bounded newest-first event page.
    fn list_events(
        &self,
        filter: &SecurityEventFilter,
    ) -> Result<SecurityEventPage, SecurityStoreError>;
}

/// AgentSight-owned local SQLite security store.
pub struct SecurityStore {
    conn: Mutex<Connection>,
}

impl SecurityStore {
    /// Opens the default `~/.agentsight/security.db` store.
    ///
    /// # Errors
    ///
    /// Returns a typed open or schema error.
    pub fn open_default() -> Result<Self, SecurityStoreError> {
        Self::open(default_base_path().join("security.db"))
    }

    /// Opens a security store at `path` and applies additive schema creation.
    ///
    /// # Errors
    ///
    /// Returns a typed open or schema error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SecurityStoreError> {
        let conn = create_connection(path.as_ref())
            .map_err(|error| SecurityStoreError::Open(error.to_string()))?;
        Self::from_connection(conn)
    }

    /// Opens an isolated in-memory security store for tests and no-op modes.
    ///
    /// # Errors
    ///
    /// Returns a SQLite error when the connection or schema cannot be created.
    pub fn open_in_memory() -> Result<Self, SecurityStoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Returns the default security database path.
    pub fn default_path() -> PathBuf {
        default_base_path().join("security.db")
    }

    /// Inserts one immutable event, returning false for a duplicate ID.
    ///
    /// # Errors
    ///
    /// Returns a typed database, serialization, timestamp, or lock error.
    pub fn insert_event(&self, event: &SecurityEvent) -> Result<bool, SecurityStoreError> {
        <Self as SecurityEventStore>::insert_event(self, event)
    }

    /// Loads one event by its stable ID.
    ///
    /// # Errors
    ///
    /// Returns a typed database, serialization, or lock error.
    pub fn event(&self, event_id: Uuid) -> Result<Option<SecurityEvent>, SecurityStoreError> {
        <Self as SecurityEventStore>::event(self, event_id)
    }

    /// Lists a bounded newest-first event page.
    ///
    /// # Errors
    ///
    /// Returns a typed database, serialization, timestamp, or lock error.
    pub fn list_events(
        &self,
        filter: &SecurityEventFilter,
    ) -> Result<SecurityEventPage, SecurityStoreError> {
        <Self as SecurityEventStore>::list_events(self, filter)
    }

    fn from_connection(conn: Connection) -> Result<Self, SecurityStoreError> {
        conn.busy_timeout(std::time::Duration::from_millis(500))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS security_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                occurred_at_ns INTEGER NOT NULL,
                observed_at_ns INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                agent_name TEXT,
                session_id TEXT,
                pid INTEGER NOT NULL,
                process_start_time INTEGER NOT NULL,
                binding_id TEXT NOT NULL,
                policy_id TEXT,
                policy_revision INTEGER,
                result TEXT,
                destination_class TEXT,
                event_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_security_events_time
                ON security_events(occurred_at_ns DESC);
            CREATE INDEX IF NOT EXISTS idx_security_events_session_time
                ON security_events(session_id, occurred_at_ns DESC);
            CREATE TABLE IF NOT EXISTS risk_cases (
                case_id TEXT PRIMARY KEY,
                correlation_key TEXT NOT NULL UNIQUE,
                policy_id TEXT NOT NULL,
                policy_revision INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                session_id TEXT,
                severity TEXT NOT NULL,
                risk_score INTEGER NOT NULL,
                status TEXT NOT NULL,
                blocked INTEGER NOT NULL,
                opened_at_ns INTEGER NOT NULL,
                updated_at_ns INTEGER NOT NULL,
                summary TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS risk_evidence_links (
                case_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY(case_id, event_id)
            );
            CREATE TABLE IF NOT EXISTS policy_revisions (
                policy_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                policy_json TEXT NOT NULL,
                created_at_ns INTEGER NOT NULL,
                PRIMARY KEY(policy_id, revision)
            );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Groups all events by one approved indexed column.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityStoreError::InvalidFilter`] for unknown fields.
    pub fn count_by(&self, field: &str) -> Result<Vec<SecurityCountBy>, SecurityStoreError> {
        let sql = match field {
            "event_type" => {
                "SELECT COALESCE(event_type, 'unknown'), COUNT(*) FROM security_events GROUP BY event_type ORDER BY COUNT(*) DESC"
            }
            "result" => {
                "SELECT COALESCE(result, 'unknown'), COUNT(*) FROM security_events GROUP BY result ORDER BY COUNT(*) DESC"
            }
            "policy_id" => {
                "SELECT COALESCE(policy_id, 'unknown'), COUNT(*) FROM security_events GROUP BY policy_id ORDER BY COUNT(*) DESC"
            }
            "destination_class" => {
                "SELECT COALESCE(destination_class, 'unknown'), COUNT(*) FROM security_events GROUP BY destination_class ORDER BY COUNT(*) DESC"
            }
            _ => return Err(SecurityStoreError::InvalidFilter(field.into())),
        };
        let conn = self.connection()?;
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok(SecurityCountBy {
                key: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Returns aggregate totals without deserializing event payloads.
    ///
    /// # Errors
    ///
    /// Returns a typed database error when the summary query fails.
    pub fn summary(&self) -> Result<SecuritySummary, SecurityStoreError> {
        self.connection()?
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN result = 'blocked' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN event_type = 'enforcement_state'
                                          AND event_json LIKE '%\"evidence_loss\"%'
                                     THEN 1 ELSE 0 END), 0)
                 FROM security_events",
                [],
                |row| {
                    Ok(SecuritySummary {
                        total_events: row.get(0)?,
                        blocked_events: row.get(1)?,
                        evidence_loss_events: row.get(2)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Deletes immutable events older than `cutoff_ns`.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, or lock error.
    pub fn purge_before(&self, cutoff_ns: u64) -> Result<u64, SecurityStoreError> {
        let deleted = self.connection()?.execute(
            "DELETE FROM security_events WHERE occurred_at_ns < ?1",
            [sqlite_time(cutoff_ns)?],
        )?;
        Ok(deleted as u64)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, SecurityStoreError> {
        self.conn.lock().map_err(|_| SecurityStoreError::Poisoned)
    }
}

impl SecurityEventStore for SecurityStore {
    fn insert_event(&self, event: &SecurityEvent) -> Result<bool, SecurityStoreError> {
        let metadata = EventMetadata::from_event(event);
        let event_json = serde_json::to_string(event)?;
        let changed = self.connection()?.execute(
            "INSERT OR IGNORE INTO security_events (
                event_id, event_type, occurred_at_ns, observed_at_ns, agent_id, agent_name,
                session_id, pid, process_start_time, binding_id, policy_id, policy_revision,
                result, destination_class, event_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                event.event_id.to_string(),
                metadata.event_type,
                sqlite_time(event.occurred_at_ns)?,
                sqlite_time(event.observed_at_ns)?,
                event.identity.agent_id,
                event.identity.agent_name,
                event.identity.session_id,
                event.identity.pid,
                sqlite_time(event.identity.process_start_time)?,
                event.identity.binding_id.to_string(),
                metadata.policy_id,
                metadata.policy_revision.map(sqlite_time).transpose()?,
                metadata.result,
                metadata.destination_class,
                event_json,
            ],
        )?;
        Ok(changed == 1)
    }

    fn event(&self, event_id: Uuid) -> Result<Option<SecurityEvent>, SecurityStoreError> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT event_json FROM security_events WHERE event_id = ?1",
                [event_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(Into::into)
    }

    fn list_events(
        &self,
        filter: &SecurityEventFilter,
    ) -> Result<SecurityEventPage, SecurityStoreError> {
        let limit = filter.limit.clamp(1, 1_000);
        let offset = filter.offset.max(0);
        let binding_id = filter.binding_id.map(|value| value.to_string());
        let conn = self.connection()?;
        let mut statement = conn.prepare(EVENT_QUERY)?;
        let rows = statement.query_map(
            params![
                filter.start_ns.map(sqlite_time).transpose()?,
                filter.end_ns.map(sqlite_time).transpose()?,
                filter.event_type,
                filter.result,
                filter.policy_id,
                filter.agent_id,
                filter.session_id,
                binding_id,
                limit as i64,
                offset,
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut items = Vec::new();
        for row in rows {
            items.push(serde_json::from_str(&row?)?);
        }
        Ok(SecurityEventPage {
            items,
            limit,
            offset,
        })
    }
}

struct EventMetadata<'a> {
    event_type: &'static str,
    policy_id: Option<&'a str>,
    policy_revision: Option<u64>,
    result: &'static str,
    destination_class: Option<&'static str>,
}

impl<'a> EventMetadata<'a> {
    fn from_event(event: &'a SecurityEvent) -> Self {
        match &event.kind {
            SecurityEventKind::FileAction(action) => Self {
                event_type: "file_action",
                policy_id: Some(&action.policy_id),
                policy_revision: Some(action.policy_revision),
                result: if action.succeeded { "allowed" } else { "failed" },
                destination_class: None,
            },
            SecurityEventKind::TaintTransition(transition) => Self {
                event_type: "taint_transition",
                policy_id: Some(&transition.policy_id),
                policy_revision: Some(transition.policy_revision),
                result: "changed",
                destination_class: None,
            },
            SecurityEventKind::NetworkAction(action) => Self {
                event_type: "network_action",
                policy_id: Some(&action.policy_id),
                policy_revision: Some(action.policy_revision),
                result: if action.succeeded { "allowed" } else { "blocked" },
                destination_class: Some(destination_class(action.destination_class)),
            },
            SecurityEventKind::PolicyDecision(decision) => Self {
                event_type: "policy_decision",
                policy_id: Some(&decision.policy_id),
                policy_revision: Some(decision.policy_revision),
                result: if decision.blocked { "blocked" } else { "allowed" },
                destination_class: None,
            },
            SecurityEventKind::EnforcementState(state) => Self {
                event_type: "enforcement_state",
                policy_id: state.policy_id.as_deref(),
                policy_revision: state.policy_revision,
                result: if state.ready { "ready" } else { "degraded" },
                destination_class: None,
            },
        }
    }
}

fn destination_class(class: DestinationClass) -> &'static str {
    match class {
        DestinationClass::Local => "local",
        DestinationClass::Private => "private",
        DestinationClass::Trusted => "trusted",
        DestinationClass::Public => "public",
        DestinationClass::Unknown => "unknown",
    }
}

fn sqlite_time(value: u64) -> Result<i64, SecurityStoreError> {
    i64::try_from(value).map_err(|_| SecurityStoreError::TimestampOutOfRange(value))
}
