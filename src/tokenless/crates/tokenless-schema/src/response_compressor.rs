use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;
use tokenless_ccr::{StashStore, marker_for};

/// ResponseCompressor compresses API responses by truncating strings,
/// limiting array sizes, removing null values, and dropping debug fields.
pub struct ResponseCompressor {
    drop_fields: HashSet<String>,
    truncate_strings_at: usize,
    truncate_arrays_at: usize,
    drop_nulls: bool,
    drop_empty_fields: bool,
    max_depth: usize,
    add_truncation_marker: bool,
    /// Optional reversible stash. When present, array items dropped by
    /// truncation are stashed under a BLAKE3 key and a `<<tokenless:KEY>>`
    /// marker is embedded in the output so the LLM can retrieve the originals.
    /// When `None`, truncation is lossy and non-retrievable — the original
    /// pre-stash behavior. Keeping this optional means the stash stays off
    /// the core compression path unless a caller explicitly enables it.
    stash_store: Option<Arc<dyn StashStore>>,
}

impl Default for ResponseCompressor {
    fn default() -> Self {
        let mut drop_fields = HashSet::new();
        drop_fields.insert("debug".to_string());
        drop_fields.insert("trace".to_string());
        drop_fields.insert("traces".to_string());
        drop_fields.insert("stack".to_string());
        drop_fields.insert("stacktrace".to_string());
        drop_fields.insert("logs".to_string());
        drop_fields.insert("logging".to_string());

        Self {
            drop_fields,
            truncate_strings_at: 4096,
            truncate_arrays_at: 32,
            drop_nulls: true,
            drop_empty_fields: true,
            // Runtime responses rarely nest beyond a handful of levels in
            // practice, so 8 trades aggressive token savings (collapsing
            // deeply-nested structures to a `<...truncated...>` marker) for
            // a tiny risk of losing useful detail. SchemaCompressor defaults
            // to 32 because schema definitions stack anyOf/oneOf/allOf
            // branches that legitimately need the extra depth — see
            // `SchemaCompressor::default()`.
            max_depth: 8,
            add_truncation_marker: true,
            stash_store: None,
        }
    }
}

impl ResponseCompressor {
    /// Create a new ResponseCompressor with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum string length before truncation
    pub fn with_truncate_strings_at(mut self, len: usize) -> Self {
        self.truncate_strings_at = len;
        self
    }

    /// Set the maximum array length before truncation
    pub fn with_truncate_arrays_at(mut self, len: usize) -> Self {
        self.truncate_arrays_at = len;
        self
    }

    /// Set whether to drop null values
    pub fn with_drop_nulls(mut self, drop: bool) -> Self {
        self.drop_nulls = drop;
        self
    }

    /// Set whether to drop empty fields ({}, [], "")
    pub fn with_drop_empty_fields(mut self, drop: bool) -> Self {
        self.drop_empty_fields = drop;
        self
    }

    /// Set the maximum depth before truncation
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set whether to add truncation markers
    pub fn with_add_truncation_marker(mut self, add: bool) -> Self {
        self.add_truncation_marker = add;
        self
    }

    /// Attach a reversible stash store. When set, dropped array items are
    /// stashed and a retrievable marker is embedded in the output; when
    /// unset (the default), truncation stays lossy.
    pub fn with_stash_store(mut self, store: Arc<dyn StashStore>) -> Self {
        self.stash_store = Some(store);
        self
    }

    /// Add a field name to the drop list
    pub fn add_drop_field(&mut self, field: &str) {
        self.drop_fields.insert(field.to_string());
    }

    /// Compress a JSON response value
    pub fn compress(&self, response: &Value) -> Value {
        let original_text = serde_json::to_string(response).unwrap_or_default();
        let result = self.compress_value(response, 0);

        // Compare with original to see if anything actually changed
        let compressed_text = serde_json::to_string(&result).unwrap_or_default();
        if original_text == compressed_text {
            return response.clone(); // Return original if no change
        }

        result
    }

