//! Workspace-scoped session store, legacy upgrade, and summary pagination.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{open, openat, unlinkat, AtFlags, Dir, Mode, OFlags};
use sha2::{Digest, Sha256};

use crate::config::DEFAULT_SESSION_PERSIST_DIR;
use crate::provider::Message;

use super::io::{
    expand_persist_dir, io_error, now_ms, open_file_time_ms, read_bounded_open_session_file,
    try_exclusive_lock, unlock_file, MAX_SESSION_FILE_BYTES,
};
use super::listing::{
    collect_list_page, entry_is_after_cursor, format_list_cursor, parse_list_cursor,
};
use super::scoped::ScopedStorage;
use super::summary::{bounded_summary_text, summary_from_session, MAX_SUMMARY_WORKSPACE_BYTES};
use super::{
    PersistedSession, ProviderSessionId, SessionError, SessionHealth, SessionSummary,
    CURRENT_SCHEMA_VERSION,
};

const MAX_LIST_LIMIT: usize = 100;

/// Workspace-scoped session persistence with atomic commits.
pub struct SessionStore {
    base_dir: PathBuf,
    scoped: ScopedStorage,
    legacy_dirs: Vec<LegacyDirectory>,
    workspace_scope: String,
}

struct LegacyDirectory {
    path: PathBuf,
    file: File,
}

struct LegacySessionFile<'a> {
    directory: &'a LegacyDirectory,
    filename: String,
    path: PathBuf,
    file: File,
}

