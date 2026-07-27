use serde_json::{Map, Value};
use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;
use tokenless_ccr::{StashStore, marker_for};

/// Build the stash-augmented truncation suffix for `key`:
/// `… (truncated, retrieve with <<tokenless:KEY>>)`.
pub(crate) fn stash_suffix(key: &str) -> String {
    format!("… (truncated, retrieve with {})", marker_for(key))
}

/// Char-length of [`stash_suffix`]. Constant because the key is always 24
/// hex chars, so the budget for the suffix can be reserved before stashing.
/// Derived from [`stash_suffix`] so the two cannot drift out of sync.
/// Shared with `schema_compressor` for description truncation.
pub(crate) fn stash_suffix_char_len() -> usize {
    // 24-char stand-in; marker_for is `<<tokenless:` + key + `>>`.
    stash_suffix("000000000000000000000000").chars().count()
}

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
    /// Number of stash writes performed during the last `compress()` call.
    /// Exposed for stats recording so callers can observe stash usage without
    /// the schema crate depending on the stats crate.
    stash_writes: Cell<usize>,
    /// Number of stash writes that **failed** during the last `compress()`
    /// call (backend error — disk full, locked DB, I/O). Exposed so the CLI
    /// can surface persistent backend failures instead of silently degrading
    /// to the lossy marker.
    stash_errors: Cell<usize>,
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
            stash_writes: Cell::new(0),
            stash_errors: Cell::new(0),
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
        // Reset the stash counters so they reflect this call only. Cell (not
        // AtomicUsize) is the right primitive: ResponseCompressor is
        // stack-allocated per compress call and never shared across threads,
        // and Cell makes the struct !Sync — preventing the false thread-safety
        // impression that a reset-then-increment AtomicUsize pattern would give.
        self.stash_writes.set(0);
        self.stash_errors.set(0);
        let original_text = serde_json::to_string(response).unwrap_or_default();
        let result = self.compress_value(response, 0);

        // Compare with original to see if anything actually changed
        let compressed_text = serde_json::to_string(&result).unwrap_or_default();
        if original_text == compressed_text {
            return response.clone(); // Return original if no change
        }

        result
    }

    /// Number of stash writes performed during the last `compress()` call.
    /// Zero when no stash store is attached or no arrays were truncated.
    pub fn stash_writes(&self) -> usize {
        self.stash_writes.get()
    }

    /// Number of stash writes that failed during the last `compress()` call.
    /// Non-zero signals a persistent backend problem (disk full, locked DB,
    /// I/O) — the caller should log it so the failure isn't invisible.
    pub fn stash_errors(&self) -> usize {
        self.stash_errors.get()
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
            // Try to stash the original subtree so the LLM can retrieve the
            // verbatim original via the embedded marker. On any failure (no
            // store, serialization error, stash backend error) fall back to
            // the plain lossy depth marker.
            if let Some(store) = self.stash_store.as_ref()
                && let Ok(serialized) = serde_json::to_string(value)
                && let Ok(key) = store.stash(&serialized)
            {
                return Value::String(format!(
                    "<{type_name} truncated at depth {depth}, retrieve with {}>",
                    marker_for(&key)
                ));
            }
            return Value::String(format!("<{type_name} truncated at depth {depth}>"));
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
    ///
    /// When a stash store is attached, the suffix carries a `<<tokenless:KEY>>`
    /// marker and the ORIGINAL full string is stashed so truncation is
    /// reversible. On stash failure the suffix degrades to the plain lossy
    /// `… (truncated)` marker (or hard truncation if even that won't fit).
    fn compress_string(&self, s: &str) -> Value {
        let char_count = s.chars().count();
        if char_count <= self.truncate_strings_at {
            return Value::String(s.to_string());
        }

        const LOSSY_MARKER: &str = "… (truncated)";
        let lossy_marker_len = LOSSY_MARKER.chars().count();

        // Reversible path: a stash store is attached, truncation markers are
        // enabled, and the limit can fit the stash suffix plus at least one
        // content character. Stash the ORIGINAL full string (not the truncated
        // form) so retrieval yields the verbatim original. Fit is checked
        // BEFORE stashing so a too-small limit (or disabled markers) does not
        // orphan a stash entry with no embedded marker — a stash without a
        // reachable marker is unretrievable.
        if self.add_truncation_marker
            && self.truncate_strings_at > stash_suffix_char_len()
            && let Some(store) = self.stash_store.as_ref()
            && let Ok(key) = store.stash(s)
        {
            let target = self.truncate_strings_at - stash_suffix_char_len();
            let truncate_pos = s
                .char_indices()
                .nth(target)
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            return Value::String(format!("{}{}", &s[..truncate_pos], stash_suffix(&key)));
        }

        // Lossy path: existing behavior. Only attach the marker when the
        // limit can fit it plus at least one content character; otherwise
        // dropping the marker is the only way to honor truncate_strings_at.
        let attach_marker =
            self.add_truncation_marker && self.truncate_strings_at > lossy_marker_len;
        let target = if attach_marker {
            self.truncate_strings_at - lossy_marker_len
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
            Value::String(format!("{}{}", truncated, LOSSY_MARKER))
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
        let key = match stash.stash(&payload) {
            Ok(k) => {
                self.stash_writes.set(self.stash_writes.get() + 1);
                k
            }
            Err(_) => {
                // Surface the backend failure via the counter so the CLI can
                // log it; degrade to the lossy marker for this entry.
                self.stash_errors.set(self.stash_errors.get() + 1);
                return None;
            }
        };
        Some(key)
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
    include!("tests/response_compressor_tests.rs");
}
