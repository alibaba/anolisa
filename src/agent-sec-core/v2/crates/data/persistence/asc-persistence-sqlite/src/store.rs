use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::schema;

/// One process-owned `SQLite` adapter.
pub struct SqlitePolicyStore {
    connection: Mutex<Connection>,
}

impl SqlitePolicyStore {
    /// Opens a database and creates the phase-one schema transactionally.
    ///
    /// # Errors
    /// Returns a safe persistence error when open or migration fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreOpenError> {
        let mut connection = Connection::open(path).map_err(|_| StoreOpenError)?;
        schema::initialize(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Creates an isolated in-memory store for tests.
    ///
    /// # Errors
    /// Returns an error if `SQLite` initialization fails.
    pub fn memory() -> Result<Self, StoreOpenError> {
        Self::open(":memory:")
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, ()> {
        self.connection.lock().map_err(|_| ())
    }
}

/// Database open or schema compatibility failure.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("policy database could not be opened safely")]
pub struct StoreOpenError;