impl SessionStore {
    /// Resolves a canonical workspace and its deterministic storage directory.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Io`] when the workspace cannot be canonicalized,
    /// or [`SessionError::InvalidRequest`] when its canonical path is not UTF-8.
    pub fn for_workspace(persist_dir: &str, workspace: &Path) -> Result<Self, SessionError> {
        let canonical = fs::canonicalize(workspace)
            .map_err(|error| io_error("resolve workspace", workspace, error))?;
        let workspace_scope = canonical
            .to_str()
            .ok_or_else(|| SessionError::InvalidRequest {
                message: "canonical workspace path is not valid UTF-8".to_string(),
            })?
            .to_string();
        let root = expand_persist_dir(persist_dir, &canonical);
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|error| io_error("resolve session root", &root, error))?
                .join(root)
        };
        // Resolve symlinks in the storage root once, up front, so descriptor
        // pinning can refuse symlinks strictly below the root without
        // rejecting legitimately symlinked home or dotfile layouts.
        let root = canonicalize_storage_root(&root);
        let scope_hash = hex::encode(Sha256::digest(workspace_scope.as_bytes()));
        let legacy_candidate = if persist_dir == DEFAULT_SESSION_PERSIST_DIR {
            Some(canonical.join("sessions"))
        } else if persist_dir_is_workspace_relative(persist_dir) {
            Some(root.clone())
        } else {
            None
        };
        let legacy_dirs = match legacy_candidate {
            Some(candidate) => {
                let workspace_directory = open_directory_path_no_follow(&canonical)
                    .map_err(|error| io_error("open canonical workspace", &canonical, error))?;
                workspace_owned_legacy_dir(&canonical, &workspace_directory, &candidate)
                    .into_iter()
                    .collect()
            }
            None => Vec::new(),
        };
        let base_dir = root.join(&scope_hash[..24]);
        let scoped = ScopedStorage::new(base_dir.clone())?;
        Ok(Self {
            base_dir,
            scoped,
            legacy_dirs,
            workspace_scope,
        })
    }

    /// Returns the canonical workspace owned by this store.
    pub fn workspace_scope(&self) -> &str {
        &self.workspace_scope
    }

    /// Returns the validated session's envelope path.
    pub fn session_file(&self, session_id: &ProviderSessionId) -> PathBuf {
        self.scoped.session_path(session_id)
    }

    /// Loads and validates a persisted or legacy session.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing, corrupt, incompatible, or mismatched data.
    pub fn load(&self, session_id: &ProviderSessionId) -> Result<PersistedSession, SessionError> {
        let (bytes, modified_at_ms) = self.read_session_bytes(session_id)?;
        let content = decode_session_content(session_id, &bytes)?;
        self.parse_content(session_id, content, modified_at_ms)
    }

    /// Atomically commits a session if its generation is current.
    ///
    /// # Errors
    ///
    /// Returns a typed validation, conflict, serialization, or I/O error.
    pub fn persist(&self, session: &mut PersistedSession) -> Result<(), SessionError> {
        if session.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SessionError::IncompatibleVersion {
                session_id: session.session_id.to_string(),
                version: session.schema_version,
            });
        }
        if session.workspace_scope != self.workspace_scope {
            return Err(SessionError::ScopeMismatch {
                session_id: session.session_id.to_string(),
                expected: self.workspace_scope.clone(),
                actual: session.workspace_scope.clone(),
            });
        }

        let Some(directory) = self.scoped.directory(true)? else {
            return Err(io_error(
                "create scoped session directory",
                &self.base_dir,
                io::Error::other("directory creation returned no descriptor"),
            ));
        };
        let _lock = self.scoped.acquire_lock(&directory, &session.session_id)?;
        let scoped_file = self.scoped.open_session(&directory, &session.session_id)?;
        let scoped_exists = scoped_file.is_some();
        let mut legacy = if scoped_exists {
            self.lock_legacy_session_for_clear(&session.session_id)?
        } else {
            self.lock_legacy_session(&session.session_id)?
        };
        let current_generation = match scoped_file {
            Some(file) => {
                let modified_at_ms = open_file_time_ms(&file);
                let bytes = read_bounded_open_session_file(
                    file,
                    &self.session_file(&session.session_id),
                    &session.session_id,
                )?;
                let content = decode_session_content(&session.session_id, &bytes)?;
                let current = self.parse_content(&session.session_id, content, modified_at_ms)?;
                current.generation
            }
            None => 0,
        };
        if current_generation != session.generation {
            return Err(SessionError::Conflict {
                session_id: session.session_id.to_string(),
            });
        }
        let next_generation =
            session
                .generation
                .checked_add(1)
                .ok_or_else(|| SessionError::Corrupt {
                    session_id: session.session_id.to_string(),
                    message: "session generation is exhausted".to_string(),
                })?;
        if scoped_exists {
            if let Some(legacy) = legacy.take() {
                remove_locked_legacy_session_file(legacy, &session.session_id)?;
            }
        }

        let mut next = session.clone();
        next.generation = next_generation;
        next.updated_at_ms = now_ms();
        // Redact only the on-disk copy; the caller's in-memory turn context
        // keeps the original text for the current provider conversation.
        let mut envelope = next.clone();
        crate::redaction::redact_messages(&mut envelope.messages);
        let bytes =
            serde_json::to_vec_pretty(&envelope).map_err(|error| SessionError::Corrupt {
                session_id: session.session_id.to_string(),
                message: format!("serialization failed: {error}"),
            })?;
        if bytes.len() as u64 > MAX_SESSION_FILE_BYTES {
            return Err(SessionError::Corrupt {
                session_id: session.session_id.to_string(),
                message: format!(
                    "serialized session exceeds the {} byte safety limit",
                    MAX_SESSION_FILE_BYTES
                ),
            });
        }

        self.scoped
            .write_atomic(&directory, &session.session_id, &bytes)?;
        *session = next;
        if let Some(legacy) = legacy {
            remove_locked_legacy_session_file(legacy, &session.session_id)?;
        }
        Ok(())
    }

    /// Reads summary metadata without requiring the session to be healthy.
    ///
    /// # Errors
    ///
    /// Returns an I/O or not-found error when the envelope cannot be read.
    pub fn inspect(&self, session_id: &ProviderSessionId) -> Result<SessionSummary, SessionError> {
        let (bytes, modified_at_ms) = self.read_session_bytes(session_id)?;
        Ok(self.summary_from_bytes(session_id, &bytes, modified_at_ms))
    }

    /// Fully validates a session and returns its resumable summary.
    ///
    /// # Errors
    ///
    /// Returns the same typed validation failures as [`Self::load`].
    pub fn validate(&self, session_id: &ProviderSessionId) -> Result<SessionSummary, SessionError> {
        let session = self.load(session_id)?;
        Ok(summary_from_session(&session, SessionHealth::Ready))
    }

    /// Lists a bounded page of newest-first session summaries.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Io`] when the scoped directory cannot be read.
    pub fn list(
        &self,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<SessionSummary>, Option<String>), SessionError> {
        let cursor = cursor.map(parse_list_cursor).transpose()?;
        let directory = self.scoped.directory(false)?;
        let mut entries = match directory.as_ref() {
            Some(directory) => self.scoped.entries(directory)?,
            None => Vec::new(),
        };
        let mut seen_ids = entries
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect::<HashSet<_>>();
        for legacy in &self.legacy_dirs {
            collect_legacy_list_entries(legacy, &mut seen_ids, &mut entries)?;
        }
        entries.sort_by(|left, right| {
            right
                .modified_at_ms
                .cmp(&left.modified_at_ms)
                .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
        });

        let start = match cursor.as_ref() {
            Some(cursor) => entries
                .iter()
                .position(|entry| entry_is_after_cursor(entry, cursor))
                .unwrap_or(entries.len()),
            None => 0,
        };
        let limit = limit.clamp(1, MAX_LIST_LIMIT);
        let (page, examined_end) = collect_list_page(&entries, start, limit, |entry| {
            self.list_entry_summary(directory.as_ref(), entry)
        });
        let next_cursor = (examined_end < entries.len())
            .then(|| {
                entries
                    .get(examined_end.saturating_sub(1))
                    .map(format_list_cursor)
            })
            .flatten();
        Ok((page, next_cursor))
    }

    /// Summarizes one listed entry from scoped storage or its legacy fallback.
    fn list_entry_summary(
        &self,
        directory: Option<&File>,
        entry: &super::listing::ListEntry,
    ) -> Option<SessionSummary> {
        if let Some(directory) = directory {
            match self.read_scoped_session_bytes_from(directory, &entry.session_id) {
                Ok((content, _)) => {
                    return Some(self.summary_from_bytes(
                        &entry.session_id,
                        &content,
                        entry.modified_at_ms,
                    ));
                }
                Err(SessionError::NotFound { .. }) => {}
                Err(SessionError::Corrupt { .. }) => {
                    return Some(self.corrupt_summary(&entry.session_id, entry.modified_at_ms));
                }
                Err(_) => return None,
            }
        }
        match self.read_legacy_session_bytes(&entry.session_id) {
            Ok(Some((content, _))) => {
                Some(self.summary_from_bytes(&entry.session_id, &content, entry.modified_at_ms))
            }
            Ok(None) => None,
            Err(SessionError::Corrupt { .. }) => {
                Some(self.corrupt_summary(&entry.session_id, entry.modified_at_ms))
            }
            Err(_) => None,
        }
    }

    /// Lists canonical stored IDs without loading message history or summaries.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Io`] when the scoped directory cannot be read.
    pub fn session_ids(&self) -> Result<Vec<ProviderSessionId>, SessionError> {
        let mut unique_ids = HashSet::new();
        if let Some(directory) = self.scoped.directory(false)? {
            unique_ids.extend(
                self.scoped
                    .entries(&directory)?
                    .into_iter()
                    .map(|entry| entry.session_id),
            );
        }
        for directory in &self.legacy_dirs {
            collect_session_ids_from_legacy_directory(directory, &mut unique_ids)?;
        }
        let mut session_ids = unique_ids.into_iter().collect::<Vec<_>>();
        session_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(session_ids)
    }

    /// Removes a session unless its ID appears in the protected set.
    ///
    /// # Errors
    ///
    /// Returns a typed protected, conflict, missing, or I/O error.
    pub fn clear(
        &self,
        session_id: &ProviderSessionId,
        protected: &[ProviderSessionId],
    ) -> Result<(), SessionError> {
        if protected.iter().any(|value| value == session_id) {
            return Err(SessionError::ActiveSession {
                session_id: session_id.to_string(),
            });
        }
        let directory = self.scoped.directory(false)?;
        let scoped_file = directory
            .as_ref()
            .map(|directory| self.scoped.open_session(directory, session_id))
            .transpose()?
            .flatten();
        let scoped_exists = scoped_file.is_some();
        let legacy = self.open_legacy_session(session_id)?;
        if !scoped_exists && legacy.is_none() {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            });
        }

        let _scoped_lock = match (&directory, scoped_exists) {
            (Some(directory), true) => Some(self.scoped.acquire_lock(directory, session_id)?),
            _ => None,
        };
        let legacy = legacy
            .map(|legacy| lock_legacy_session_file(legacy, session_id))
            .transpose()?;
        if let Some(legacy) = legacy {
            remove_locked_legacy_session_file(legacy, session_id)?;
        }
        if scoped_exists {
            let Some(directory) = directory.as_ref() else {
                return Err(io_error(
                    "remove scoped session",
                    &self.base_dir,
                    io::Error::other("session file has no pinned parent directory"),
                ));
            };
            self.scoped.remove_session(directory, session_id)?;
        }
        Ok(())
    }

    fn parse_content(
        &self,
        session_id: &ProviderSessionId,
        content: &str,
        modified_at_ms: u64,
    ) -> Result<PersistedSession, SessionError> {
        let value: serde_json::Value =
            serde_json::from_str(content).map_err(|error| SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: error.to_string(),
            })?;
        if value.is_array() {
            let mut messages: Vec<Message> =
                serde_json::from_value(value).map_err(|error| SessionError::Corrupt {
                    session_id: session_id.to_string(),
                    message: error.to_string(),
                })?;
            // Redact before replay so secrets in pre-redaction files never
            // reach the provider context or picker previews.
            crate::redaction::redact_messages(&mut messages);
            return Ok(PersistedSession {
                schema_version: CURRENT_SCHEMA_VERSION,
                session_id: session_id.clone(),
                workspace_scope: self.workspace_scope.clone(),
                created_at_ms: modified_at_ms,
                updated_at_ms: modified_at_ms,
                model: String::new(),
                generation: 0,
                messages,
            });
        }

        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: "missing schema_version".to_string(),
            })? as u32;
        if version != CURRENT_SCHEMA_VERSION {
            return Err(SessionError::IncompatibleVersion {
                session_id: session_id.to_string(),
                version,
            });
        }
        let session: PersistedSession =
            serde_json::from_value(value).map_err(|error| SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: error.to_string(),
            })?;
        if &session.session_id != session_id {
            return Err(SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: "filename and envelope session IDs differ".to_string(),
            });
        }
        if session.workspace_scope != self.workspace_scope {
            return Err(SessionError::ScopeMismatch {
                session_id: session_id.to_string(),
                expected: self.workspace_scope.clone(),
                actual: session.workspace_scope,
            });
        }
        let mut session = session;
        // Envelopes are redacted at persist time; redacting again on load
        // keeps externally written or pre-redaction files equally safe.
        crate::redaction::redact_messages(&mut session.messages);
        Ok(session)
    }

    fn read_session_bytes(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<(Vec<u8>, u64), SessionError> {
        match self.read_scoped_session_bytes(session_id) {
            Ok(value) => return Ok(value),
            Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }
        if let Some(legacy) = self.read_legacy_session_bytes(session_id)? {
            return Ok(legacy);
        }
        Err(SessionError::NotFound {
            session_id: session_id.to_string(),
        })
    }

    fn read_scoped_session_bytes(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<(Vec<u8>, u64), SessionError> {
        let Some(directory) = self.scoped.directory(false)? else {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            });
        };
        self.read_scoped_session_bytes_from(&directory, session_id)
    }

    fn read_scoped_session_bytes_from(
        &self,
        directory: &File,
        session_id: &ProviderSessionId,
    ) -> Result<(Vec<u8>, u64), SessionError> {
        let Some(file) = self.scoped.open_session(directory, session_id)? else {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            });
        };
        let modified_at_ms = open_file_time_ms(&file);
        let bytes =
            read_bounded_open_session_file(file, &self.session_file(session_id), session_id)?;
        Ok((bytes, modified_at_ms))
    }

    fn read_legacy_session_bytes(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<Option<(Vec<u8>, u64)>, SessionError> {
        let Some(legacy) = self.open_legacy_session(session_id)? else {
            return Ok(None);
        };
        let modified_at_ms = open_file_time_ms(&legacy.file);
        let legacy_bytes = read_bounded_open_session_file(legacy.file, &legacy.path, session_id)?;
        let legacy_content = decode_session_content(session_id, &legacy_bytes)?;
        let value: serde_json::Value =
            serde_json::from_str(legacy_content).map_err(|error| SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: error.to_string(),
            })?;
        if !value.is_array() {
            return Err(SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: "legacy session is not a message array".to_string(),
            });
        }
        serde_json::from_value::<Vec<Message>>(value).map_err(|error| SessionError::Corrupt {
            session_id: session_id.to_string(),
            message: error.to_string(),
        })?;
        Ok(Some((legacy_bytes, modified_at_ms)))
    }

    fn lock_legacy_session(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<Option<LegacySessionFile<'_>>, SessionError> {
        let Some(legacy) = self.open_legacy_session(session_id)? else {
            return Ok(None);
        };
        let legacy = lock_legacy_session_file(legacy, session_id)?;
        let bytes = read_bounded_open_session_file(
            legacy
                .file
                .try_clone()
                .map_err(|error| io_error("clone legacy session", &legacy.path, error))?,
            &legacy.path,
            session_id,
        )?;
        let content = decode_session_content(session_id, &bytes)?;
        let value: serde_json::Value =
            serde_json::from_str(content).map_err(|error| SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: error.to_string(),
            })?;
        if !value.is_array() || serde_json::from_value::<Vec<Message>>(value).is_err() {
            return Err(SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: "legacy session is not a valid message array".to_string(),
            });
        }
        Ok(Some(legacy))
    }

    fn lock_legacy_session_for_clear(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<Option<LegacySessionFile<'_>>, SessionError> {
        self.open_legacy_session(session_id)?
            .map(|legacy| lock_legacy_session_file(legacy, session_id))
            .transpose()
    }

    fn open_legacy_session(
        &self,
        session_id: &ProviderSessionId,
    ) -> Result<Option<LegacySessionFile<'_>>, SessionError> {
        for directory in &self.legacy_dirs {
            match open_legacy_session_file(directory, session_id) {
                Ok(Some(legacy)) => return Ok(Some(legacy)),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn summary_from_bytes(
        &self,
        session_id: &ProviderSessionId,
        content: &[u8],
        modified_at_ms: u64,
    ) -> SessionSummary {
        match std::str::from_utf8(content) {
            Ok(content) => self.summary_from_content(session_id, content, modified_at_ms),
            Err(_) => self.corrupt_summary(session_id, modified_at_ms),
        }
    }

    fn summary_from_content(
        &self,
        session_id: &ProviderSessionId,
        content: &str,
        modified_at_ms: u64,
    ) -> SessionSummary {
        match self.parse_content(session_id, content, modified_at_ms) {
            Ok(session) => summary_from_session(&session, SessionHealth::Ready),
            Err(SessionError::IncompatibleVersion { version, .. }) => SessionSummary {
                session_id: session_id.clone(),
                workspace_scope: bounded_summary_text(
                    &self.workspace_scope,
                    MAX_SUMMARY_WORKSPACE_BYTES,
                ),
                created_at_ms: modified_at_ms,
                updated_at_ms: modified_at_ms,
                model: None,
                message_count: 0,
                first_prompt: None,
                schema_version: Some(version),
                health: SessionHealth::Incompatible,
            },
            Err(SessionError::ScopeMismatch { actual, .. }) => SessionSummary {
                session_id: session_id.clone(),
                workspace_scope: bounded_summary_text(&actual, MAX_SUMMARY_WORKSPACE_BYTES),
                created_at_ms: modified_at_ms,
                updated_at_ms: modified_at_ms,
                model: None,
                message_count: 0,
                first_prompt: None,
                schema_version: Some(CURRENT_SCHEMA_VERSION),
                health: SessionHealth::ScopeMismatch,
            },
            Err(_) => self.corrupt_summary(session_id, modified_at_ms),
        }
    }

    fn corrupt_summary(
        &self,
        session_id: &ProviderSessionId,
        modified_at_ms: u64,
    ) -> SessionSummary {
        SessionSummary {
            session_id: session_id.clone(),
            workspace_scope: bounded_summary_text(
                &self.workspace_scope,
                MAX_SUMMARY_WORKSPACE_BYTES,
            ),
            created_at_ms: modified_at_ms,
            updated_at_ms: modified_at_ms,
            model: None,
            message_count: 0,
            first_prompt: None,
            schema_version: None,
            health: SessionHealth::Corrupt,
        }
    }
}

fn persist_dir_is_workspace_relative(persist_dir: &str) -> bool {
    let path = Path::new(persist_dir);
    persist_dir != "~"
        && !persist_dir.starts_with("~/")
        && path.is_relative()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// Resolves symlinks in the deepest existing prefix of a storage root.
///
/// Missing trailing components are reattached verbatim; they are created
/// later as real directories by the descriptor-relative walk.
fn canonicalize_storage_root(root: &Path) -> PathBuf {
    let mut suffix = Vec::new();
    let mut current = root.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut canonical) => {
                for component in suffix.iter().rev() {
                    canonical.push(component);
                }
                return canonical;
            }
            Err(_) => match (current.parent(), current.file_name()) {
                (Some(parent), Some(name)) => {
                    suffix.push(name.to_os_string());
                    current = parent.to_path_buf();
                }
                _ => return root.to_path_buf(),
            },
        }
    }
}

