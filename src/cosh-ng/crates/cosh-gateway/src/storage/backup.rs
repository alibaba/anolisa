//! Verified online backup and new-path restore for Gateway SQLite state.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use cosh_gateway_contracts::ids::InstallationId;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::{schema, sqlite, SqliteTaskStore, StoreError};

const BACKUP_PAGES_PER_STEP: i32 = 128;
const BACKUP_RETRY_LIMIT: u32 = 100;
const BACKUP_RETRY_PAUSE: Duration = Duration::from_millis(5);
const TEMPORARY_CREATE_ATTEMPTS: u32 = 64;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl SqliteTaskStore {
    /// Creates and durably publishes a verified online backup.
    ///
    /// The destination must be an absolute, previously unused path beneath a
    /// private directory. The source writer is exclusively borrowed while the
    /// SQLite online backup captures committed WAL state.
    ///
    /// # Errors
    ///
    /// Returns an error when path hardening, online copy, schema validation,
    /// installation binding, file sync, or atomic publication fails.
    pub fn backup_to_verified(
        &mut self,
        destination: impl AsRef<Path>,
        expected_installation_id: &InstallationId,
    ) -> Result<(), StoreError> {
        let destination = destination.as_ref();
        sqlite::prepare_new_private_file_path(destination)?;
        verify_connection(self.connection(), expected_installation_id)?;

        let temporary = TemporaryDatabase::create(destination)?;
        {
            let mut destination_connection = open_temporary_database(temporary.path())?;
            online_copy(self.connection(), &mut destination_connection)?;
            configure_standalone_database(&destination_connection)?;
        }
        require_self_contained_database(temporary.path())?;
        verify_backup_path(temporary.path(), expected_installation_id)?;
        temporary.publish(destination)
    }

    /// Verifies a private backup without modifying or migrating it.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, malformed or incompatible schemas,
    /// failed integrity checks, or an installation identity mismatch.
    pub fn verify_backup(
        backup_path: impl AsRef<Path>,
        expected_installation_id: &InstallationId,
    ) -> Result<(), StoreError> {
        verify_backup_path(backup_path.as_ref(), expected_installation_id)
    }

    /// Restores a verified backup to a new private database path.
    ///
    /// The destination is never opened or overwritten when it already exists.
    /// Known older schemas are migrated on the temporary copy before atomic
    /// publication; the source backup remains read-only.
    ///
    /// # Errors
    ///
    /// Returns an error when verification, online copy, migration, durability,
    /// publication, or final store open fails.
    pub fn restore_to_new_path(
        backup_path: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        expected_installation_id: &InstallationId,
    ) -> Result<Self, StoreError> {
        let backup_path = backup_path.as_ref();
        let destination = destination.as_ref();
        verify_backup_path(backup_path, expected_installation_id)?;
        sqlite::prepare_new_private_file_path(destination)?;

        let source = open_read_only_database(backup_path)?;
        let temporary = TemporaryDatabase::create(destination)?;
        {
            let mut destination_connection = open_temporary_database(temporary.path())?;
            online_copy(&source, &mut destination_connection)?;
            configure_standalone_database(&destination_connection)?;
            schema::migrate(&mut destination_connection)?;
            bind_restored_installation(&destination_connection, expected_installation_id)?;
            verify_connection(&destination_connection, expected_installation_id)?;
        }
        require_self_contained_database(temporary.path())?;
        temporary.publish(destination)?;

        let store = Self::open(destination)?;
        verify_connection(store.connection(), expected_installation_id)?;
        Ok(store)
    }
}

