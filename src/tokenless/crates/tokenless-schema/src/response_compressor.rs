use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokenless_ccr::{StashStore, StashWrite, marker_for};

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
    /// Number of items preserved from the tail of a truncated array. When
    /// non-zero and the array exceeds `truncate_arrays_at`, the compressor
    /// keeps `head + tail` items with a truncation marker in between, so
    /// ground-truth facts near the end (error codes, final-status entries)
    /// are not silently dropped. When zero, the compressor falls back to
    /// pure head-only truncation.
    array_tail_preserve: usize,
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
    /// Unique stash rows this `compress()` created, plus refreshes of keys
    /// it did not create. An in-compress refresh of a key already pending
    /// rollback does not increment again, so after `rollback_stash_writes`
    /// the counter matches remaining live rows from this call.
    stash_writes: Cell<usize>,
    /// Number of stash writes or rollback deletes that failed for the most
    /// recent `compress()` call. Non-zero signals a persistent backend problem
    /// (disk full, locked DB, I/O) — the caller should log it so the failure
    /// isn't invisible.
    stash_errors: Cell<usize>,
    /// Number of truncations that could not embed a retrievable marker during
    /// the last `compress()` call. Includes backend failures and marker-budget
    /// limits without conflating the two causes.
    unrecoverable_truncations: Cell<usize>,
    /// Total truncation events (string, array, depth) during the last
    /// `compress()` call, counted whether or not a stash store is attached.
    /// Zero means the call removed no content beyond dropped fields/nulls,
    /// which lets callers report the candidate as lossless.
    truncations: Cell<usize>,
    /// Every successful stash write of the last `compress()` call, in order.
    /// The pipeline's ledger consumes these to own rollback end-to-end.
    stash_write_events: RefCell<Vec<StashWrite>>,
    /// Keys created during the last `compress()` call, mapped to the latest
    /// generation this call still owns. An in-compress refresh updates the
    /// generation only when `StashWrite::previous_generation` matches, so a
    /// foreign refresh in between cannot be re-adopted and later deleted.
    stash_keys_created: RefCell<HashMap<String, u64>>,
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
            // Preserve 8 items from the tail of truncated arrays so
            // ground-truth facts near the end (error codes, final-status
            // entries, terminal diff hunks) survive compression. At 32+8=40
            // items the token cost is modest while covering the empirically
            // observed pattern of key information landing in the last
            // quarter of an array.
            array_tail_preserve: 8,
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
            unrecoverable_truncations: Cell::new(0),
            truncations: Cell::new(0),
            stash_write_events: RefCell::new(Vec::new()),
            stash_keys_created: RefCell::new(HashMap::new()),
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

    /// Set how many items from the tail of a truncated array are preserved.
    /// Zero disables tail preservation (pure head-only truncation). Values
    /// large enough to cover the array together with the head limit keep
    /// every item: truncation only drops items beyond the combined budget.
    pub fn with_array_tail_preserve(mut self, n: usize) -> Self {
        self.array_tail_preserve = n;
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
        self.unrecoverable_truncations.set(0);
        self.truncations.set(0);
        self.stash_write_events.borrow_mut().clear();
        self.stash_keys_created.borrow_mut().clear();
        let original_text = serde_json::to_string(response).unwrap_or_default();
        let result = self.compress_value(response, 0);

        // Compare with original to see if anything actually changed
        let compressed_text = serde_json::to_string(&result).unwrap_or_default();
        if original_text == compressed_text {
            return response.clone(); // Return original if no change
        }

        result
    }

    /// Unique stash rows created during the last `compress()` call, plus
    /// refresh operations for keys this call did not create. Duplicate
    /// payloads first created by this call count once; repeated refreshes of
    /// a pre-existing key count once per refresh. Zero when no stash store is
    /// attached or no value was stashed.
    pub fn stash_writes(&self) -> usize {
        self.stash_writes.get()
    }

    /// Number of stash writes or rollback deletes that failed for the most
    /// recent `compress()` call. Non-zero signals a persistent backend problem
    /// (disk full, locked DB, I/O) — the caller should log it so the failure
    /// isn't invisible.
    pub fn stash_errors(&self) -> usize {
        self.stash_errors.get()
    }

    /// Number of truncations that lacked a retrievable marker despite an
    /// attached stash. A non-zero value means the candidate is partly lossy.
    pub fn unrecoverable_truncations(&self) -> usize {
        self.unrecoverable_truncations.get()
    }

    /// Total truncation events (string, array, depth) during the last
    /// `compress()` call, with or without an attached stash. Zero means the
    /// call removed nothing beyond dropped fields, nulls, and empty values.
    pub fn truncations(&self) -> usize {
        self.truncations.get()
    }

    /// Takes every successful stash write of the last `compress()` call, in
    /// order. Callers that hand rollback to the pipeline's ledger consume
    /// these instead of calling [`Self::rollback_stash_writes`]; taking them
    /// does not affect this compressor's own rollback bookkeeping.
    pub fn take_stash_writes(&self) -> Vec<StashWrite> {
        std::mem::take(&mut *self.stash_write_events.borrow_mut())
    }

    /// Delete stash entries created during the last `compress()` call.
    ///
    /// Call this when the compressed output (and its embedded markers) will
    /// never be emitted — e.g. the CLI no-savings path that falls back to the
    /// original input. Returns how many keys were successfully removed.
    pub fn rollback_stash_writes(&self) -> usize {
        let Some(store) = self.stash_store.as_ref() else {
            return 0;
        };
        let writes = std::mem::take(&mut *self.stash_keys_created.borrow_mut());
        let mut removed = 0usize;
        for (key, generation) in &writes {
            match store.delete(key, *generation) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(e) => {
                    // Keep the key list drained so we don't retry forever, but
                    // surface the failure — a silent orphan is worse than a
                    // loud warning (AGENTS.md: do not swallow operational errors).
                    self.record_stash_error();
                    eprintln!("[tokenless] stash: rollback delete failed for key {key}: {e}");
                }
            }
        }
        // Keep counters consistent with the rolled-back store.
        self.stash_writes
            .set(self.stash_writes.get().saturating_sub(removed));
        removed
    }

    fn record_stash_success(&self, write: &StashWrite) {
        self.stash_write_events.borrow_mut().push(write.clone());
        let mut pending = self.stash_keys_created.borrow_mut();
        if write.created {
            self.stash_writes.set(self.stash_writes.get() + 1);
            pending.insert(write.key.clone(), write.generation);
        } else if let Some(&expected) = pending.get(&write.key) {
            if write.previous_generation == Some(expected) {
                // Unbroken in-session refresh: update generation so rollback
                // CAS matches the live row. Do not double-count stash_writes.
                pending.insert(write.key.clone(), write.generation);
            } else {
                // A foreign writer refreshed (and likely emitted a marker)
                // between our create and this write. Drop ownership so
                // rollback cannot delete the live row those markers need.
                pending.remove(&write.key);
            }
        } else {
            // Refresh of a key this compress never created. Another emitted
            // marker may still need it, so it stays off the rollback list,
            // but the write still counts.
            self.stash_writes.set(self.stash_writes.get() + 1);
        }
    }

    fn record_stash_error(&self) {
        self.stash_errors.set(self.stash_errors.get() + 1);
    }

    /// Recursively compress a JSON value
    fn compress_value(&self, value: &Value, depth: usize) -> Value {
        // Check depth limit
        if depth > self.max_depth {
            self.truncations.set(self.truncations.get() + 1);
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
            if self.stash_store.is_some() {
                if let Ok(serialized) = serde_json::to_string(value) {
                    if let Some(key) = self.stash_payload(&serialized) {
                        return Value::String(format!(
                            "<{type_name} truncated at depth {depth}, retrieve with {}>",
                            marker_for(&key)
                        ));
                    }
                } else {
                    self.mark_unrecoverable_truncation();
                }
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
        self.truncations.set(self.truncations.get() + 1);

        const LOSSY_MARKER: &str = "… (truncated)";
        let lossy_marker_len = LOSSY_MARKER.chars().count();

        // Reversible path: a stash store is attached, truncation markers are
        // enabled, and the limit can fit the stash suffix plus at least one
        // content character. Stash the ORIGINAL full string (not the truncated
        // form) so retrieval yields the verbatim original. Fit is checked
        // BEFORE stashing so a too-small limit (or disabled markers) does not
        // orphan a stash entry with no embedded marker — a stash without a
        // reachable marker is unretrievable.
        let reversible_marker_fits =
            self.add_truncation_marker && self.truncate_strings_at > stash_suffix_char_len();
        if reversible_marker_fits {
            if let Some(key) = self.stash_payload(s) {
                let target = self.truncate_strings_at - stash_suffix_char_len();
                let truncate_pos = s
                    .char_indices()
                    .nth(target)
                    .map(|(i, _)| i)
                    .unwrap_or(s.len());
                return Value::String(format!("{}{}", &s[..truncate_pos], stash_suffix(&key)));
            }
        } else if self.stash_store.is_some() {
            self.mark_unrecoverable_truncation();
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
            Value::String(format!("{truncated}{LOSSY_MARKER}"))
        } else {
            Value::String(truncated.to_string())
        }
    }

    /// Compress an array, truncating if necessary.
    ///
    /// When `array_tail_preserve` is non-zero and the array exceeds the
    /// head limit, items from the tail are preserved alongside the head
    /// with a truncation marker in between. This prevents ground-truth
    /// facts near the end of long arrays (error codes, final-status
    /// entries, terminal diff hunks) from being silently dropped.
    fn compress_array(&self, arr: &[Value], depth: usize) -> Value {
        let mut result = Vec::new();
        let head_limit = self.truncate_arrays_at;
        // Truncation drops middle items only when the array exceeds both
        // the head limit AND the combined head+tail budget. The budget uses
        // saturating arithmetic: `array_tail_preserve` reaches the public
        // API/CLI as an unconstrained `usize`, and a saturated budget just
        // means head+tail covers the whole array, so no item is dropped and
        // every index derived below stays within `arr`.
        let head_tail_budget = head_limit.saturating_add(self.array_tail_preserve);
        let truncate = arr.len() > head_limit && arr.len() > head_tail_budget;
        if truncate {
            self.truncations.set(self.truncations.get() + 1);
        }
        // Tail preserves items from the end: the configured count when
        // truncation drops middle items, or the overflow beyond head_limit
        // when head+tail covers the array (no items lost).
        let tail_count = if truncate {
            self.array_tail_preserve
        } else if arr.len() > head_limit {
            arr.len() - head_limit
        } else {
            0
        };
        let head_end = if arr.len() > head_limit {
            head_limit
        } else {
            arr.len()
        };

        // Process head items
        for item in arr.iter().take(head_end) {
            self.push_compressed_if_kept(&mut result, item, depth);
        }

        // Truncation marker sits between head and tail
        if truncate && self.add_truncation_marker {
            let tail_start = arr.len() - tail_count;
            let remaining = tail_start - head_end;
            let dropped = &arr[head_end..tail_start];
            let marker = match self.stash_dropped(dropped) {
                Some(key) => format!(
                    "<... {} items truncated, retrieve with {}>",
                    remaining,
                    marker_for(&key)
                ),
                None => format!("<... {remaining} more items truncated, not stashed>"),
            };
            result.push(Value::String(marker));
        } else if truncate && self.stash_store.is_some() {
            self.mark_unrecoverable_truncation();
        }

        // Process tail items
        for item in arr.iter().skip(arr.len() - tail_count) {
            self.push_compressed_if_kept(&mut result, item, depth);
        }

        Value::Array(result)
    }

    /// Compress and conditionally push a single item, skipping nulls and
    /// empty values when configured. Shared by both the head and tail
    /// passes of [`Self::compress_array`].
    fn push_compressed_if_kept(&self, result: &mut Vec<Value>, item: &Value, depth: usize) {
        let compressed = self.compress_value(item, depth + 1);
        if self.drop_nulls && compressed.is_null() {
            return;
        }
        if self.drop_empty_fields && self.is_empty_value(&compressed) {
            return;
        }
        result.push(compressed);
    }

    /// Stash the dropped tail of a truncated array, returning the stash key.
    /// Returns `None` when no store is attached, when the dropped slice is
    /// empty, or when stashing fails — in all these cases the caller falls
    /// back to the plain (lossy) truncation marker. Stashing the raw dropped
    /// items (not their compressed forms) means retrieval yields the original
    /// content verbatim.
    fn stash_dropped(&self, dropped: &[Value]) -> Option<String> {
        if dropped.is_empty() {
            return None;
        }
        let Ok(payload) = serde_json::to_string(dropped) else {
            // Nothing was stashed, so the truncation is lossy despite the
            // attached store — count it like the depth path does.
            if self.stash_store.is_some() {
                self.mark_unrecoverable_truncation();
            }
            return None;
        };
        if payload.is_empty() {
            return None;
        }
        self.stash_payload(&payload)
    }

    /// Write one payload and keep per-compression observability consistent
    /// across string, array, and depth truncation paths.
    fn stash_payload(&self, payload: &str) -> Option<String> {
        let stash = self.stash_store.as_ref()?;
        match stash.stash(payload) {
            Ok(write) => {
                self.record_stash_success(&write);
                Some(write.key)
            }
            Err(_) => {
                self.record_stash_error();
                self.mark_unrecoverable_truncation();
                None
            }
        }
    }

    fn mark_unrecoverable_truncation(&self) {
        self.unrecoverable_truncations
            .set(self.unrecoverable_truncations.get() + 1);
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
