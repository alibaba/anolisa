//! Response ID → Session ID Mapping
//!
//! Processes FileWrite events from the eBPF filewrite probe to extract
//! `responseId` from JSONL content and map it to `sessionId` (the UUID filename).
//!
//! # Data Flow
//!
//! ```text
//! FileWriteEvent { filename: "<UUID>.jsonl", buf: JSONL bytes }
//!     → parse filename to extract UUID as session_id
//!     → parse buf lines as JSON, extract "responseId" field
//!     → store responseId → sessionId in LRU cache
//! ```

use std::num::NonZeroUsize;

use lru::LruCache;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::probes::FileWriteEvent;

/// Maximum number of responseId → sessionId entries kept in memory.
const MAX_RESPONSE_MAP_ENTRIES: usize = 10_000;

/// Regex to match `responseId":"<value>"` or `response_id":"<value>"` in raw text.
/// Supports both camelCase (used by some agents) and snake_case (used by cosh).
static RESPONSE_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?:responseId|response_id)":"([^"]+)"#).unwrap());

/// Processes FileWrite events to build an in-memory responseId → sessionId mapping.
/// Uses an LRU cache to bound memory usage.
pub struct ResponseSessionMapper {
    /// responseId → sessionId (bounded by LRU eviction)
    map: LruCache<String, String>,
}

impl Default for ResponseSessionMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseSessionMapper {
    /// Create a new empty mapper with default capacity.
    pub fn new() -> Self {
        ResponseSessionMapper {
            map: LruCache::new(
                NonZeroUsize::new(MAX_RESPONSE_MAP_ENTRIES).unwrap(),
            ),
        }
    }

    /// Process a FileWriteEvent:
    /// 1. Extract UUID from filename as session_id
    /// 2. Parse buf as UTF-8, split by '\n'
    /// 3. For each line, try JSON parse and extract "responseId"
    /// 4. Insert responseId → sessionId into the map
    pub fn process_filewrite(&mut self, event: &FileWriteEvent) {
        let session_id = match Self::extract_session_id(&event.filename) {
            Some(id) => id,
            None => {
                log::trace!(
                    "ResponseSessionMapper: filename not UUID.jsonl format: {}",
                    event.filename
                );
                return;
            }
        };

        let text = match std::str::from_utf8(&event.buf) {
            Ok(s) => s,
            Err(e) => {
                log::trace!(
                    "ResponseSessionMapper: buf is not valid UTF-8 for {}: {}",
                    event.filename,
                    e
                );
                return;
            }
        };

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(response_id) = Self::extract_response_id(line) {
                log::debug!(
                    "ResponseSessionMapper: responseId={} → sessionId={}",
                    response_id,
                    session_id
                );
                self.map.put(response_id, session_id.clone());
            }
        }
    }

    /// Look up sessionId by responseId.
    /// Uses `peek` (no LRU promotion) since each responseId is typically looked up only once.
    pub fn get_session_by_response_id(&self, response_id: &str) -> Option<&str> {
        self.map.peek(response_id).map(|s| s.as_str())
    }

    /// Extract UUID from a filename like `<UUID>.jsonl` or `/path/to/<UUID>.jsonl`.
    /// Returns the UUID portion (without path prefix or `.jsonl` suffix).
    fn extract_session_id(filename: &str) -> Option<String> {
        // Take the last path component
        let basename = filename.rsplit('/').next().unwrap_or(filename);

        // Strip .jsonl suffix
        let uuid = basename.strip_suffix(".jsonl")?;

        // Basic UUID length validation (36 chars: 8-4-4-4-12)
        if uuid.len() == 36 {
            Some(uuid.to_string())
        } else {
            None
        }
    }

    /// Extract "responseId" or "response_id" value from a single JSONL line using regex.
    /// Matches patterns like `responseId":"chatcmpl-xxxx"` or `response_id":"chatcmpl-xxxx"`.
    fn extract_response_id(line: &str) -> Option<String> {
        RESPONSE_ID_RE
            .captures(line)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_id_simple() {
        let id = ResponseSessionMapper::extract_session_id(
            "550e8400-e29b-41d4-a716-446655440000.jsonl",
        );
        assert_eq!(id.as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_extract_session_id_with_path() {
        let id = ResponseSessionMapper::extract_session_id(
            "/home/user/.agent/550e8400-e29b-41d4-a716-446655440000.jsonl",
        );
        assert_eq!(id.as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_extract_session_id_not_jsonl() {
        assert!(ResponseSessionMapper::extract_session_id("file.txt").is_none());
    }

    #[test]
    fn test_extract_session_id_wrong_length() {
        assert!(ResponseSessionMapper::extract_session_id("short.jsonl").is_none());
    }

    #[test]
    fn test_extract_response_id() {
        let line = r#"{"responseId":"chatcmpl-03a158a1-8982-90cd-adb1-6c8a1176f1f8","other":"data"}"#;
        let id = ResponseSessionMapper::extract_response_id(line);
        assert_eq!(
            id.as_deref(),
            Some("chatcmpl-03a158a1-8982-90cd-adb1-6c8a1176f1f8")
        );
    }

    #[test]
    fn test_extract_response_id_snake_case() {
        // cosh writes response_id in snake_case
        let line = r#"{"response_id":"chatcmpl-f2748a8e-85d0-9058-b28f-c70e6f5fd590","model":"qwen-plus"}"#;
        let id = ResponseSessionMapper::extract_response_id(line);
        assert_eq!(
            id.as_deref(),
            Some("chatcmpl-f2748a8e-85d0-9058-b28f-c70e6f5fd590")
        );
    }

    #[test]
    fn test_extract_response_id_missing() {
        let line = r#"{"other":"data"}"#;
        assert!(ResponseSessionMapper::extract_response_id(line).is_none());
    }

    #[test]
    fn test_extract_response_id_empty() {
        let line = r#"{"responseId":""}"#;
        assert!(ResponseSessionMapper::extract_response_id(line).is_none());
    }

    #[test]
    fn test_extract_response_id_invalid_json() {
        assert!(ResponseSessionMapper::extract_response_id("not json").is_none());
    }

    #[test]
    fn test_process_and_query() {
        let mut mapper = ResponseSessionMapper::new();
        let event = FileWriteEvent {
            pid: 1234,
            tid: 1234,
            uid: 1000,
            timestamp_ns: 0,
            write_size: 0,
            comm: "agent".to_string(),
            filename: "550e8400-e29b-41d4-a716-446655440000.jsonl".to_string(),
            buf: br#"{"responseId":"chatcmpl-abc123","content":"hello"}
{"responseId":"chatcmpl-def456","content":"world"}
"#
            .to_vec(),
        };
        mapper.process_filewrite(&event);

        assert_eq!(
            mapper.get_session_by_response_id("chatcmpl-abc123"),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(
            mapper.get_session_by_response_id("chatcmpl-def456"),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert!(mapper.get_session_by_response_id("nonexistent").is_none());
    }

    #[test]
    fn test_process_and_query_snake_case() {
        // cosh writes response_id (snake_case) in ui_telemetry records
        let mut mapper = ResponseSessionMapper::new();
        let event = FileWriteEvent {
            pid: 5678,
            tid: 5678,
            uid: 1000,
            timestamp_ns: 0,
            write_size: 0,
            comm: "node".to_string(),
            filename: "a1b2c3d4-e5f6-7890-abcd-ef1234567890.jsonl".to_string(),
            buf: br#"{"type":"system","subtype":"ui_telemetry","systemPayload":{"uiEvent":{"event.name":"api_response","response_id":"chatcmpl-f2748a8e-85d0-9058-b28f-c70e6f5fd590","model":"qwen-plus"}}}
"#
            .to_vec(),
        };
        mapper.process_filewrite(&event);

        assert_eq!(
            mapper.get_session_by_response_id("chatcmpl-f2748a8e-85d0-9058-b28f-c70e6f5fd590"),
            Some("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        );
    }
}