    /// Recursively compress a JSON value
    fn compress_value(&self, value: &Value, depth: usize) -> Value {
        // Check depth limit
        if depth > self.max_depth {
            let type_name = match value {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            return Value::String(format!("<{} truncated at depth {}>", type_name, depth));
        }

        match value {
            Value::Null => Value::Null,

            Value::Bool(b) => Value::Bool(*b),

            Value::Number(n) => Value::Number(n.clone()),

            Value::String(s) => self.compress_string(s),

            Value::Array(arr) => self.compress_array(arr, depth),

            Value::Object(obj) => self.compress_object(obj, depth),
        }
    }

    /// Compress a string value, truncating if necessary.
    /// When a truncation marker is added, the marker length is reserved so the
    /// final output stays within `truncate_strings_at` characters. If the
    /// configured limit is too small to fit both the marker and a content
    /// character, the marker is dropped so the output never exceeds the limit.
    fn compress_string(&self, s: &str) -> Value {
        let char_count = s.chars().count();
        if char_count <= self.truncate_strings_at {
            return Value::String(s.to_string());
        }

        const MARKER: &str = "… (truncated)";
        let marker_len = MARKER.chars().count();
        // Only attach the marker when the limit can fit it plus at least one
        // content character; otherwise dropping the marker is the only way to
        // honor truncate_strings_at.
        let attach_marker = self.add_truncation_marker && self.truncate_strings_at > marker_len;
        let target = if attach_marker {
            self.truncate_strings_at - marker_len
        } else {
            self.truncate_strings_at
        };

        let truncate_pos = s
            .char_indices()
            .nth(target)
            .map(|(i, _)| i)
            .unwrap_or(s.len());

        let truncated = &s[..truncate_pos];

        if attach_marker {
            Value::String(format!("{}{}", truncated, MARKER))
        } else {
            Value::String(truncated.to_string())
        }
    }

    /// Compress an array, truncating if necessary
    fn compress_array(&self, arr: &[Value], depth: usize) -> Value {
        let mut result = Vec::new();
        let truncate = arr.len() > self.truncate_arrays_at;
        let limit = if truncate {
            self.truncate_arrays_at
        } else {
            arr.len()
        };

        for item in arr.iter().take(limit) {
            let compressed = self.compress_value(item, depth + 1);

            // Skip null values if configured
            if self.drop_nulls && compressed.is_null() {
                continue;
            }

            // Skip empty values if configured
            if self.drop_empty_fields && self.is_empty_value(&compressed) {
                continue;
            }

            result.push(compressed);
        }

        // Add truncation marker if array was truncated
        if truncate && self.add_truncation_marker {
            let remaining = arr.len() - self.truncate_arrays_at;
            // NOTE: the dropped slice is captured BEFORE compress_value runs,
            // so stashed items preserve fields the compressor would otherwise
            // strip (drop_fields like `debug`/`stacktrace`, nulls, depth
            // limits). This is intentional — retrieval must yield the
            // original content verbatim — but it means drop_fields serves no
            // data-hygiene purpose for stashed content; if a field must not
            // survive in the stash DB, strip it upstream of the compressor.
            let dropped = &arr[self.truncate_arrays_at..];
            let marker = match self.stash_dropped(dropped) {
                Some(key) => format!(
                    "<... {} items truncated, retrieve with {}>",
                    remaining,
                    marker_for(&key)
                ),
                None => format!("<... {} more items truncated>", remaining),
            };
            result.push(Value::String(marker));
        }

        Value::Array(result)
    }

    /// Stash the dropped tail of a truncated array, returning the stash key.
    /// Returns `None` when no store is attached, when the dropped slice is
    /// empty, or when stashing fails — in all these cases the caller falls
    /// back to the plain (lossy) truncation marker. Stashing the raw dropped
    /// items (not their compressed forms) means retrieval yields the original
    /// content verbatim.
    fn stash_dropped(&self, dropped: &[Value]) -> Option<String> {
        let stash = self.stash_store.as_ref()?;
        if dropped.is_empty() {
            return None;
        }
        let payload = serde_json::to_string(dropped).ok()?;
        if payload.is_empty() {
            return None;
        }
        stash.stash(&payload).ok()
    }

    /// Compress an object, removing drop_fields and recursing
    fn compress_object(&self, obj: &Map<String, Value>, depth: usize) -> Value {
        let mut result = Map::new();

        for (key, value) in obj {
            // Skip fields in drop_fields
            if self.drop_fields.contains(key) {
                continue;
            }

            let compressed = self.compress_value(value, depth + 1);

            // Skip null values if configured
            if self.drop_nulls && compressed.is_null() {
                continue;
            }

            // Skip empty values if configured
            if self.drop_empty_fields && self.is_empty_value(&compressed) {
                continue;
            }

            result.insert(key.clone(), compressed);
        }

        Value::Object(result)
    }

    /// Check if a value is considered "empty"
    fn is_empty_value(&self, value: &Value) -> bool {
        match value {
            Value::String(s) => s.is_empty(),
            Value::Array(arr) => arr.is_empty(),
            Value::Object(obj) => obj.is_empty(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_string_truncation() {
        let compressor = ResponseCompressor::new().with_truncate_strings_at(20);

        let long_string = "This is a very long string that should be truncated";
        let result = compressor.compress(&json!(long_string));

        let s = result.as_str().unwrap();
        assert!(s.contains("… (truncated)"));
        assert!(s.len() < long_string.len() + 20); // Accounting for marker
    }

    #[test]
    fn test_string_truncation_4096_default() {
        let compressor = ResponseCompressor::new();

        let long_string = "x".repeat(5000);
        let result = compressor.compress(&json!(long_string));

        let s = result.as_str().unwrap();
        assert!(s.contains("… (truncated)"));
    }

    #[test]
    fn test_array_truncation() {
        let compressor = ResponseCompressor::new().with_truncate_arrays_at(3);

        let arr: Vec<i32> = (1..=10).collect();
        let result = compressor.compress(&json!(arr));

        let arr_result = result.as_array().unwrap();
        // 3 items + 1 truncation marker = 4
        assert_eq!(arr_result.len(), 4);
        assert!(arr_result[3].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn test_array_truncation_32_default() {
        let compressor = ResponseCompressor::new();

        let arr: Vec<i32> = (1..=50).collect();
        let result = compressor.compress(&json!(arr));

        let arr_result = result.as_array().unwrap();
        // 32 items + 1 truncation marker = 33
        assert_eq!(arr_result.len(), 33);
    }

    #[test]
    fn test_drop_fields() {
        let compressor = ResponseCompressor::new();

        let obj = json!({
            "data": "important",
            "debug": "should be removed",
            "trace": "should be removed",
            "traces": "should be removed",
            "stack": "should be removed",
            "stacktrace": "should be removed",
            "logs": "should be removed",
            "logging": "should be removed"
        });

        let result = compressor.compress(&obj);
        let obj_result = result.as_object().unwrap();

        assert!(obj_result.contains_key("data"));
        assert!(!obj_result.contains_key("debug"));
        assert!(!obj_result.contains_key("trace"));
        assert!(!obj_result.contains_key("traces"));
        assert!(!obj_result.contains_key("stack"));
        assert!(!obj_result.contains_key("stacktrace"));
        assert!(!obj_result.contains_key("logs"));
        assert!(!obj_result.contains_key("logging"));
    }

    #[test]
    fn test_drop_nulls() {
        let compressor = ResponseCompressor::new();

        let obj = json!({
            "name": "test",
            "value": null,
            "count": 5
        });

        let result = compressor.compress(&obj);
        let obj_result = result.as_object().unwrap();

        assert!(obj_result.contains_key("name"));
        assert!(obj_result.contains_key("count"));
        assert!(!obj_result.contains_key("value"));
    }

    #[test]
    fn test_drop_nulls_disabled() {
        let compressor = ResponseCompressor::new().with_drop_nulls(false);

        let obj = json!({
            "name": "test",
            "value": null
        });

        let result = compressor.compress(&obj);
        let obj_result = result.as_object().unwrap();

        assert!(obj_result.contains_key("value"));
    }

    #[test]
    fn test_drop_empty_fields() {
        let compressor = ResponseCompressor::new();

        let obj = json!({
            "name": "test",
            "empty_string": "",
            "empty_array": [],
            "empty_object": {},
            "valid": "data"
        });

        let result = compressor.compress(&obj);
        let obj_result = result.as_object().unwrap();

        assert!(obj_result.contains_key("name"));
        assert!(obj_result.contains_key("valid"));
        assert!(!obj_result.contains_key("empty_string"));
        assert!(!obj_result.contains_key("empty_array"));
        assert!(!obj_result.contains_key("empty_object"));
    }

    #[test]
    fn test_drop_empty_fields_disabled() {
        let compressor = ResponseCompressor::new().with_drop_empty_fields(false);

        let obj = json!({
            "empty_string": "",
            "empty_array": [],
            "empty_object": {}
        });

        let result = compressor.compress(&obj);
        let obj_result = result.as_object().unwrap();

        assert!(obj_result.contains_key("empty_string"));
        assert!(obj_result.contains_key("empty_array"));
        assert!(obj_result.contains_key("empty_object"));
    }

    #[test]
    fn test_max_depth_truncation() {
        let compressor = ResponseCompressor::new().with_max_depth(2);

        let deep = json!({
            "level1": {
                "level2": {
                    "level3": {
                        "level4": "deep value"
                    }
                }
            }
        });

        let result = compressor.compress(&deep);

        // At depth 3, we should see truncation
        let level3 = &result["level1"]["level2"]["level3"];
        assert!(level3.as_str().unwrap().contains("truncated at depth"));
    }

    #[test]
    fn test_nested_object_recursive_compression() {
        let compressor = ResponseCompressor::new()
            .with_truncate_strings_at(20)
            .with_drop_nulls(true);

        let nested = json!({
            "outer": {
                "inner": {
                    "long_text": "This is a very long text that should be truncated",
                    "null_field": null,
                    "number": 42
                }
            }
        });

        let result = compressor.compress(&nested);

        // Check nested string truncation
        let inner_text = result["outer"]["inner"]["long_text"].as_str().unwrap();
        assert!(inner_text.contains("truncated"));

        // Check nested null removal
        assert!(result["outer"]["inner"].get("null_field").is_none());

        // Check number preserved
        assert_eq!(result["outer"]["inner"]["number"], 42);
    }

    #[test]
    fn test_array_with_objects() {
        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(2)
            .with_drop_nulls(true);

        let arr = json!([
            {"id": 1, "debug": "remove", "value": null},
            {"id": 2},
            {"id": 3},
            {"id": 4}
        ]);

        let result = compressor.compress(&arr);
        let arr_result = result.as_array().unwrap();

        // 2 items + truncation marker
        assert_eq!(arr_result.len(), 3);

        // First item should have debug and null removed
        assert!(!arr_result[0].as_object().unwrap().contains_key("debug"));
        assert!(!arr_result[0].as_object().unwrap().contains_key("value"));
    }

    #[test]
    fn test_preserve_primitives() {
        let compressor = ResponseCompressor::new();

        assert_eq!(compressor.compress(&json!(true)), json!(true));
        assert_eq!(compressor.compress(&json!(false)), json!(false));
        assert_eq!(compressor.compress(&json!(42)), json!(42));
        assert_eq!(compressor.compress(&json!(42.5)), json!(42.5));
        assert_eq!(compressor.compress(&json!("short")), json!("short"));
    }

    #[test]
    fn test_utf8_safe_truncation() {
        let compressor = ResponseCompressor::new().with_truncate_strings_at(10);

        // String with multi-byte UTF-8 characters
        let text = "你好世界，这是测试";
        let result = compressor.compress(&json!(text));

        // Should not panic and should be valid UTF-8
        let s = result.as_str().unwrap();
        assert!(!s.is_empty());
    }

    #[test]
    fn test_array_truncation_without_stash_is_lossy() {
        // No stash attached: original lossy marker, no retrievable hash.
        let compressor = ResponseCompressor::new().with_truncate_arrays_at(3);
        let arr: Vec<i32> = (1..=10).collect();
        let result = compressor.compress(&json!(arr));
        let arr_result = result.as_array().unwrap();
        // 3 kept items + 1 marker
        assert_eq!(arr_result.len(), 4);
        assert_eq!(arr_result[0], json!(1));
        assert_eq!(arr_result[1], json!(2));
        assert_eq!(arr_result[2], json!(3));
        let marker = arr_result[3].as_str().unwrap();
        assert!(marker.contains("more items truncated"));
        assert!(marker.contains("7")); // 10 - 3 dropped
        assert!(!marker.contains("tokenless:"));
    }

    #[test]
    fn test_array_truncation_with_stash_round_trip() {
        use std::sync::Arc;
        use tokenless_ccr::{InMemoryStore, StashStore, extract_hash};

        let store = Arc::new(InMemoryStore::new());
        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(3)
            .with_stash_store(store.clone());
        let arr: Vec<i32> = (1..=10).collect();
        let result = compressor.compress(&json!(arr));
        let arr_result = result.as_array().unwrap();
        // 3 kept items + 1 marker
        assert_eq!(arr_result.len(), 4);
        // Kept items are the first 3 (off-by-one in the slice would break this).
        assert_eq!(arr_result[0], json!(1));
        assert_eq!(arr_result[1], json!(2));
        assert_eq!(arr_result[2], json!(3));
        let marker = arr_result[3].as_str().unwrap();
        assert!(marker.contains("retrieve with"));
        let hash = extract_hash(marker).expect("marker should embed a hash");

        // Retrieved payload is the JSON array of the dropped items [4..=10].
        let retrieved = store.retrieve(hash).unwrap().expect("must be retrievable");
        let recovered: Vec<i32> = serde_json::from_str(&retrieved).unwrap();
        assert_eq!(recovered, (4..=10).collect::<Vec<_>>());
    }

    #[test]
    fn test_array_truncation_with_failing_stash_falls_back_to_lossy() {
        // A stash that always errors must not break compression: the marker
        // degrades to the plain lossy form.
        use std::sync::Arc;
        use tokenless_ccr::{StashError, StashStore};

        struct AlwaysFail;
        impl StashStore for AlwaysFail {
            fn stash(&self, _payload: &str) -> Result<String, StashError> {
                Err(StashError::Backend("simulated".to_string()))
            }
            fn retrieve(&self, _hash: &str) -> Result<Option<String>, StashError> {
                Ok(None)
            }
            fn len(&self) -> usize {
                0
            }
            fn evict_expired(&self) -> Result<usize, StashError> {
                Ok(0)
            }
        }

        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(3)
            .with_stash_store(Arc::new(AlwaysFail));
        let arr: Vec<i32> = (1..=10).collect();
        let result = compressor.compress(&json!(arr));
        let marker = result.as_array().unwrap().last().unwrap();
        let s = marker.as_str().unwrap();
        assert!(s.contains("more items truncated"));
        assert!(!s.contains("tokenless:"));
    }

    #[test]
    fn test_stash_round_trip_with_cjk_items() {
        // CJK payloads are multi-byte; the stashed JSON must round-trip
        // byte-for-byte (review §12: char vs byte semantics).
        use std::sync::Arc;
        use tokenless_ccr::{InMemoryStore, StashStore, extract_hash};

        let store = Arc::new(InMemoryStore::new());
        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(2)
            .with_stash_store(store.clone());
        let arr = json!(["你好世界", "第二个条目", "第三个条目", "第四个条目"]);
        let result = compressor.compress(&arr);
        let arr_result = result.as_array().unwrap();
        // Kept items are the first 2.
        assert_eq!(arr_result[0], json!("你好世界"));
        assert_eq!(arr_result[1], json!("第二个条目"));
        let marker = arr_result.last().unwrap();
        let hash = extract_hash(marker.as_str().unwrap()).unwrap();
        let retrieved = store.retrieve(hash).unwrap().unwrap();
        let recovered: Vec<String> = serde_json::from_str(&retrieved).unwrap();
        assert_eq!(recovered, vec!["第三个条目", "第四个条目"]);
    }

    #[test]
    fn test_stash_round_trip_with_object_array() {
        // The "100 normal + 2 error" case: dropped object items must be
        // recoverable verbatim, including fields the compressor would
        // otherwise strip (debug/trace). The kept item carries a `debug`
        // field too, so the test can prove kept items ARE compressed
        // (debug stripped) while stashed items are raw (debug preserved).
        use std::sync::Arc;
        use tokenless_ccr::{InMemoryStore, StashStore, extract_hash};

        let store = Arc::new(InMemoryStore::new());
        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(1)
            .with_stash_store(store.clone());
        let arr = json!([
            {"id": 1, "status": "ok", "debug": "should be stripped"},
            {"id": 2, "status": "error", "debug": "trace data"},
            {"id": 3, "status": "ok"}
        ]);
        let result = compressor.compress(&arr);
        let arr_result = result.as_array().unwrap();
        // Kept item is compressed: debug stripped.
        assert_eq!(arr_result[0]["id"], json!(1));
        assert!(
            arr_result[0].get("debug").is_none(),
            "kept items must be compressed (debug stripped)"
        );
        let marker = arr_result.last().unwrap();
        let hash = extract_hash(marker.as_str().unwrap()).unwrap();
        let retrieved = store.retrieve(hash).unwrap().unwrap();
        let recovered: Vec<Value> = serde_json::from_str(&retrieved).unwrap();
        // Stashed items are raw (pre-compression): debug survives.
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0]["debug"], json!("trace data"));
    }

    #[test]
    fn test_stash_not_engaged_when_array_within_limit() {
        // No truncation → no stash write → no marker. Stash stays empty.
        use std::sync::Arc;
        use tokenless_ccr::InMemoryStore;

        let store = Arc::new(InMemoryStore::new());
        let compressor = ResponseCompressor::new()
            .with_truncate_arrays_at(10)
            .with_stash_store(store.clone());
        let arr: Vec<i32> = (1..=5).collect();
        let result = compressor.compress(&json!(arr));
        // No truncation marker at all.
        assert!(result.as_array().unwrap().iter().all(|v| v.is_number()));
        assert_eq!(store.len(), 0);
    }
}
