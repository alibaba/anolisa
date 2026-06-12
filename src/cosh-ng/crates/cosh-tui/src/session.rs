//! Session management for cosh-tui.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata about a session.
#[derive(Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub working_dir: String,
    pub command_count: usize,
}

/// History entries recorded in a session.
#[derive(Serialize, Deserialize)]
pub struct SessionHistory {
    pub entries: Vec<SessionHistoryEntry>,
}

/// A single history entry within a session.
#[derive(Serialize, Deserialize, Clone)]
pub struct SessionHistoryEntry {
    pub command: String,
    pub output: String,
    pub success: bool,
    pub timestamp: String,
}

/// Return the base directory for session storage.
pub fn session_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot-shell")
        .join("sessions")
}

/// Save session metadata and history to disk.
///
/// Atomic: each file is written to `<name>.tmp` then renamed over the target.
/// Permissions are tightened to 0700 directory / 0600 files because session
/// history may contain prompts, tool outputs, or chat content the user does
/// not want world-readable.
pub fn save_session(
    metadata: &SessionMetadata,
    history: &SessionHistory,
) -> std::io::Result<()> {
    let dir = session_dir().join(&metadata.id);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    write_atomic_0600(
        &dir.join("metadata.json"),
        serde_json::to_string_pretty(metadata)?.as_bytes(),
    )?;
    write_atomic_0600(
        &dir.join("history.json"),
        serde_json::to_string_pretty(history)?.as_bytes(),
    )?;
    Ok(())
}

fn write_atomic_0600(path: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

/// Load a session's history from disk.
#[allow(dead_code)]
pub fn load_session_history(session_id: &str) -> Option<SessionHistory> {
    let path = session_dir().join(session_id).join("history.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all saved sessions by scanning the session directory.
pub fn list_sessions() -> Vec<SessionMetadata> {
    let dir = session_dir();
    let mut sessions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("metadata.json");
            if meta_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_json::from_str::<SessionMetadata>(&content) {
                        sessions.push(meta);
                    }
                }
            }
        }
    }
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_save_and_load_session() {
        let test_id = format!("test_session_{}", std::process::id());
        let metadata = SessionMetadata {
            id: test_id.clone(),
            name: "test-session".to_string(),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            working_dir: "/tmp".to_string(),
            command_count: 3,
        };
        let history = SessionHistory {
            entries: vec![
                SessionHistoryEntry {
                    command: "pkg list".to_string(),
                    output: "vim nginx".to_string(),
                    success: true,
                    timestamp: "100ms".to_string(),
                },
                SessionHistoryEntry {
                    command: "bad cmd".to_string(),
                    output: "error".to_string(),
                    success: false,
                    timestamp: "50ms".to_string(),
                },
            ],
        };

        // Save
        save_session(&metadata, &history).expect("save_session failed");

        // Verify metadata
        let dir = session_dir().join(&test_id);
        assert!(dir.join("metadata.json").exists());
        assert!(dir.join("history.json").exists());

        // Load and verify
        let loaded = load_session_history(&test_id).expect("load failed");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].command, "pkg list");
        assert!(!loaded.entries[1].success);

        // List sessions should find it
        let sessions = list_sessions();
        assert!(sessions.iter().any(|s| s.id == test_id));

        // Cleanup
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_list_sessions_empty_dir() {
        // Just ensure it doesn't panic on missing dir
        let _sessions = list_sessions();
    }
}