fn online_copy(source: &Connection, destination: &mut Connection) -> Result<(), StoreError> {
    let backup = Backup::new(source, destination)?;
    let mut transient_failures = 0_u32;
    loop {
        match backup.step(BACKUP_PAGES_PER_STEP)? {
            StepResult::Done => return Ok(()),
            StepResult::More => thread::sleep(BACKUP_RETRY_PAUSE),
            state @ (StepResult::Busy | StepResult::Locked) => {
                transient_failures = transient_failures.saturating_add(1);
                if transient_failures >= BACKUP_RETRY_LIMIT {
                    let error_code = if matches!(state, StepResult::Locked) {
                        rusqlite::ffi::SQLITE_LOCKED
                    } else {
                        rusqlite::ffi::SQLITE_BUSY
                    };
                    return Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(error_code),
                        Some("online backup remained busy".to_owned()),
                    )));
                }
                thread::sleep(BACKUP_RETRY_PAUSE);
            }
            _ => {
                return Err(StoreError::Corrupt {
                    message: "SQLite returned an unsupported online backup state".to_owned(),
                });
            }
        }
    }
}

fn verify_backup_path(
    path: &Path,
    expected_installation_id: &InstallationId,
) -> Result<(), StoreError> {
    sqlite::validate_existing_private_file_path(path)?;
    require_self_contained_database(path)?;
    let connection = open_read_only_database(path)?;
    verify_connection(&connection, expected_installation_id)
}

fn verify_connection(
    connection: &Connection,
    expected_installation_id: &InstallationId,
) -> Result<(), StoreError> {
    schema::preflight_existing(connection)?;
    let stored = read_installation_id(connection)?.ok_or_else(|| StoreError::Corrupt {
        message: "backup has no recoverable installation identity".to_owned(),
    })?;
    if &stored != expected_installation_id {
        return Err(StoreError::LedgerConflict {
            message: "backup belongs to another installation identity".to_owned(),
        });
    }
    Ok(())
}

fn read_installation_id(connection: &Connection) -> Result<Option<InstallationId>, StoreError> {
    let has_identity_table = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_schema
             WHERE type = 'table' AND name = 'gateway_identity'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    let stored = if has_identity_table {
        connection
            .query_row(
                "SELECT installation_id FROM gateway_identity WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(InstallationId::parse)
            .transpose()
            .map_err(|error| StoreError::Corrupt {
                message: format!("stored backup installation identity is invalid: {error}"),
            })?
    } else {
        None
    };
    if stored.is_some() {
        return Ok(stored);
    }
    sqlite::recover_installation_id(connection)
}

fn bind_restored_installation(
    connection: &Connection,
    expected_installation_id: &InstallationId,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO gateway_identity(singleton, installation_id)
         VALUES (1, ?1)
         ON CONFLICT(singleton) DO NOTHING",
        params![expected_installation_id.as_str()],
    )?;
    Ok(())
}

fn open_read_only_database(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    sqlite::configure_read_only(&connection)?;
    Ok(connection)
}

fn open_temporary_database(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    configure_standalone_database(&connection)?;
    Ok(connection)
}

fn configure_standalone_database(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = FULL;
         PRAGMA busy_timeout = 5000;
         PRAGMA trusted_schema = OFF;",
    )?;
    Ok(())
}

struct TemporaryDatabase {
    path: PathBuf,
    file: File,
    cleanup: bool,
}

