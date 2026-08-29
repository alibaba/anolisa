//! JSON-domain compression for PostTool results.
//!
//! One call owns the complete JSON decision: tree cleanup and truncation,
//! structured-slot restoration, compact serialization, and optional TOON
//! representation selection. The caller owns final acceptance and Stash
//! commit or rollback.

use std::collections::HashSet;

use serde_json::{Map, Value};
use tokenless_ccr::{
    StashStore, StashWrite, marker_for, truncation_suffix, truncation_suffix_char_len,
};
use tokenless_protocol::estimate_tokens;

/// Configuration for one JSON-domain compressor.
#[derive(Debug, Clone)]
pub struct JsonCompressionConfig {
    /// Maximum string length in Unicode scalar values.
    pub truncate_strings_at: usize,
    /// Number of array items retained from the head.
    pub truncate_arrays_at: usize,
    /// Number of array items retained from the tail.
    pub array_tail_preserve: usize,
    /// Maximum JSON nesting depth before a subtree is replaced.
    pub max_depth: usize,
    /// Removes null-valued object fields and array entries.
    pub drop_nulls: bool,
    /// Removes empty strings, arrays, and objects.
    pub drop_empty_fields: bool,
    /// Emits a bounded marker when truncating content.
    pub add_truncation_marker: bool,
}

impl Default for JsonCompressionConfig {
    fn default() -> Self {
        Self {
            truncate_strings_at: 4096,
            truncate_arrays_at: 32,
            array_tail_preserve: 8,
            drop_nulls: true,
            drop_empty_fields: true,
            max_depth: 8,
            add_truncation_marker: true,
        }
    }
}

/// Per-call facts that affect valid JSON representations.
pub struct JsonCompressionContext<'a> {
    /// Store used to back truncation markers, when retrieval is reachable.
    pub stash: Option<&'a dyn StashStore>,
    /// Whether the host accepts a non-JSON text representation such as TOON.
    pub allow_toon: bool,
    /// Whether empty top-level fields must survive replacement.
    pub preserve_top_level_shape: bool,
    /// Minimum candidate size before TOON is considered.
    pub min_toon_chars: usize,
}

/// Stable operations performed inside the JSON domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonOperation {
    /// Structural cleanup or compact JSON serialization changed the value.
    Cleanup,
    /// One or more values were bounded by string, array, or depth limits.
    Truncation,
    /// TOON was selected as the final representation.
    Toon,
}

impl JsonOperation {
    /// Stable internal operation identifier.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Cleanup => "json-cleanup",
            Self::Truncation => "json-truncation",
            Self::Toon => "json-toon",
        }
    }
}

/// Recovery state of a JSON candidate relative to its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recoverability {
    /// No bounded content was removed.
    Lossless,
    /// Every truncation has a reachable Stash marker.
    Retrievable,
    /// At least one truncation cannot be recovered.
    Unrecoverable,
}

/// Observability produced during one JSON compression attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonMetrics {
    /// Failed Stash writes while producing tentative candidates.
    pub stash_errors: usize,
    /// Truncations present in the selected candidate.
    pub truncations: usize,
    /// Selected truncations without a retrievable marker.
    pub unrecoverable_truncations: usize,
}

/// Complete result of one JSON-domain compression attempt.
#[derive(Debug)]
pub struct JsonOutcome {
    /// Candidate selected inside the JSON domain.
    pub output: String,
    /// Operations that shaped `output`, in execution order.
    pub operations: Vec<JsonOperation>,
    /// Recovery state of `output`.
    pub recoverability: Recoverability,
    /// Every tentative write performed while producing candidates. The
    /// Runtime ledger decides which writes reach the final output.
    pub stash_writes: Vec<StashWrite>,
    /// Metrics associated with the attempt and selected candidate.
    pub metrics: JsonMetrics,
}

/// JSON-domain compression failures.
#[derive(Debug, thiserror::Error)]
pub enum JsonError {
    /// Input was not valid JSON.
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

/// Compresses JSON tool results as one content domain.
#[derive(Debug, Clone, Default)]
pub struct JsonCompressor {
    config: JsonCompressionConfig,
}

impl JsonCompressor {
    /// Builds a compressor with explicit limits.
    #[must_use]
    pub fn new(config: JsonCompressionConfig) -> Self {
        Self { config }
    }