/// Collects valid legacy session entries not shadowed by scoped storage.
fn collect_legacy_list_entries(
    directory: &LegacyDirectory,
    seen_ids: &mut HashSet<ProviderSessionId>,
    entries: &mut Vec<super::listing::ListEntry>,
) -> Result<(), SessionError> {
    let dir_entries = Dir::read_from(&directory.file).map_err(|error| {
        io_error(
            "list legacy sessions",
            &directory.path,
            io::Error::from_raw_os_error(error.raw_os_error()),
        )
    })?;
    for entry in dir_entries.flatten() {
        let Ok(filename) = std::str::from_utf8(entry.file_name().to_bytes()) else {
            continue;
        };
        let Some(stem) = filename.strip_suffix(".json") else {
            continue;
        };
        let Ok(session_id) = ProviderSessionId::parse(stem) else {
            continue;
        };
        if !seen_ids.insert(session_id.clone()) {
            continue;
        }
        let Ok(Some(legacy)) = open_legacy_session_file(directory, &session_id) else {
            continue;
        };
        entries.push(super::listing::ListEntry {
            modified_at_ms: open_file_time_ms(&legacy.file),
            session_id,
        });
    }
    Ok(())
}

fn workspace_owned_legacy_dir(
    workspace: &Path,
    workspace_directory: &File,
    candidate: &Path,
) -> Option<LegacyDirectory> {
    let canonical = fs::canonicalize(candidate).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    if !metadata.is_dir() || !canonical.starts_with(workspace) {
        return None;
    }
    let relative = canonical.strip_prefix(workspace).ok()?;
    let file = open_relative_directory_no_follow(workspace_directory, relative).ok()?;
    file.metadata()
        .ok()
        .is_some_and(|metadata| metadata.is_dir())
        .then_some(LegacyDirectory {
            path: canonical,
            file,
        })
}