impl TemporaryDatabase {
    fn create(destination: &Path) -> Result<Self, StoreError> {
        let parent = destination.parent().ok_or_else(|| StoreError::UnsafePath {
            path: destination.to_path_buf(),
            message: "backup destination has no parent directory".to_owned(),
        })?;
        let file_name = destination
            .file_name()
            .ok_or_else(|| StoreError::UnsafePath {
                path: destination.to_path_buf(),
                message: "backup destination has no file name".to_owned(),
            })?;

        for _ in 0..TEMPORARY_CREATE_ATTEMPTS {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut temporary_name = OsString::from(".");
            temporary_name.push(file_name);
            temporary_name.push(format!(".{}.{}.tmp", std::process::id(), sequence));
            let path = parent.join(temporary_name);
            match create_private_file(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file,
                        cleanup: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(StoreError::Io {
                        operation: "create backup temporary file",
                        path,
                        source,
                    });
                }
            }
        }
        Err(StoreError::UnsafePath {
            path: destination.to_path_buf(),
            message: "could not allocate a unique backup temporary file".to_owned(),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn publish(mut self, destination: &Path) -> Result<(), StoreError> {
        self.file.sync_all().map_err(|source| StoreError::Io {
            operation: "sync verified backup file",
            path: self.path().to_path_buf(),
            source,
        })?;
        let temporary_path = self.path().to_path_buf();
        fs::hard_link(&temporary_path, destination).map_err(|source| StoreError::Io {
            operation: "atomically publish verified backup",
            path: destination.to_path_buf(),
            source,
        })?;
        fs::remove_file(&temporary_path).map_err(|source| StoreError::Io {
            operation: "remove published backup temporary name",
            path: temporary_path.clone(),
            source,
        })?;
        self.cleanup = false;
        sync_parent_directory(destination)
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        if self.cleanup {
            remove_sqlite_files(&self.path);
        }
    }
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn sync_parent_directory(path: &Path) -> Result<(), StoreError> {
    let parent = path.parent().ok_or_else(|| StoreError::UnsafePath {
        path: path.to_path_buf(),
        message: "storage path has no parent directory".to_owned(),
    })?;
    let directory = File::open(parent).map_err(|source| StoreError::Io {
        operation: "open storage parent directory for sync",
        path: parent.to_path_buf(),
        source,
    })?;
    directory.sync_all().map_err(|source| StoreError::Io {
        operation: "sync storage parent directory",
        path: parent.to_path_buf(),
        source,
    })
}

fn remove_sqlite_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut companion = path.as_os_str().to_owned();
        companion.push(suffix);
        let _ = fs::remove_file(PathBuf::from(companion));
    }
}

fn require_self_contained_database(path: &Path) -> Result<(), StoreError> {
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut companion = path.as_os_str().to_owned();
        companion.push(suffix);
        let companion = PathBuf::from(companion);
        match fs::symlink_metadata(&companion) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(StoreError::Corrupt {
                    message: format!(
                        "backup is not self-contained: unexpected SQLite {suffix} companion"
                    ),
                });
            }
            Err(source) => {
                return Err(StoreError::Io {
                    operation: "inspect backup companion file",
                    path: companion,
                    source,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cosh_gateway_contracts::common::{
        BoundedName, BoundedOpaque, ContractHeader, ContractSchema, Correlation, Digest, TargetRef,
    };
    use cosh_gateway_contracts::ids::{ActorId, MessageId, TaskId};
    use cosh_gateway_contracts::task::{TaskEvent, TaskEventEnvelope};
    use rusqlite::types::ValueRef;

    use super::*;

    fn source_store(root: &Path) -> (SqliteTaskStore, InstallationId, PathBuf) {
        let source_path = root.join("source/state.db");
        let installation_id = InstallationId::new();
        let mut store = SqliteTaskStore::open(&source_path).unwrap();
        assert_eq!(
            store.bind_installation_id(Some(&installation_id)).unwrap(),
            installation_id
        );
        store
            .connection()
            .execute_batch("PRAGMA wal_autocheckpoint = 0;")
            .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO ledger_receipts(
                     actor_id, idempotency_key, command_digest, operation,
                     result_json, committed_at_ms
                 ) VALUES ('actor', 'backup-marker', ?1, 'test', '{\"ok\":true}', 100)",
                ["a".repeat(64)],
            )
            .unwrap();
        (store, installation_id, source_path)
    }

    fn copy_private(source: &Path, destination: &Path) {
        fs::copy(source, destination).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            fs::set_permissions(destination, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                fs::symlink_metadata(destination).unwrap().mode() & 0o777,
                0o600
            );
        }
    }

    fn logical_snapshot(connection: &Connection) -> BTreeMap<String, Vec<Vec<u8>>> {
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
        let mut snapshot = BTreeMap::new();
        for table in tables {
            let quoted_table = table.replace('"', "\"\"");
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{quoted_table}\""))
                .unwrap();
            let column_count = statement.column_count();
            let mut query = statement.query([]).unwrap();
            let mut encoded_rows = Vec::new();
            while let Some(row) = query.next().unwrap() {
                let mut encoded = Vec::new();
                for index in 0..column_count {
                    match row.get_ref(index).unwrap() {
                        ValueRef::Null => encoded.push(0),
                        ValueRef::Integer(value) => {
                            encoded.push(1);
                            encoded.extend_from_slice(&value.to_be_bytes());
                        }
                        ValueRef::Real(value) => {
                            encoded.push(2);
                            encoded.extend_from_slice(&value.to_bits().to_be_bytes());
                        }
                        ValueRef::Text(value) => {
                            encoded.push(3);
                            encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
                            encoded.extend_from_slice(value);
                        }
                        ValueRef::Blob(value) => {
                            encoded.push(4);
                            encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
                            encoded.extend_from_slice(value);
                        }
                    }
                }
                encoded_rows.push(encoded);
            }
            encoded_rows.sort();
            snapshot.insert(table, encoded_rows);
        }
        snapshot
    }

    fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
        let mut value = path.as_os_str().to_owned();
        value.push(suffix);
        PathBuf::from(value)
    }

    #[test]
    fn online_backup_captures_committed_wal_and_verifies() {
        let root = tempfile::tempdir().unwrap();
        let (mut store, installation_id, source_path) = source_store(root.path());
        let wal_path = append_suffix(&source_path, "-wal");
        assert!(fs::metadata(wal_path).unwrap().len() > 0);
        let backup_path = root.path().join("backups/verified.db");

        store
            .backup_to_verified(&backup_path, &installation_id)
            .unwrap();

        SqliteTaskStore::verify_backup(&backup_path, &installation_id).unwrap();
        let backup = open_read_only_database(&backup_path).unwrap();
        let markers = backup
            .query_row(
                "SELECT COUNT(*) FROM ledger_receipts
                 WHERE idempotency_key = 'backup-marker'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap();
        assert_eq!(markers, 1);
        assert!(!append_suffix(&backup_path, "-wal").exists());
        assert!(!append_suffix(&backup_path, "-shm").exists());
        assert!(!append_suffix(&backup_path, "-journal").exists());
    }

    #[test]
    fn restore_to_new_path_preserves_all_logical_rows() {
        let root = tempfile::tempdir().unwrap();
        let (mut source, installation_id, _) = source_store(root.path());
        let backup_path = root.path().join("backups/verified.db");
        source
            .backup_to_verified(&backup_path, &installation_id)
            .unwrap();
        let before = logical_snapshot(source.connection());
        let restored_path = root.path().join("restored/state.db");

        let restored =
            SqliteTaskStore::restore_to_new_path(&backup_path, &restored_path, &installation_id)
                .unwrap();

        let after = logical_snapshot(restored.connection());
        assert!(before == after, "restored database differs logically");
        assert_eq!(restored.path(), Some(restored_path.as_path()));
    }

    #[test]
    fn verification_rejects_corrupt_or_incompatible_backups() {
        let root = tempfile::tempdir().unwrap();
        let (mut store, installation_id, _) = source_store(root.path());
        let backup_path = root.path().join("backups/verified.db");
        store
            .backup_to_verified(&backup_path, &installation_id)
            .unwrap();

        let truncated = root.path().join("backups/truncated.db");
        copy_private(&backup_path, &truncated);
        OpenOptions::new()
            .write(true)
            .open(&truncated)
            .unwrap()
            .set_len(64)
            .unwrap();
        assert!(SqliteTaskStore::verify_backup(&truncated, &installation_id).is_err());

        let orphaned = root.path().join("backups/orphaned.db");
        copy_private(&backup_path, &orphaned);
        let connection = Connection::open(&orphaned).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO outbox(
                     delivery_id, task_id, event_id, delivery_kind, payload_json, state,
                     attempt, next_attempt_at_ms, lease_owner, lease_expires_at_ms,
                     created_at_ms, delivered_at_ms
                 ) VALUES (
                     'orphan-delivery', 'missing-task', 'missing-event',
                     'runtime_start', '{}', 'pending', 0, 0, NULL, NULL, 0, NULL
                 );",
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteTaskStore::verify_backup(&orphaned, &installation_id),
            Err(StoreError::Corrupt { message }) if message.contains("foreign_key_check")
        ));

        let newer = root.path().join("backups/newer.db");
        copy_private(&backup_path, &newer);
        let connection = Connection::open(&newer).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version, checksum, applied_at_ms)
                 VALUES (?1, 'future', 0)",
                [schema::CURRENT_SCHEMA_VERSION + 1],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteTaskStore::verify_backup(&newer, &installation_id),
            Err(StoreError::NewerSchema { .. })
        ));

        let checksum = root.path().join("backups/checksum.db");
        copy_private(&backup_path, &checksum);
        let connection = Connection::open(&checksum).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = 'changed' WHERE version = 1",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteTaskStore::verify_backup(&checksum, &installation_id),
            Err(StoreError::MigrationChecksum { version: 1 })
        ));

        assert!(matches!(
            SqliteTaskStore::verify_backup(&backup_path, &InstallationId::new()),
            Err(StoreError::LedgerConflict { .. })
        ));
    }

    #[test]
    fn backup_and_restore_never_replace_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let (mut store, installation_id, _) = source_store(root.path());
        let backup_path = root.path().join("backups/verified.db");
        store
            .backup_to_verified(&backup_path, &installation_id)
            .unwrap();
        let occupied = root.path().join("occupied.db");
        let sentinel = b"occupied destination";
        fs::write(&occupied, sentinel).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&occupied, fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert!(matches!(
            store.backup_to_verified(&occupied, &installation_id),
            Err(StoreError::UnsafePath { .. })
        ));
        assert!(matches!(
            SqliteTaskStore::restore_to_new_path(&backup_path, &occupied, &installation_id),
            Err(StoreError::UnsafePath { .. })
        ));
        assert_eq!(fs::read(&occupied).unwrap(), sentinel);
    }

    #[test]
    fn restore_migrates_a_verified_v1_backup_before_publication() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let backup_path = root.path().join("gateway-v1.db");
        let installation_id = InstallationId::new();
        let actor_id = ActorId::new();
        let task_id = TaskId::new();
        let mut correlation = Correlation::new(installation_id.clone());
        correlation.actor_id = Some(actor_id.clone());
        correlation.task_id = Some(task_id.clone());
        let event = TaskEventEnvelope {
            header: ContractHeader::new(
                ContractSchema::TaskEvent,
                MessageId::new(),
                1,
                correlation,
            ),
            task_id: task_id.clone(),
            revision: 1,
            event: TaskEvent::TaskSubmitted {
                intent_digest: Digest::parse("a".repeat(64)).unwrap(),
                target: TargetRef {
                    kind: BoundedName::new("local").unwrap(),
                    authority: BoundedName::new("test").unwrap(),
                    identifier: BoundedOpaque::new("target").unwrap(),
                },
            },
        };
        {
            let mut connection = Connection::open(&backup_path).unwrap();
            connection
                .execute_batch("PRAGMA foreign_keys = ON;")
                .unwrap();
            schema::migrate_to_for_test(&mut connection, 1).unwrap();
            connection
                .execute(
                    "INSERT INTO tasks(
                         task_id, owner_actor_id, target_ref, revision, state,
                         snapshot_json, created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, '{}', 1, 'submitted', '{}', 100, 100)",
                    params![task_id.as_str(), actor_id.as_str()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO task_events(
                         event_id, task_id, revision, event_type, schema_version,
                         payload_json, occurred_at_ms, causation_id, correlation_id
                     ) VALUES (?1, ?2, 1, 'task_submitted', 1, ?3, 100, NULL, NULL)",
                    params![
                        event.header.message_id.as_str(),
                        task_id.as_str(),
                        serde_json::to_string(&event).unwrap()
                    ],
                )
                .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        SqliteTaskStore::verify_backup(&backup_path, &installation_id).unwrap();
        let restored_path = root.path().join("restored/state.db");
        let restored =
            SqliteTaskStore::restore_to_new_path(&backup_path, &restored_path, &installation_id)
                .unwrap();

        let version = restored
            .connection()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap();
        let stored_identity = restored
            .connection()
            .query_row(
                "SELECT installation_id FROM gateway_identity WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(version, schema::CURRENT_SCHEMA_VERSION);
        assert_eq!(stored_identity, installation_id.as_str());
    }
}