    /// Produces the best valid JSON-domain candidate.
    ///
    /// # Errors
    ///
    /// Returns [`JsonError::InvalidJson`] when the direct input or a detected
    /// JSON string envelope cannot be parsed.
    pub fn compress(
        &self,
        input: &str,
        context: &JsonCompressionContext<'_>,
    ) -> Result<JsonOutcome, JsonError> {
        let (normalized, original) = parse_input(input)?;
        let mut session = Session::new(&self.config, context.stash);
        let transformed = session.compress_value(&original, 0);
        let transformed = if context.preserve_top_level_shape {
            restore_top_level_shape(&original, transformed)
        } else {
            transformed
        };
        let compact = serde_json::to_string(&transformed)?;
        let cleanup_selected = strictly_smaller(&compact, &normalized);

        let mut operations = Vec::new();
        if cleanup_selected
            && (session.cleanup_changes > 0 || (session.truncations == 0 && compact != normalized))
        {
            operations.push(JsonOperation::Cleanup);
        }
        if cleanup_selected && session.truncations > 0 {
            operations.push(JsonOperation::Truncation);
        }

        let (base_text, base_value) = if cleanup_selected {
            (compact.as_str(), &transformed)
        } else {
            (normalized.as_str(), &original)
        };
        let toon = context
            .allow_toon
            .then(|| toon_candidate(base_value, base_text, context.min_toon_chars))
            .flatten();

        let (output, recoverability, truncations, unrecoverable_truncations) =
            if let Some(toon) = toon {
                operations.push(JsonOperation::Toon);
                if cleanup_selected {
                    (
                        toon,
                        session.recoverability(),
                        session.truncations,
                        session.unrecoverable_truncations,
                    )
                } else {
                    (toon, Recoverability::Lossless, 0, 0)
                }
            } else if cleanup_selected {
                (
                    compact,
                    session.recoverability(),
                    session.truncations,
                    session.unrecoverable_truncations,
                )
            } else {
                (normalized, Recoverability::Lossless, 0, 0)
            };

        Ok(JsonOutcome {
            output,
            operations,
            recoverability,
            stash_writes: session.stash_writes,
            metrics: JsonMetrics {
                stash_errors: session.stash_errors,
                truncations,
                unrecoverable_truncations,
            },
        })
    }
}

fn parse_input(input: &str) -> Result<(String, Value), JsonError> {
    let outer: Value = serde_json::from_str(input)?;
    if let Value::String(inner) = &outer
        && let Ok(value @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str(inner)
    {
        return Ok((serde_json::to_string(&value)?, value));
    }
    Ok((input.to_owned(), outer))
}

fn restore_top_level_shape(original: &Value, transformed: Value) -> Value {
    let Value::Object(original) = original else {
        return transformed;
    };
    let mut transformed = match transformed {
        Value::Object(transformed) => transformed,
        other => return other,
    };
    for (key, value) in original {
        if !transformed.contains_key(key) && is_empty_or_null(value) {
            transformed.insert(key.clone(), value.clone());
        }
    }
    Value::Object(transformed)
}

fn toon_candidate(value: &Value, baseline: &str, min_chars: usize) -> Option<String> {
    if baseline.chars().count() < min_chars {
        return None;
    }
    let encoded = toon_format::encode_default(value).ok()?;
    let candidate = encoded.trim_end().to_owned();
    (!candidate.is_empty() && strictly_smaller(&candidate, baseline)).then_some(candidate)
}

fn strictly_smaller(candidate: &str, baseline: &str) -> bool {
    candidate.chars().count() < baseline.chars().count()
        && estimate_tokens(candidate) < estimate_tokens(baseline)
}

fn is_empty(value: &Value) -> bool {
    value.as_str() == Some("")
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(Map::is_empty)
}

fn is_empty_or_null(value: &Value) -> bool {
    value.is_null() || is_empty(value)
}

struct Session<'a> {
    config: &'a JsonCompressionConfig,
    stash: Option<&'a dyn StashStore>,
    drop_fields: HashSet<&'static str>,
    stash_writes: Vec<StashWrite>,
    stash_errors: usize,
    cleanup_changes: usize,
    truncations: usize,
    unrecoverable_truncations: usize,
}

impl<'a> Session<'a> {
    fn new(config: &'a JsonCompressionConfig, stash: Option<&'a dyn StashStore>) -> Self {
        Self {
            config,
            stash,
            drop_fields: HashSet::from([
                "debug",
                "trace",
                "traces",
                "stack",
                "stacktrace",
                "logs",
                "logging",
            ]),
            stash_writes: Vec::new(),
            stash_errors: 0,
            cleanup_changes: 0,
            truncations: 0,
            unrecoverable_truncations: 0,
        }
    }

    fn recoverability(&self) -> Recoverability {
        if self.truncations == 0 {
            Recoverability::Lossless
        } else if self.stash.is_some() && self.unrecoverable_truncations == 0 {
            Recoverability::Retrievable
        } else {
            Recoverability::Unrecoverable
        }
    }

