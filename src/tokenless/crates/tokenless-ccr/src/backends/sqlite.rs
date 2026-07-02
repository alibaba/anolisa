//! SQLite-backed stash store (default production backend).
//!
//! Persists stashed payloads to a single file so state survives across the
//! short-lived processes that tokenless hooks fork+exec on every call. Uses
//! WAL mode and a single-writer `Mutex<Connection>` to serialize writes,
//! mirroring the approach in `tokenless-stats`.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::key::compute_key;
use crate::store::{StashError, StashStore};

/// Default time-to-live for an entry: 1 hour. A retrieve window of an hour
/// comfortably covers a typical agent session's compress→retrieve round trip.
const DEFAULT_TTL_SECONDS: u64 = 60 * 60;

/// Default maximum number of live entries before FIFO eviction.
const DEFAULT_CAPACITY: usize = 10_000;

pub struct SqliteStore {
    conn: Mutex<Connection>,
    ttl_seconds: u64,
    capacity: usize,
}

impl SqliteStore {
    /// Open (or create) a stash database at `path` with default TTL and
    /// capacity. The file and its `-wal`/`-shm` sidecars are created on first
    /// write; the parent directory must already exist.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StashError> {
        Self::with_limits(path, DEFAULT_TTL_SECONDS, DEFAULT_CAPACITY)
    }

    /// Open (or create) a stash database with a custom TTL and capacity.
    pub fn with_limits<P: AsRef<Path>>(
        path: P,
        ttl_seconds: u64,
        capacity: usize,
    ) -> Result<Self, StashError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             PRAGMA synchronous=NORMAL;",
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS stash (
                hash TEXT PRIMARY KEY,
                payload TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_stash_expires_at ON stash(expires_at)",
            [],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            ttl_seconds,
            capacity,
        })
    }

    /// Acquire the connection guard, recovering from poison rather than
    /// failing. A poisoned mutex means a prior holder panicked; for our
    /// single-statement workload the SQLite connection itself stays usable,
    /// so we clear the poison and reuse the underlying guard. This mirrors
    /// the fail-soft policy in `tokenless-stats::recorder::StatsRecorder`.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| {
            eprintln!(
                "[tokenless-ccr] WARNING: sqlite mutex poisoned by a previous panic; recovering: {}",
                poisoned
            );
            self.conn.clear_poison();
            poisoned.into_inner()
        })
    }

    /// FIFO-evict oldest entries once the live count exceeds `capacity`.
    /// Returns the number evicted. "Oldest" is approximated by the lowest
    /// `expires_at` (earliest-inserted, since all entries share the same TTL).
    fn enforce_capacity(&self, conn: &Connection) -> Result<usize, StashError> {
        let now = now_unix();
        let live: i64 = conn.query_row(
            "SELECT COUNT(*) FROM stash WHERE expires_at >= ?",
            [now as i64],
            |row| row.get(0),
        )?;
        let surplus = live.saturating_sub(self.capacity as i64);
        if surplus <= 0 {
            return Ok(0);
        }
        let evicted = conn.execute(
            "DELETE FROM stash
             WHERE hash IN (
                 SELECT hash FROM stash
                 WHERE expires_at >= ?
                 ORDER BY expires_at ASC
                 LIMIT ?
             )",
            rusqlite::params![now as i64, surplus],
        )?;
        Ok(evicted)
    }
}

impl StashStore for SqliteStore {
    fn stash(&self, payload: &str) -> Result<String, StashError> {
        let key = compute_key(payload.as_bytes());
        let now = now_unix();
        let expires_at = now + self.ttl_seconds;
        let conn = self.lock_conn();
        conn.execute(
            "INSERT OR REPLACE INTO stash (hash, payload, expires_at) VALUES (?, ?, ?)",
            rusqlite::params![key, payload, expires_at as i64],
        )?;
        self.enforce_capacity(&conn)?;
        Ok(key)
    }

