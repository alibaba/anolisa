use std::path::{Path, PathBuf};

use crate::provider::Message;

pub struct SessionStore {
    base_dir: PathBuf,
}

impl SessionStore {
    pub fn new(persist_dir: &str) -> Self {
        let base_dir = if persist_dir.starts_with('~') {
            dirs::home_dir().unwrap_or_default().join(&persist_dir[2..])
        } else {
            PathBuf::from(persist_dir)
        };
        Self { base_dir }
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.json"))
    }

    pub fn persist(&self, session_id: &str, messages: &[Message]) -> Result<(), String> {
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("Failed to create session dir: {e}"))?;

        let json = serde_json::to_string_pretty(messages)
            .map_err(|e| format!("Failed to serialize messages: {e}"))?;

        std::fs::write(self.session_path(session_id), json)
            .map_err(|e| format!("Failed to write session file: {e}"))?;

        Ok(())
    }

    pub fn resume(&self, session_id: &str) -> Result<Vec<Message>, String> {
        let path = self.session_path(session_id);
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Vec<Message>, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read session file: {e}"))?;

        serde_json::from_str(&content).map_err(|e| format!("Failed to parse session file: {e}"))
    }

    pub fn list(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    path.file_stem().map(|s| s.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn clear(&self, session_id: &str) -> Result<(), String> {
        let path = self.session_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove session file: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_str().unwrap());

        let messages = vec![Message::user("hello"), Message::assistant("hi there")];

        store.persist("test-session", &messages).unwrap();
        let loaded = store.resume("test-session").unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[1].role, "assistant");
    }

    #[test]
    fn list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_str().unwrap());

        store.persist("sess-1", &[Message::user("a")]).unwrap();
        store.persist("sess-2", &[Message::user("b")]).unwrap();

        let mut sessions = store.list();
        sessions.sort();
        assert_eq!(sessions, vec!["sess-1", "sess-2"]);
    }

    #[test]
    fn clear_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_str().unwrap());

        store.persist("sess-1", &[Message::user("a")]).unwrap();
        assert!(store.resume("sess-1").is_ok());

        store.clear("sess-1").unwrap();
        assert!(store.resume("sess-1").is_err());
    }

    #[test]
    fn resume_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_str().unwrap());
        assert!(store.resume("nonexistent").is_err());
    }

    #[test]
    fn list_empty_dir() {
        let store = SessionStore::new("/nonexistent/path");
        assert!(store.list().is_empty());
    }
}