fn open_directory_path_no_follow(path: &Path) -> io::Result<File> {
    let descriptor = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(rustix_error_to_io)?;
    let root = File::from(descriptor);
    let relative = path
        .strip_prefix("/")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path is not absolute"))?;
    open_relative_directory_no_follow(&root, relative)
}

fn open_relative_directory_no_follow(root: &File, relative: &Path) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory path contains a non-normal component",
            ));
        };
        let descriptor = openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(rustix_error_to_io)?;
        directory = File::from(descriptor);
    }
    Ok(directory)
}

fn rustix_error_to_io(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

fn collect_session_ids_from_legacy_directory(
    directory: &LegacyDirectory,
    session_ids: &mut HashSet<ProviderSessionId>,
) -> Result<(), SessionError> {
    let entries = Dir::read_from(&directory.file).map_err(|error| {
        io_error(
            "list legacy IDs",
            &directory.path,
            io::Error::from_raw_os_error(error.raw_os_error()),
        )
    })?;
    for entry in entries.flatten() {
        let Ok(filename) = std::str::from_utf8(entry.file_name().to_bytes()) else {
            continue;
        };
        let Some(stem) = filename.strip_suffix(".json") else {
            continue;
        };
        let Ok(session_id) = ProviderSessionId::parse(stem) else {
            continue;
        };
        if open_legacy_session_file(directory, &session_id).is_ok_and(|entry| entry.is_some()) {
            session_ids.insert(session_id);
        }
    }
    Ok(())
}

fn open_legacy_session_file<'a>(
    directory: &'a LegacyDirectory,
    session_id: &ProviderSessionId,
) -> Result<Option<LegacySessionFile<'a>>, SessionError> {
    let filename = format!("{session_id}.json");
    let path = directory.path.join(&filename);
    let descriptor = match openat(
        &directory.file,
        filename.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => {
            return Err(SessionError::Corrupt {
                session_id: session_id.to_string(),
                message: "legacy session entry is a symbolic link".to_string(),
            });
        }
        Err(error) => {
            return Err(io_error(
                "open legacy session",
                &path,
                io::Error::from_raw_os_error(error.raw_os_error()),
            ));
        }
    };
    let file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect legacy session", &path, error))?;
    if !metadata.is_file() {
        return Err(SessionError::Corrupt {
            session_id: session_id.to_string(),
            message: "legacy session entry is not a regular file".to_string(),
        });
    }
    Ok(Some(LegacySessionFile {
        directory,
        filename,
        path,
        file,
    }))
}

fn lock_legacy_session_file<'a>(
    legacy: LegacySessionFile<'a>,
    session_id: &ProviderSessionId,
) -> Result<LegacySessionFile<'a>, SessionError> {
    match try_exclusive_lock(&legacy.file) {
        Ok(true) => Ok(legacy),
        Ok(false) => Err(SessionError::Conflict {
            session_id: session_id.to_string(),
        }),
        Err(error) => Err(io_error("lock legacy session", &legacy.path, error)),
    }
}

fn remove_locked_legacy_session_file(
    legacy: LegacySessionFile<'_>,
    session_id: &ProviderSessionId,
) -> Result<(), SessionError> {
    let result = unlinkat(
        &legacy.directory.file,
        legacy.filename.as_str(),
        AtFlags::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            SessionError::NotFound {
                session_id: session_id.to_string(),
            }
        } else {
            io_error(
                "remove",
                &legacy.path,
                io::Error::from_raw_os_error(error.raw_os_error()),
            )
        }
    });
    unlock_file(&legacy.file);
    drop(legacy);
    result
}