    fn compress_value(&mut self, value: &Value, depth: usize) -> Value {
        if depth > self.config.max_depth {
            self.truncations += 1;
            let type_name = match value {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            if let Ok(serialized) = serde_json::to_string(value)
                && let Some(key) = self.stash_payload(&serialized)
            {
                return Value::String(format!(
                    "<{type_name} truncated at depth {depth}, run: tokenless retrieve '{}'>",
                    marker_for(&key)
                ));
            }
            self.mark_unrecoverable();
            return Value::String(format!("<{type_name} truncated at depth {depth}>"));
        }

        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
            Value::String(value) => self.compress_string(value),
            Value::Array(value) => self.compress_array(value, depth),
            Value::Object(value) => self.compress_object(value, depth),
        }
    }

    fn compress_string(&mut self, value: &str) -> Value {
        if value.chars().count() <= self.config.truncate_strings_at {
            return Value::String(value.to_owned());
        }
        self.truncations += 1;

        let reversible_fits = self.config.add_truncation_marker
            && self.config.truncate_strings_at > truncation_suffix_char_len();
        if reversible_fits && let Some(key) = self.stash_payload(value) {
            let keep = self.config.truncate_strings_at - truncation_suffix_char_len();
            return Value::String(format!(
                "{}{}",
                prefix_chars(value, keep),
                truncation_suffix(&key)
            ));
        }
        self.mark_unrecoverable();

        const MARKER: &str = "… (truncated)";
        let marker_len = MARKER.chars().count();
        let attach_marker =
            self.config.add_truncation_marker && self.config.truncate_strings_at > marker_len;
        let keep = if attach_marker {
            self.config.truncate_strings_at - marker_len
        } else {
            self.config.truncate_strings_at
        };
        let mut output = prefix_chars(value, keep).to_owned();
        if attach_marker {
            output.push_str(MARKER);
        }
        Value::String(output)
    }

    fn compress_array(&mut self, values: &[Value], depth: usize) -> Value {
        let head = self.config.truncate_arrays_at;
        let budget = head.saturating_add(self.config.array_tail_preserve);
        let truncate = values.len() > head && values.len() > budget;
        if truncate {
            self.truncations += 1;
        }
        let tail = if truncate {
            self.config.array_tail_preserve
        } else if values.len() > head {
            values.len() - head
        } else {
            0
        };
        let head_end = values.len().min(head);
        let mut output = Vec::new();
        for value in values.iter().take(head_end) {
            self.push_if_kept(&mut output, value, depth);
        }

        if truncate && self.config.add_truncation_marker {
            let tail_start = values.len() - tail;
            let dropped = &values[head_end..tail_start];
            let marker = if let Some(key) = self.stash_dropped(dropped) {
                format!(
                    "<... {} items truncated, run: tokenless retrieve '{}'>",
                    dropped.len(),
                    marker_for(&key)
                )
            } else {
                self.mark_unrecoverable();
                format!("<... {} more items truncated, not stashed>", dropped.len())
            };
            output.push(Value::String(marker));
        } else if truncate {
            self.mark_unrecoverable();
        }

        for value in values.iter().skip(values.len() - tail) {
            self.push_if_kept(&mut output, value, depth);
        }
        Value::Array(output)
    }

    fn push_if_kept(&mut self, output: &mut Vec<Value>, value: &Value, depth: usize) {
        let compressed = self.compress_value(value, depth + 1);
        if (self.config.drop_nulls && compressed.is_null())
            || (self.config.drop_empty_fields && is_empty(&compressed))
        {
            self.cleanup_changes += 1;
            return;
        }
        output.push(compressed);
    }

    fn compress_object(&mut self, values: &Map<String, Value>, depth: usize) -> Value {
        let mut output = Map::new();
        for (key, value) in values {
            if self.drop_fields.contains(key.as_str()) {
                self.cleanup_changes += 1;
                continue;
            }
            let compressed = self.compress_value(value, depth + 1);
            if (self.config.drop_nulls && compressed.is_null())
                || (self.config.drop_empty_fields && is_empty(&compressed))
            {
                self.cleanup_changes += 1;
                continue;
            }
            output.insert(key.clone(), compressed);
        }
        Value::Object(output)
    }

    fn stash_dropped(&mut self, dropped: &[Value]) -> Option<String> {
        if dropped.is_empty() {
            return None;
        }
        let payload = match serde_json::to_string(dropped) {
            Ok(payload) => payload,
            Err(_) => return None,
        };
        self.stash_payload(&payload)
    }

    fn stash_payload(&mut self, payload: &str) -> Option<String> {
        let stash = self.stash?;
        match stash.stash(payload) {
            Ok(write) => {
                let key = write.key.clone();
                self.stash_writes.push(write);
                Some(key)
            }
            Err(_) => {
                self.stash_errors += 1;
                None
            }
        }
    }

    fn mark_unrecoverable(&mut self) {
        self.unrecoverable_truncations += 1;
    }
}

fn prefix_chars(value: &str, count: usize) -> &str {
    let end = value
        .char_indices()
        .nth(count)
        .map_or(value.len(), |(index, _)| index);
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/json_tests.rs");
}
