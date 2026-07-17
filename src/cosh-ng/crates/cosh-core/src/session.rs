use std::fs::OpenOptions;
use std::io::Write;
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

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.base_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("Failed to secure session dir: {e}"))?;
        }

        let mut safe_messages = messages.to_vec();
        crate::redaction::redact_messages(&mut safe_messages);
        let json = serde_json::to_string_pretty(&safe_messages)
            .map_err(|e| format!("Failed to serialize messages: {e}"))?;

        let path = self.session_path(session_id);
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|e| format!("Failed to open session file: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("Failed to secure session file: {e}"))?;
        }
        file.write_all(json.as_bytes())
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

        let mut messages: Vec<Message> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse session file: {e}"))?;
        crate::redaction::redact_messages(&mut messages);
        Ok(messages)
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
    use crate::provider::MessageContent;

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

    #[test]
    fn persisted_and_resumed_sessions_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_str().unwrap());
        let secret = "sk-session-secret-value";
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(format!("use api_key={secret}")),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }];

        store.persist("secret-session", &messages).unwrap();

        let content = std::fs::read_to_string(store.session_path("secret-session")).unwrap();
        assert!(!content.contains(secret), "{content}");
        assert!(content.contains("<redacted>"), "{content}");

        let resumed = store.resume("secret-session").unwrap();
        assert!(!resumed[0].content.as_text().contains(secret));
    }

    #[cfg(unix)]
    #[test]
    fn session_storage_uses_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("sessions");
        let store = SessionStore::new(session_dir.to_str().unwrap());

        store
            .persist("private-session", &[Message::user("safe")])
            .unwrap();

        let dir_mode = std::fs::metadata(&session_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = std::fs::metadata(store.session_path("private-session"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn loading_legacy_sessions_redacts_before_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.json");
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz123456";
        std::fs::write(
            &path,
            format!(
                r#"[{{"role":"user","content":"replay {secret}","tool_call_id":null,"name":null,"tool_calls":null}}]"#
            ),
        )
        .unwrap();

        let loaded = SessionStore::load_from_path(&path).unwrap();

        assert!(!loaded[0].content.as_text().contains(secret));
        assert!(loaded[0].content.as_text().contains("<redacted>"));
    }
}