    fn retrieve(&self, hash: &str) -> Result<Option<String>, StashError> {
        let now = now_unix();
        // Keys are stored as lowercase BLAKE3 hex; accept a marker the LLM may
        // have uppercased by lowercasing the lookup (case-insensitive retrieve).
        let key = hash.to_ascii_lowercase();
        let conn = self.lock_conn();
        match conn.query_row(
            "SELECT payload FROM stash WHERE hash = ? AND expires_at >= ?",
            rusqlite::params![key, now as i64],
            |row| row.get::<_, String>(0),
        ) {
            Ok(payload) => Ok(Some(payload)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StashError::from(e)),
        }
    }

    fn len(&self) -> usize {
        let now = now_unix();
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT COUNT(*) FROM stash WHERE expires_at >= ?",
            [now as i64],
            |row| row.get(0),
        )
        .unwrap_or(0) as usize
    }

    fn evict_expired(&self) -> Result<usize, StashError> {
        let now = now_unix();
        let conn = self.lock_conn();
        let evicted = conn.execute("DELETE FROM stash WHERE expires_at < ?", [now as i64])?;
        Ok(evicted)
    }
}

/// Current wall-clock seconds since the Unix epoch. Used for expiry math so
/// the stash does not depend on `chrono`.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn tmp_store(ttl: u64, cap: usize) -> (SqliteStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::with_limits(dir.path().join("stash.db"), ttl, cap).unwrap();
        (store, dir)
    }

    #[test]
    fn retrieve_is_case_insensitive() {
        let (store, _dir) = tmp_store(60, 100);
        let key = store.stash("payload").unwrap();
        assert_eq!(key, key.to_ascii_lowercase());
        let upper = key.to_uppercase();
        assert_ne!(upper, key);
        assert_eq!(store.retrieve(&upper).unwrap(), Some("payload".to_string()));
    }

    #[test]
    fn round_trip_persists_across_connections() {
        let (store, dir) = tmp_store(60, 100);
        let key = store.stash("payload-A").unwrap();
        assert_eq!(store.retrieve(&key).unwrap(), Some("payload-A".to_string()));

        // A second connection to the same file sees the entry: proves the
        // store survives across processes (the hook fork+exec case).
        let store2 = SqliteStore::new(dir.path().join("stash.db")).unwrap();
        assert_eq!(
            store2.retrieve(&key).unwrap(),
            Some("payload-A".to_string())
        );
    }

    #[test]
    fn retrieve_missing_returns_none() {
        let (store, _dir) = tmp_store(60, 100);
        assert_eq!(store.retrieve("000000000000000000000000").unwrap(), None);
    }

    #[test]
    fn expired_entry_not_retrievable() {
        let (store, _dir) = tmp_store(1, 100);
        let key = store.stash("ephemeral").unwrap();
        thread::sleep(std::time::Duration::from_secs(2));
        assert_eq!(store.retrieve(&key).unwrap(), None);
    }

    #[test]
    fn evict_expired_reports_count() {
        let (store, _dir) = tmp_store(1, 100);
        store.stash("a").unwrap();
        store.stash("b").unwrap();
        thread::sleep(std::time::Duration::from_secs(2));
        assert_eq!(store.evict_expired().unwrap(), 2);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn fifo_eviction_when_over_capacity() {
        let (store, _dir) = tmp_store(60, 3);
        let k0 = store.stash("0").unwrap();
        store.stash("1").unwrap();
        store.stash("2").unwrap();
        store.stash("3").unwrap(); // surplus=1, evicts oldest live (k0)
        assert_eq!(store.retrieve(&k0).unwrap(), None);
        assert!(store.len() <= 3);
    }

    #[test]
    fn concurrent_writes_no_deadlock() {
        let (store, _dir) = tmp_store(60, 10_000);
        let store = std::sync::Arc::new(store);
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = store.clone();
                thread::spawn(move || {
                    for j in 0..50 {
                        s.stash(&format!("p-{i}-{j}")).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(store.len() <= 10_000);
    }
}
