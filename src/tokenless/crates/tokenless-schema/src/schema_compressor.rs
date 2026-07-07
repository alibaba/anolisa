use regex::Regex;
use serde_json::Value;
use std::sync::{Arc, LazyLock};
use tokenless_ccr::StashStore;

use crate::response_compressor::{stash_suffix, stash_suffix_char_len};

static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```[\s\S]*?```").unwrap());
static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]+`").unwrap());
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Convert a character count `n` to a byte offset in `s`. Returns `s.len()`
/// when `n` exceeds the number of characters.
fn char_index(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

/// SchemaCompressor compresses OpenAI Function Calling schema
/// by truncating descriptions, removing titles/examples, and applying
/// smart compression to reduce token usage.
pub struct SchemaCompressor {
    func_desc_max_len: usize,
    param_desc_max_len: usize,
    drop_examples: bool,
    drop_titles: bool,
    drop_markdown: bool,
    max_depth: usize,
    /// Optional reversible stash. When present, truncated descriptions are
    /// stashed (verbatim original, including markdown) and a
    /// `<<tokenless:KEY>>` marker is appended so the LLM can retrieve the
    /// full original. When `None`, truncation is lossy — the pre-stash
    /// behavior. Mirrors `ResponseCompressor::stash_store`.
    stash_store: Option<Arc<dyn StashStore>>,
}

impl Default for SchemaCompressor {
    fn default() -> Self {
        Self {
            func_desc_max_len: 256,
            param_desc_max_len: 160,
            drop_examples: true,
            drop_titles: true,
            drop_markdown: true,
            // Bound recursion to keep deeply-nested or pathological schemas
            // (e.g. attacker-crafted ~1000-level JSON) from blowing the stack.
            // Schemas tolerate more depth than runtime responses because
            // OpenAPI/JSON-Schema definitions legitimately stack anyOf /
            // oneOf / allOf branches several layers deep — 8 (the
            // ResponseCompressor default) would truncate real-world tool
            // descriptions. 32 keeps a wide safety margin below the
            // ~1024-frame default stack while leaving real schemas intact.
            max_depth: 32,
            stash_store: None,
        }
    }
}

impl SchemaCompressor {
    /// Create a new SchemaCompressor with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a reversible stash store. When set, truncated descriptions
    /// carry a `<<tokenless:KEY>>` marker and the verbatim original is
    /// stashed for retrieval; when unset (the default), truncation stays
    /// lossy.
    pub fn with_stash_store(mut self, store: Arc<dyn StashStore>) -> Self {
        self.stash_store = Some(store);
        self
    }

    /// Set the maximum length for function-level descriptions
    pub fn with_func_desc_max_len(mut self, len: usize) -> Self {
        self.func_desc_max_len = len;
        self
    }

    /// Set the maximum length for parameter-level descriptions
    pub fn with_param_desc_max_len(mut self, len: usize) -> Self {
        self.param_desc_max_len = len;
        self
    }

    /// Set whether to drop examples from schema
    pub fn with_drop_examples(mut self, drop: bool) -> Self {
        self.drop_examples = drop;
        self
    }

    /// Set whether to drop titles from schema
    pub fn with_drop_titles(mut self, drop: bool) -> Self {
        self.drop_titles = drop;
        self
    }

    /// Set whether to drop markdown formatting from descriptions
    pub fn with_drop_markdown(mut self, drop: bool) -> Self {
        self.drop_markdown = drop;
        self
    }

    /// Set the maximum recursion depth for nested schemas
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Compress an OpenAI Function Calling schema
    pub fn compress(&self, tool: &Value) -> Value {
        let original_text = serde_json::to_string(tool).unwrap_or_default();

        let mut result = tool.clone();

        // Check if this is a function wrapper or direct schema
        if let Some(function) = result.get_mut("function") {
            // Compress function-level description
            if let Some(desc) = function.get("description").and_then(|d| d.as_str()) {
                let compressed = self.truncate_description(desc, self.func_desc_max_len);
                function["description"] = Value::String(compressed);
            }

            // Optionally remove title
            #[allow(clippy::collapsible_if)]
            if self.drop_titles {
                if let Some(obj) = function.as_object_mut() {
                    obj.remove("title");
                }
            }

            // Compress parameters
            if let Some(params) = function.get_mut("parameters") {
                self.compress_json_schema(params, 1);
            }
        } else {
            // Direct schema (no function wrapper)
            // Compress top-level description
            if let Some(desc) = result.get("description").and_then(|d| d.as_str()) {
                let compressed = self.truncate_description(desc, self.func_desc_max_len);
                result["description"] = Value::String(compressed);
            }

            // Optionally remove title
            #[allow(clippy::collapsible_if)]
            if self.drop_titles {
                if let Some(obj) = result.as_object_mut() {
                    obj.remove("title");
                }
            }

            // Compress parameters if present
            if let Some(params) = result.get_mut("parameters") {
                self.compress_json_schema(params, 1);
            }

            // If this looks like a JSON Schema itself, compress it recursively
            if result.get("type").is_some() || result.get("properties").is_some() {
                self.compress_json_schema(&mut result, 0);
            }
        }

        // Compare with original to see if anything actually changed
        let compressed_text = serde_json::to_string(&result).unwrap_or_default();
        if original_text == compressed_text {
            return tool.clone(); // Return original if no change
        }

        result
    }

    /// Recursively compress a JSON Schema
    pub fn compress_json_schema(&self, schema: &mut Value, depth: usize) {
        // Stack-overflow guard for pathological schemas. Beyond max_depth we
        // stop descending — the deepest nodes keep their original shape, which
        // is acceptable since this path is best-effort token reduction.
        // Use `>` (not `>=`) so the threshold matches response_compressor.rs
        // semantics: a node at depth==max_depth is still processed, only its
        // grandchildren (depth+1 > max_depth) are skipped.
        if depth > self.max_depth {
            return;
        }

        let Some(obj) = schema.as_object_mut() else {
            return;
        };

        // Remove title if configured
        if self.drop_titles {
            obj.remove("title");
        }

        // Remove examples if configured
        if self.drop_examples {
            obj.remove("examples");
        }

        // Compress description
        if let Some(desc) = obj
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string())
        {
            let max_len = if depth == 0 {
                self.func_desc_max_len
            } else {
                self.param_desc_max_len
            };
            let compressed = self.truncate_description(&desc, max_len);
            obj.insert("description".to_string(), Value::String(compressed));
        }

        // Recursively compress properties (for object types)
        #[allow(clippy::collapsible_if)]
        if let Some(properties) = obj.get_mut("properties") {
            if let Some(props_obj) = properties.as_object_mut() {
                for (_key, prop_schema) in props_obj.iter_mut() {
                    self.compress_json_schema(prop_schema, depth + 1);
                }
            }
        }

        // Recursively compress items (for array types)
        if let Some(items) = obj.get_mut("items") {
            self.compress_json_schema(items, depth + 1);
        }

        // Handle anyOf
        #[allow(clippy::collapsible_if)]
        if let Some(any_of) = obj.get_mut("anyOf") {
            if let Some(arr) = any_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }

        // Handle oneOf
        #[allow(clippy::collapsible_if)]
        if let Some(one_of) = obj.get_mut("oneOf") {
            if let Some(arr) = one_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }

        // Handle allOf
        #[allow(clippy::collapsible_if)]
        if let Some(all_of) = obj.get_mut("allOf") {
            if let Some(arr) = all_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }
    }

    /// Intelligently truncate a description string. When a stash store is
    /// attached, the verbatim original `desc` (including markdown, before
    /// stripping) is stashed and a `<<tokenless:KEY>>` marker is appended so
    /// the LLM can retrieve the full original; the stash suffix length is
    /// reserved from `max_len` so the result still honors the limit. On stash
    /// failure the suffix is dropped (lossy truncation, the pre-stash
    /// behavior).
    pub fn truncate_description(&self, desc: &str, max_len: usize) -> String {
        // Trim whitespace
        let mut text = desc.trim().to_string();

        if self.drop_markdown {
            text = CODE_BLOCK_RE.replace_all(&text, "").to_string();
            text = INLINE_CODE_RE.replace_all(&text, "").to_string();
        }

        text = WHITESPACE_RE.replace_all(&text, " ").to_string();
        text = text.trim().to_string();

        // When stash is attached, reserve room for the retrievable suffix so
        // the final string still fits `max_len`. Fit is checked before any
        // stash call so a too-small `max_len` cannot orphan a stash entry
        // whose marker never reaches the LLM.
        let stash_active = self.stash_store.is_some() && max_len > stash_suffix_char_len();
        let effective_max = if stash_active {
            max_len - stash_suffix_char_len()
        } else {
            max_len
        };

        // If already within limit, return as-is (use char count, not byte length).
        // No truncation → nothing stashed (markdown stripping's loss is
        // pre-existing behavior, out of scope for the reversible-truncation path).
        if text.chars().count() <= effective_max {
            return text;
        }

        // Truncation will happen. Stash the ORIGINAL desc (verbatim, with
        // markdown) so retrieval yields the unredacted original — matching
        // ResponseCompressor's "retrieval yields the original content
        // verbatim" contract.
        let stash_key = if stash_active {
            self.stash_store
                .as_ref()
                .and_then(|store| store.stash(desc).ok())
        } else {
            None
        };

        // Try to find a sentence boundary in the range [effective_max*0.5,
        // effective_max]. Convert char counts to byte positions via
        // char_index so the search range and hard-truncation fallback use
        // correct byte offsets even for multi-byte text (CJK, emoji, etc.).
        let min_target = (effective_max as f64 * 0.5) as usize;
        let min_pos = char_index(&text, min_target);
        let max_pos = char_index(&text, effective_max.min(text.chars().count()));
        let search_range = &text[min_pos..max_pos];

        // Look for sentence endings: . 。 ！ ？
        let sentence_endings = ['.', '。', '！', '？'];
        let mut best_pos = None;

        for (i, c) in search_range.char_indices() {
            if sentence_endings.contains(&c) {
                // Position after the sentence ending
                best_pos = Some(min_pos + i + c.len_utf8());
            }
        }

        let truncated = if let Some(pos) = best_pos {
            text[..pos].trim().to_string()
        } else {
            // No sentence boundary found, hard truncate at effective_max.
            let truncate_pos = char_index(&text, effective_max);
            text[..truncate_pos].trim().to_string()
        };

        match stash_key {
            Some(key) => format!("{}{}", truncated, stash_suffix(&key)),
            None => truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/schema_compressor_tests.rs");
}