fn decode_session_content<'a>(
    session_id: &ProviderSessionId,
    bytes: &'a [u8],
) -> Result<&'a str, SessionError> {
    std::str::from_utf8(bytes).map_err(|error| SessionError::Corrupt {
        session_id: session_id.to_string(),
        message: format!("invalid UTF-8: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::super::io::{
        private_open_options, reset_session_file_read_count, session_file_read_count,
        write_atomic_file,
    };
    use super::super::listing::ListEntry;
    use super::super::summary::{
        MAX_PROMPT_PREVIEW_CHARS, MAX_SUMMARY_MODEL_BYTES, MAX_SUMMARY_WORKSPACE_BYTES,
    };
    use super::*;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn store(temp: &tempfile::TempDir) -> SessionStore {
        SessionStore::for_workspace(temp.path().join("sessions").to_str().unwrap(), temp.path())
            .unwrap()
    }

    fn new_session(store: &SessionStore, prompt: &str) -> PersistedSession {
        PersistedSession::new(
            ProviderSessionId::new(),
            store.workspace_scope().to_string(),
            "mock-model".to_string(),
            vec![Message::user(prompt), Message::assistant("done")],
        )
    }

    fn write_invalid_utf8_session(store: &SessionStore) -> ProviderSessionId {
        fs::create_dir_all(&store.base_dir).unwrap();
        let session_id = ProviderSessionId::new();
        fs::write(store.session_file(&session_id), b"{\"prompt\":\"\xff\"}").unwrap();
        session_id
    }

    #[test]
    fn versioned_persist_and_load_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "hello");

        store.persist(&mut session).unwrap();
        let loaded = store.load(&session.session_id).unwrap();

        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.workspace_scope, store.workspace_scope());
    }

    #[test]
    fn persisted_and_loaded_sessions_are_redacted() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let secret = "sk-session-secret-value";
        // Bypass the redacting Message constructors so this exercises the
        // store's own persist-time redaction boundary.
        let raw = Message {
            role: "user".to_string(),
            content: crate::provider::MessageContent::Text(format!("use api_key={secret}")),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        };
        let mut session = PersistedSession::new(
            ProviderSessionId::new(),
            store.workspace_scope().to_string(),
            "mock-model".to_string(),
            vec![raw],
        );

        store.persist(&mut session).unwrap();

        // The on-disk envelope must not leak the secret, while the caller's
        // in-memory copy keeps the original turn context.
        let content = fs::read_to_string(store.session_file(&session.session_id)).unwrap();
        assert!(!content.contains(secret), "{content}");
        assert!(content.contains("<redacted>"), "{content}");
        assert!(session.messages[0].content.as_text().contains(secret));

        let loaded = store.load(&session.session_id).unwrap();
        assert!(!loaded.messages[0].content.as_text().contains(secret));
    }

    #[test]
    fn loading_legacy_sessions_redacts_before_replay() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let id = ProviderSessionId::new();
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz123456";
        let raw = Message {
            role: "user".to_string(),
            content: crate::provider::MessageContent::Text(format!("replay {secret}")),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        };
        fs::write(
            legacy_dir.join(format!("{id}.json")),
            serde_json::to_vec(&vec![raw]).unwrap(),
        )
        .unwrap();

        let loaded = store.load(&id).unwrap();

        assert!(!loaded.messages[0].content.as_text().contains(secret));
        assert!(loaded.messages[0].content.as_text().contains("<redacted>"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_workspace_paths_are_rejected_before_scope_hashing() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'w', 0xfe]));
        let second = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'w', 0xff]));
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();

        for workspace in [&first, &second] {
            assert!(matches!(
                SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, workspace),
                Err(SessionError::InvalidRequest { ref message })
                    if message.contains("not valid UTF-8")
            ));
        }
    }

    #[test]
    fn summaries_are_newest_first_and_derive_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut older = new_session(&store, "first prompt");
        store.persist(&mut older).unwrap();
        let mut newer = new_session(&store, "second prompt");
        store.persist(&mut newer).unwrap();
        newer.updated_at_ms = older.updated_at_ms.saturating_add(10);
        store.persist(&mut newer).unwrap();

        let (summaries, cursor) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].session_id, newer.session_id);
        assert_eq!(summaries[0].first_prompt.as_deref(), Some("second prompt"));
        assert_eq!(summaries[0].message_count, 2);
        assert_eq!(summaries[0].health, SessionHealth::Ready);
        assert!(cursor.is_none());

        let (first_page, cursor) = store.list(1, None).unwrap();
        assert_eq!(first_page.len(), 1);
        let cursor = cursor.expect("second page cursor");
        let (second_page, final_cursor) = store.list(1, Some(&cursor)).unwrap();
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].session_id, second_page[0].session_id);
        assert!(final_cursor.is_none());
    }

    #[test]
    fn summaries_bound_untrusted_model_and_mismatched_workspace_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "bounded metadata");
        session.model = "🧠".repeat(300_000);
        store.persist(&mut session).unwrap();

        for summary in [
            store.inspect(&session.session_id).unwrap(),
            store.validate(&session.session_id).unwrap(),
            store.list(1, None).unwrap().0.remove(0),
        ] {
            let model = summary.model.expect("bounded model");
            assert!(model.len() <= MAX_SUMMARY_MODEL_BYTES);
            assert!(model.ends_with('…'));
            assert!(summary.workspace_scope.len() <= MAX_SUMMARY_WORKSPACE_BYTES);
        }

        let mut mismatch = new_session(&store, "mismatched scope");
        mismatch.workspace_scope = "🗂".repeat(300_000);
        fs::write(
            store.session_file(&mismatch.session_id),
            serde_json::to_vec(&mismatch).unwrap(),
        )
        .unwrap();

        let inspected = store.inspect(&mismatch.session_id).unwrap();
        assert_eq!(inspected.health, SessionHealth::ScopeMismatch);
        assert!(inspected.workspace_scope.len() <= MAX_SUMMARY_WORKSPACE_BYTES);
        assert!(inspected.workspace_scope.ends_with('…'));
        let listed = store
            .list(MAX_LIST_LIMIT, None)
            .unwrap()
            .0
            .into_iter()
            .find(|summary| summary.session_id == mismatch.session_id)
            .expect("mismatched summary remains visible");
        assert_eq!(listed.health, SessionHealth::ScopeMismatch);
        assert!(listed.workspace_scope.len() <= MAX_SUMMARY_WORKSPACE_BYTES);
    }

    #[test]
    fn stable_cursor_survives_deletion_of_previous_page() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut older = new_session(&store, "older");
        store.persist(&mut older).unwrap();
        let mut newer = new_session(&store, "newer");
        store.persist(&mut newer).unwrap();
        newer.updated_at_ms = older.updated_at_ms.saturating_add(10);
        store.persist(&mut newer).unwrap();

        let (first_page, cursor) = store.list(1, None).unwrap();
        assert_eq!(first_page[0].session_id, newer.session_id);
        fs::remove_file(store.session_file(&newer.session_id)).unwrap();

        let (second_page, final_cursor) = store
            .list(1, cursor.as_deref())
            .expect("stable second page");

        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].session_id, older.session_id);
        assert!(final_cursor.is_none());
    }

    #[test]
    fn invalid_list_cursor_is_typed() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);

        assert!(matches!(
            store.list(10, Some("not-a-cursor")),
            Err(SessionError::InvalidCursor { .. })
        ));
    }

    #[test]
    fn list_keeps_healthy_sessions_visible_beside_invalid_utf8() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut healthy = new_session(&store, "healthy prompt");
        store.persist(&mut healthy).unwrap();
        let corrupt = write_invalid_utf8_session(&store);

        let (summaries, cursor) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 2);
        assert!(cursor.is_none());
        assert!(summaries.iter().any(|summary| {
            summary.session_id == healthy.session_id && summary.health == SessionHealth::Ready
        }));
        assert!(summaries.iter().any(|summary| {
            summary.session_id == corrupt && summary.health == SessionHealth::Corrupt
        }));
    }

    #[test]
    fn list_skips_unreadable_uuid_entry_without_hiding_healthy_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut healthy = new_session(&store, "healthy prompt");
        store.persist(&mut healthy).unwrap();
        let directory_id = ProviderSessionId::new();
        fs::create_dir(store.session_file(&directory_id)).unwrap();

        let (summaries, cursor) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, healthy.session_id);
        assert!(cursor.is_none());
    }

    #[test]
    fn list_fills_page_after_filtered_entries() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let skipped = ProviderSessionId::parse("00000000-0000-4000-8000-000000000000").unwrap();
        let first_ready = ProviderSessionId::parse("11111111-1111-4111-8111-111111111111").unwrap();
        let second_ready =
            ProviderSessionId::parse("22222222-2222-4222-8222-222222222222").unwrap();
        let entries = [&skipped, &first_ready, &second_ready]
            .into_iter()
            .enumerate()
            .map(|(index, session_id)| ListEntry {
                session_id: session_id.clone(),
                modified_at_ms: 3_u64.saturating_sub(index as u64),
            })
            .collect::<Vec<_>>();

        let (first_page, examined_end) = collect_list_page(&entries, 0, 1, |entry| {
            (entry.session_id != skipped)
                .then(|| store.corrupt_summary(&entry.session_id, entry.modified_at_ms))
        });
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].session_id, first_ready);
        assert_eq!(examined_end, 2);

        let (second_page, final_end) = collect_list_page(&entries, examined_end, 1, |entry| {
            Some(store.corrupt_summary(&entry.session_id, entry.modified_at_ms))
        });
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].session_id, second_ready);
        assert_eq!(final_end, entries.len());
    }

    #[test]
    fn summary_preview_is_single_line_unicode_safe_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let prompt = format!(
            "first\n\t{}  last",
            "界".repeat(MAX_PROMPT_PREVIEW_CHARS + 50)
        );
        let mut session = new_session(&store, &prompt);
        store.persist(&mut session).unwrap();

        let summary = store.validate(&session.session_id).unwrap();
        let preview = summary.first_prompt.expect("bounded preview");

        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\t'));
        assert_eq!(preview.chars().count(), MAX_PROMPT_PREVIEW_CHARS);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn inspect_reports_invalid_utf8_as_corrupt_summary() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let corrupt = write_invalid_utf8_session(&store);

        let summary = store.inspect(&corrupt).unwrap();

        assert_eq!(summary.session_id, corrupt);
        assert_eq!(summary.workspace_scope, store.workspace_scope());
        assert_eq!(summary.health, SessionHealth::Corrupt);
        assert_eq!(summary.schema_version, None);
    }

    #[test]
    fn load_and_validate_classify_invalid_utf8_as_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let corrupt = write_invalid_utf8_session(&store);

        assert!(matches!(
            store.load(&corrupt),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("invalid UTF-8")
        ));
        assert!(matches!(
            store.validate(&corrupt),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("invalid UTF-8")
        ));
    }

    #[test]
    fn legacy_array_loads_and_upgrades_on_write() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace("sessions", &workspace).unwrap();
        let legacy_dir = store.legacy_dirs[0].path.clone();
        let id = ProviderSessionId::new();
        let legacy_file = legacy_dir.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("legacy")]).unwrap(),
        )
        .unwrap();
        assert!(!store.session_file(&id).exists());

        let mut loaded = store.load(&id).unwrap();
        assert_eq!(loaded.generation, 0);
        assert_eq!(loaded.messages.len(), 1);
        assert!(!store.session_file(&id).exists());
        assert!(legacy_file.exists());

        loaded.messages.push(Message::assistant("upgraded"));
        store.persist(&mut loaded).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.session_file(&id)).unwrap()).unwrap();
        assert_eq!(value["schema_version"], CURRENT_SCHEMA_VERSION);
        assert_eq!(value["generation"], 1);
        assert!(!legacy_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_cleanup_failure_is_reported_and_retried_after_migration() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace("sessions", &workspace).unwrap();
        fs::create_dir_all(&store.base_dir).unwrap();
        let id = ProviderSessionId::new();
        let legacy_file = legacy_dir.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("legacy")]).unwrap(),
        )
        .unwrap();
        let mut loaded = store.load(&id).unwrap();
        loaded.messages.push(Message::assistant("migrated"));
        fs::set_permissions(&legacy_dir, fs::Permissions::from_mode(0o500)).unwrap();

        let result = store.persist(&mut loaded);

        fs::set_permissions(&legacy_dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            result,
            Err(SessionError::Io {
                operation: "remove",
                ref path,
                ..
            }) if path == &legacy_file
        ));
        assert_eq!(loaded.generation, 1);
        assert!(store.session_file(&id).exists());
        assert!(legacy_file.exists());

        store.persist(&mut loaded).unwrap();
        assert_eq!(loaded.generation, 2);
        assert!(!legacy_file.exists());
        assert_eq!(store.load(&id).unwrap().messages.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn clear_keeps_scoped_history_when_legacy_removal_fails() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace("sessions", &workspace).unwrap();
        let mut current = new_session(&store, "latest scoped history");
        store.persist(&mut current).unwrap();
        let scoped_file = store.session_file(&current.session_id);
        let legacy_file = legacy_dir.join(format!("{}.json", current.session_id));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("stale legacy history")]).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&legacy_dir, fs::Permissions::from_mode(0o500)).unwrap();

        let result = store.clear(&current.session_id, &[]);

        fs::set_permissions(&legacy_dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            result,
            Err(SessionError::Io {
                operation: "remove",
                ref path,
                ..
            }) if path == &legacy_file
        ));
        assert!(scoped_file.exists());
        assert!(legacy_file.exists());
        let loaded = store.load(&current.session_id).unwrap();
        assert_eq!(
            loaded.messages[0].content.as_text(),
            "latest scoped history"
        );
    }

    #[test]
    fn failed_scoped_validation_keeps_legacy_copy() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace("sessions", &workspace).unwrap();
        fs::create_dir_all(&store.base_dir).unwrap();
        let session = new_session(&store, "replacement");
        let legacy_file = legacy_dir.join(format!("{}.json", session.session_id));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("recoverable legacy")]).unwrap(),
        )
        .unwrap();
        let scoped_file = store.session_file(&session.session_id);
        fs::write(&scoped_file, b"{broken scoped").unwrap();
        let mut candidate = session;

        assert!(matches!(
            store.persist(&mut candidate),
            Err(SessionError::Corrupt { .. })
        ));
        assert!(legacy_file.exists());
        assert!(scoped_file.exists());
    }

    #[test]
    fn legacy_inspect_and_validate_do_not_migrate_before_persist() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let id = ProviderSessionId::new();
        let legacy_file = legacy_dir.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("read only")]).unwrap(),
        )
        .unwrap();

        assert_eq!(store.inspect(&id).unwrap().health, SessionHealth::Ready);
        assert_eq!(store.validate(&id).unwrap().health, SessionHealth::Ready);
        assert!(legacy_file.exists());
        assert!(!store.session_file(&id).exists());
    }

    #[test]
    fn regular_file_is_not_accepted_as_a_legacy_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("sessions"), b"not a directory").unwrap();

        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();

        assert!(store.legacy_dirs.is_empty());
        assert!(store.session_ids().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn pinned_legacy_directory_fd_prevents_symlink_swap_during_load_and_clear() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        let pinned_dir = workspace.join("sessions-old");
        let external_dir = temp.path().join("external");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::create_dir_all(&external_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let id = ProviderSessionId::new();
        fs::write(
            legacy_dir.join(format!("{id}.json")),
            serde_json::to_vec(&vec![Message::user("workspace-owned")]).unwrap(),
        )
        .unwrap();
        let external_file = external_dir.join(format!("{id}.json"));
        fs::write(
            &external_file,
            serde_json::to_vec(&vec![Message::user("must remain external")]).unwrap(),
        )
        .unwrap();

        fs::rename(&legacy_dir, &pinned_dir).unwrap();
        std::os::unix::fs::symlink(&external_dir, &legacy_dir).unwrap();

        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.messages[0].content.as_text(), "workspace-owned");
        assert_eq!(store.session_ids().unwrap(), vec![id.clone()]);
        store.clear(&id, &[]).unwrap();

        assert!(!pinned_dir.join(format!("{id}.json")).exists());
        assert!(external_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_session_symlink_is_never_followed_by_load_or_clear() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        let external_dir = temp.path().join("external");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::create_dir_all(&external_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let id = ProviderSessionId::new();
        let external_file = external_dir.join(format!("{id}.json"));
        fs::write(
            &external_file,
            serde_json::to_vec(&vec![Message::user("external")]).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&external_file, legacy_dir.join(format!("{id}.json"))).unwrap();

        assert!(matches!(
            store.load(&id),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("symbolic link")
        ));
        assert!(store.session_ids().unwrap().is_empty());
        assert!(matches!(
            store.clear(&id, &[]),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("symbolic link")
        ));
        assert!(external_file.exists());
    }

    #[test]
    fn shared_flat_legacy_root_is_not_claimed_by_an_ancestor_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        fs::create_dir_all(&shared).unwrap();
        let store = SessionStore::for_workspace(shared.to_str().unwrap(), temp.path()).unwrap();
        let id = ProviderSessionId::new();
        let legacy_file = shared.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("ambiguous")]).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.load(&id),
            Err(SessionError::NotFound { .. })
        ));
        assert!(legacy_file.exists());
        assert!(!store.session_file(&id).exists());
        assert!(store.session_ids().unwrap().is_empty());
        assert!(matches!(
            store.clear(&id, &[]),
            Err(SessionError::NotFound { .. })
        ));
        assert!(legacy_file.exists());
    }

    #[test]
    fn parent_relative_legacy_root_is_not_shared_by_sibling_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let first_workspace = temp.path().join("first");
        let second_workspace = temp.path().join("second");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(first_workspace.join("nested")).unwrap();
        fs::create_dir_all(second_workspace.join("nested")).unwrap();
        let id = ProviderSessionId::new();
        let legacy_file = shared.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("shared legacy")]).unwrap(),
        )
        .unwrap();

        for persist_dir in ["../shared", "nested/../../shared"] {
            for workspace in [&first_workspace, &second_workspace] {
                let store = SessionStore::for_workspace(persist_dir, workspace).unwrap();
                assert!(matches!(
                    store.load(&id),
                    Err(SessionError::NotFound { .. })
                ));
                assert!(store.session_ids().unwrap().is_empty());
                assert!(matches!(
                    store.clear(&id, &[]),
                    Err(SessionError::NotFound { .. })
                ));
            }
        }
        assert!(legacy_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn relative_symlink_cannot_escape_workspace_for_legacy_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let shared = temp.path().join("shared");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&shared).unwrap();
        std::os::unix::fs::symlink(&shared, workspace.join("sessions-link")).unwrap();
        let id = ProviderSessionId::new();
        let legacy_file = shared.join(format!("{id}.json"));
        fs::write(
            &legacy_file,
            serde_json::to_vec(&vec![Message::user("escaped legacy")]).unwrap(),
        )
        .unwrap();

        // The symlinked root is resolved once at construction time; the
        // escaped directory is never claimed as workspace-owned legacy
        // storage, so foreign session files stay invisible and untouched.
        let store = SessionStore::for_workspace("sessions-link", &workspace).unwrap();
        assert!(matches!(
            store.load(&id),
            Err(SessionError::NotFound { .. })
        ));
        assert!(store.session_ids().unwrap().is_empty());
        assert!(store.list(10, None).unwrap().0.is_empty());
        assert!(matches!(
            store.clear(&id, &[]),
            Err(SessionError::NotFound { .. })
        ));
        assert!(legacy_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn scoped_directory_symlink_cannot_cross_workspace_on_clear() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("scoped");
        let first_workspace = temp.path().join("first");
        let second_workspace = temp.path().join("second");
        fs::create_dir_all(&first_workspace).unwrap();
        fs::create_dir_all(&second_workspace).unwrap();
        let first = SessionStore::for_workspace(root.to_str().unwrap(), &first_workspace).unwrap();
        let mut session = new_session(&first, "first workspace");
        first.persist(&mut session).unwrap();
        let first_base_dir = first.base_dir.clone();
        let second =
            SessionStore::for_workspace(root.to_str().unwrap(), &second_workspace).unwrap();
        std::os::unix::fs::symlink(&first_base_dir, &second.base_dir).unwrap();

        assert!(matches!(
            second.load(&session.session_id),
            Err(SessionError::Io { .. })
        ));
        assert!(matches!(
            second.list(10, None),
            Err(SessionError::Io { .. })
        ));
        assert!(matches!(second.session_ids(), Err(SessionError::Io { .. })));
        let mut second_session = new_session(&second, "second workspace");
        assert!(matches!(
            second.persist(&mut second_session),
            Err(SessionError::Io { .. })
        ));
        assert!(matches!(
            second.clear(&session.session_id, &[]),
            Err(SessionError::Io { .. })
        ));
        assert!(first.load(&session.session_id).is_ok());
        assert!(matches!(
            SessionStore::for_workspace(root.to_str().unwrap(), &second_workspace),
            Err(SessionError::Io { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_session_symlink_is_never_followed_by_load_or_clear() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("scoped");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::for_workspace(root.to_str().unwrap(), &workspace).unwrap();
        let directory = store.scoped.directory(true).unwrap().unwrap();
        let id = ProviderSessionId::new();
        let external = temp.path().join("external.json");
        fs::write(&external, b"external").unwrap();
        std::os::unix::fs::symlink(&external, store.session_file(&id)).unwrap();

        assert!(matches!(
            store.load(&id),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("symbolic link")
        ));
        assert!(matches!(
            store.clear(&id, &[]),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("symbolic link")
        ));
        assert_eq!(fs::read(&external).unwrap(), b"external");
        drop(directory);
    }

    #[test]
    fn default_legacy_source_is_only_the_workspace_sessions_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("sessions")).unwrap();

        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();

        assert_eq!(store.legacy_dirs.len(), 1);
        assert_eq!(store.legacy_dirs[0].path, workspace.join("sessions"));
    }

    #[test]
    fn list_includes_workspace_owned_legacy_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let mut scoped = new_session(&store, "scoped prompt");
        store.persist(&mut scoped).unwrap();
        let legacy_id = ProviderSessionId::new();
        fs::write(
            legacy_dir.join(format!("{legacy_id}.json")),
            serde_json::to_vec(&vec![Message::user("legacy prompt")]).unwrap(),
        )
        .unwrap();
        let corrupt_id = ProviderSessionId::new();
        fs::write(legacy_dir.join(format!("{corrupt_id}.json")), b"{broken").unwrap();

        let (summaries, cursor) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 3);
        assert!(cursor.is_none());
        assert!(summaries.iter().any(|summary| {
            summary.session_id == scoped.session_id && summary.health == SessionHealth::Ready
        }));
        assert!(summaries.iter().any(|summary| {
            summary.session_id == legacy_id
                && summary.health == SessionHealth::Ready
                && summary.first_prompt.as_deref() == Some("legacy prompt")
        }));
        assert!(summaries.iter().any(|summary| {
            summary.session_id == corrupt_id && summary.health == SessionHealth::Corrupt
        }));
    }

    #[test]
    fn scoped_entry_shadows_legacy_copy_in_list() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let mut session = new_session(&store, "current scoped prompt");
        store.persist(&mut session).unwrap();
        fs::write(
            legacy_dir.join(format!("{}.json", session.session_id)),
            serde_json::to_vec(&vec![Message::user("stale legacy prompt")]).unwrap(),
        )
        .unwrap();

        let (summaries, _) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].first_prompt.as_deref(),
            Some("current scoped prompt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_storage_root_resolves_before_descriptor_pinning() {
        let temp = tempfile::tempdir().unwrap();
        let real_root = temp.path().join("real-root");
        fs::create_dir_all(&real_root).unwrap();
        let linked_root = temp.path().join("linked-root");
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();

        let store =
            SessionStore::for_workspace(linked_root.to_str().unwrap(), temp.path()).unwrap();
        let mut session = new_session(&store, "through symlinked root");
        store.persist(&mut session).unwrap();

        assert_eq!(store.load(&session.session_id).unwrap().generation, 1);
        assert!(store
            .session_file(&session.session_id)
            .starts_with(fs::canonicalize(&real_root).unwrap()));
    }

    #[test]
    fn clear_removes_paired_lock_file() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "lock cleanup");
        store.persist(&mut session).unwrap();
        let lock_path = store
            .base_dir
            .join(format!(".{}.lock", session.session_id.as_str()));
        assert!(lock_path.exists());

        store.clear(&session.session_id, &[]).unwrap();

        assert!(!store.session_file(&session.session_id).exists());
        assert!(!lock_path.exists());
    }

    #[test]
    fn persist_recovers_after_external_directory_removal() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut first = new_session(&store, "before removal");
        store.persist(&mut first).unwrap();

        fs::remove_dir_all(&store.base_dir).unwrap();

        let mut second = new_session(&store, "after removal");
        store.persist(&mut second).unwrap();
        assert_eq!(store.load(&second.session_id).unwrap().generation, 1);
    }

    #[test]
    fn list_shows_legacy_sessions_without_any_scoped_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let legacy_id = ProviderSessionId::new();
        fs::write(
            legacy_dir.join(format!("{legacy_id}.json")),
            serde_json::to_vec(&vec![Message::user("only legacy")]).unwrap(),
        )
        .unwrap();

        let (summaries, cursor) = store.list(10, None).unwrap();

        assert_eq!(summaries.len(), 1);
        assert!(cursor.is_none());
        assert_eq!(summaries[0].session_id, legacy_id);
        assert_eq!(summaries[0].health, SessionHealth::Ready);
    }

    #[cfg(unix)]
    #[test]
    fn stale_temporary_files_are_swept_before_first_write() {
        let temp = tempfile::tempdir().unwrap();
        let first = store(&temp);
        fs::create_dir_all(&first.base_dir).unwrap();
        let stale = first
            .base_dir
            .join(".00000000-0000-4000-8000-000000000000.stale.tmp");
        let fresh = first
            .base_dir
            .join(".11111111-1111-4111-8111-111111111111.fresh.tmp");
        fs::write(&stale, b"stale").unwrap();
        fs::write(&fresh, b"fresh").unwrap();
        // Backdate only the stale file beyond the sweep threshold.
        assert!(std::process::Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(&stale)
            .status()
            .unwrap()
            .success());

        // A second store pins the already existing directory at construction,
        // so the sweep must still run on its first write-mode open.
        let second = store(&temp);
        let mut session = new_session(&second, "sweep trigger");
        second.persist(&mut session).unwrap();

        assert!(!stale.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn clear_lists_and_removes_workspace_owned_legacy_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let legacy_dir = workspace.join("sessions");
        fs::create_dir_all(&legacy_dir).unwrap();
        let store = SessionStore::for_workspace(DEFAULT_SESSION_PERSIST_DIR, &workspace).unwrap();
        let valid = ProviderSessionId::new();
        let corrupt = ProviderSessionId::new();
        let valid_path = legacy_dir.join(format!("{valid}.json"));
        let corrupt_path = legacy_dir.join(format!("{corrupt}.json"));
        fs::write(
            &valid_path,
            serde_json::to_vec(&vec![Message::user("legacy")]).unwrap(),
        )
        .unwrap();
        fs::write(&corrupt_path, b"{broken legacy").unwrap();

        let ids = store.session_ids().unwrap();
        assert!(ids.contains(&valid));
        assert!(ids.contains(&corrupt));
        assert!(matches!(
            store.clear(&valid, std::slice::from_ref(&valid)),
            Err(SessionError::ActiveSession { .. })
        ));
        assert!(valid_path.exists());

        store.clear(&corrupt, &[]).unwrap();
        store.clear(&valid, &[]).unwrap();
        assert!(!corrupt_path.exists());
        assert!(!valid_path.exists());
        assert!(store.session_ids().unwrap().is_empty());
    }

    #[test]
    fn list_bounds_each_page_and_marks_oversized_files_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        fs::create_dir_all(&store.base_dir).unwrap();
        let oversized = ProviderSessionId::parse("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap();
        File::create(store.session_file(&oversized))
            .unwrap()
            .set_len(MAX_SESSION_FILE_BYTES + 1)
            .unwrap();
        let healthy_id = ProviderSessionId::parse("00000000-0000-4000-8000-000000000000").unwrap();
        let mut healthy = PersistedSession::new(
            healthy_id.clone(),
            store.workspace_scope().to_string(),
            "mock".to_string(),
            vec![Message::user("bounded page")],
        );
        store.persist(&mut healthy).unwrap();

        reset_session_file_read_count();
        let (first, cursor) = store.list(1, None).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].session_id, healthy_id);
        assert_eq!(session_file_read_count(), 1);
        let (second, final_cursor) = store.list(1, cursor.as_deref()).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].session_id, oversized);
        assert_eq!(second[0].health, SessionHealth::Corrupt);
        assert_eq!(session_file_read_count(), 2);
        assert!(final_cursor.is_none());
    }

    #[test]
    fn rejects_invalid_and_traversal_ids_before_path_use() {
        for value in [
            "../outside",
            "not-a-uuid",
            "A0Eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        ] {
            assert!(matches!(
                ProviderSessionId::parse(value),
                Err(SessionError::InvalidId { .. })
            ));
        }
    }

    #[test]
    fn distinguishes_missing_corrupt_incompatible_and_scope_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        fs::create_dir_all(&store.base_dir).unwrap();

        let missing = ProviderSessionId::new();
        assert!(matches!(
            store.load(&missing),
            Err(SessionError::NotFound { .. })
        ));

        let corrupt = ProviderSessionId::new();
        fs::write(store.session_file(&corrupt), b"{broken").unwrap();
        assert!(matches!(
            store.load(&corrupt),
            Err(SessionError::Corrupt { .. })
        ));

        let incompatible = ProviderSessionId::new();
        fs::write(
            store.session_file(&incompatible),
            format!(
                r#"{{"schema_version":99,"session_id":"{incompatible}","workspace_scope":"{}"}}"#,
                store.workspace_scope()
            ),
        )
        .unwrap();
        assert!(matches!(
            store.load(&incompatible),
            Err(SessionError::IncompatibleVersion { version: 99, .. })
        ));

        let mut mismatch = new_session(&store, "mismatch");
        mismatch.workspace_scope = "/other/workspace".to_string();
        fs::write(
            store.session_file(&mismatch.session_id),
            serde_json::to_vec(&mismatch).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load(&mismatch.session_id),
            Err(SessionError::ScopeMismatch { .. })
        ));
    }

    #[test]
    fn conflict_preserves_prior_good_file() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut first = new_session(&store, "original");
        store.persist(&mut first).unwrap();
        let mut stale = first.clone();
        first.messages.push(Message::assistant("new"));
        store.persist(&mut first).unwrap();

        stale.messages.push(Message::assistant("stale"));
        assert!(matches!(
            store.persist(&mut stale),
            Err(SessionError::Conflict { .. })
        ));
        let loaded = store.load(&first.session_id).unwrap();
        assert_eq!(loaded.generation, first.generation);
        assert_eq!(loaded.messages.len(), first.messages.len());
    }

    #[test]
    fn exhausted_generation_is_rejected_without_overwriting_history() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "generation limit");
        store.persist(&mut session).unwrap();
        let path = store.session_file(&session.session_id);
        session.generation = u64::MAX;
        fs::write(&path, serde_json::to_vec_pretty(&session).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        let mut loaded = store.load(&session.session_id).unwrap();
        loaded
            .messages
            .push(Message::assistant("must not overwrite"));

        assert!(matches!(
            store.persist(&mut loaded),
            Err(SessionError::Corrupt { ref message, .. })
                if message.contains("generation is exhausted")
        ));
        assert_eq!(loaded.generation, u64::MAX);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn atomic_write_failure_preserves_prior_file() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("session.json");
        let temp_path = temp.path().join("occupied.tmp");
        fs::write(&destination, b"prior-good-envelope").unwrap();
        fs::write(&temp_path, b"occupied").unwrap();

        assert!(write_atomic_file(&temp_path, &destination, b"replacement").is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"prior-good-envelope");
    }

    #[test]
    fn active_writer_lock_reports_conflict_without_mutating_session() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "locked");
        store.persist(&mut session).unwrap();
        let before = fs::read(store.session_file(&session.session_id)).unwrap();
        let lock_path = store
            .base_dir
            .join(format!(".{}.lock", session.session_id.as_str()));
        let lock = private_open_options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();

        session.messages.push(Message::assistant("must not commit"));
        assert!(matches!(
            store.persist(&mut session),
            Err(SessionError::Conflict { .. })
        ));
        assert_eq!(
            fs::read(store.session_file(&session.session_id)).unwrap(),
            before
        );
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::Unlock).unwrap();
        drop(lock);
        store.persist(&mut session).unwrap();
    }

    #[test]
    fn advisory_lock_child_process() {
        let Some(path) = std::env::var_os("COSH_SESSION_LOCK_CHILD") else {
            return;
        };
        let lock = private_open_options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();
        std::process::exit(86);
    }

    #[test]
    fn process_exit_releases_advisory_lock() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "crash-safe lock");
        store.persist(&mut session).unwrap();
        let lock_path = store
            .base_dir
            .join(format!(".{}.lock", session.session_id.as_str()));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "session::store::tests::advisory_lock_child_process",
                "--nocapture",
            ])
            .env("COSH_SESSION_LOCK_CHILD", &lock_path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86));

        session
            .messages
            .push(Message::assistant("commit after crash"));
        store.persist(&mut session).unwrap();
        assert_eq!(store.load(&session.session_id).unwrap().generation, 2);
    }

    #[cfg(unix)]
    #[test]
    fn persistence_enforces_private_directory_and_file_modes() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        fs::create_dir_all(&store.base_dir).unwrap();
        fs::set_permissions(&store.base_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let mut session = new_session(&store, "private history");
        let lock_path = store
            .base_dir
            .join(format!(".{}.lock", session.session_id.as_str()));
        fs::write(&lock_path, []).unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();

        store.persist(&mut session).unwrap();

        let directory_mode = fs::metadata(&store.base_dir).unwrap().permissions().mode() & 0o777;
        let session_mode = fs::metadata(store.session_file(&session.session_id))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let lock_mode = fs::metadata(lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(session_mode, 0o600);
        assert_eq!(lock_mode, 0o600);
    }

    #[test]
    fn clear_protects_active_session_and_reports_missing() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let mut session = new_session(&store, "protected");
        store.persist(&mut session).unwrap();

        assert!(matches!(
            store.clear(&session.session_id, &[session.session_id.clone()]),
            Err(SessionError::ActiveSession { .. })
        ));
        assert!(store.load(&session.session_id).is_ok());

        store.clear(&session.session_id, &[]).unwrap();
        assert!(matches!(
            store.clear(&session.session_id, &[]),
            Err(SessionError::NotFound { .. })
        ));

        let mut first = new_session(&store, "clear first");
        let mut second = new_session(&store, "clear second");
        store.persist(&mut first).unwrap();
        store.persist(&mut second).unwrap();
        store.clear(&first.session_id, &[]).unwrap();
        store.clear(&second.session_id, &[]).unwrap();
        assert!(matches!(
            store.load(&first.session_id),
            Err(SessionError::NotFound { .. })
        ));
        assert!(matches!(
            store.load(&second.session_id),
            Err(SessionError::NotFound { .. })
        ));
    }

    #[test]
    fn clear_missing_session_without_scoped_directory_reports_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session_id = ProviderSessionId::new();

        assert!(!store.base_dir.exists());
        assert!(matches!(
            store.clear(&session_id, &[]),
            Err(SessionError::NotFound {
                session_id: missing
            }) if missing == session_id.as_str()
        ));
        assert!(!store.base_dir.exists());
    }
}
