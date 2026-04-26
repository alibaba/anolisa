use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;

/// SchemaCompressor compresses OpenAI Function Calling schema
/// by truncating descriptions, removing titles/examples, and applying
/// smart compression to reduce token usage.
pub struct SchemaCompressor {
    #[allow(dead_code)]
    protected_fields: HashSet<&'static str>,
    func_desc_max_len: usize,
    param_desc_max_len: usize,
    drop_examples: bool,
    drop_titles: bool,
    drop_markdown: bool,
}

impl Default for SchemaCompressor {
    fn default() -> Self {
        let mut protected_fields = HashSet::new();
        protected_fields.insert("name");
        protected_fields.insert("type");
        protected_fields.insert("required");
        protected_fields.insert("enum");
        protected_fields.insert("default");
        protected_fields.insert("properties");
        protected_fields.insert("const");

        Self {
            protected_fields,
            func_desc_max_len: 256,
            param_desc_max_len: 160,
            drop_examples: true,
            drop_titles: true,
            drop_markdown: true,
        }
    }
}

impl SchemaCompressor {
    /// Create a new SchemaCompressor with default settings
    pub fn new() -> Self {
        Self::default()
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

    /// Compress an OpenAI Function Calling schema
    pub fn compress(&self, tool: &Value) -> Value {
        let mut result = tool.clone();

        // Check if this is a function wrapper or direct schema
        if let Some(function) = result.get_mut("function") {
            // Compress function-level description
            if let Some(desc) = function.get("description").and_then(|d| d.as_str()) {
                let compressed = self.truncate_description(desc, self.func_desc_max_len);
                function["description"] = Value::String(compressed);
            }

            // Optionally remove title
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

        result
    }

    /// Recursively compress a JSON Schema
    pub fn compress_json_schema(&self, schema: &mut Value, depth: usize) {
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
        if let Some(any_of) = obj.get_mut("anyOf") {
            if let Some(arr) = any_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }

        // Handle oneOf
        if let Some(one_of) = obj.get_mut("oneOf") {
            if let Some(arr) = one_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }

        // Handle allOf
        if let Some(all_of) = obj.get_mut("allOf") {
            if let Some(arr) = all_of.as_array_mut() {
                for item in arr.iter_mut() {
                    self.compress_json_schema(item, depth + 1);
                }
            }
        }
    }

    /// Intelligently truncate a description string
    pub fn truncate_description(&self, desc: &str, max_len: usize) -> String {
        // Trim whitespace
        let mut text = desc.trim().to_string();

        // Remove markdown code blocks if configured
        if self.drop_markdown {
            // Remove fenced code blocks: ```...```
            let code_block_re = Regex::new(r"```[\s\S]*?```").unwrap();
            text = code_block_re.replace_all(&text, "").to_string();

            // Remove inline code: `...`
            let inline_code_re = Regex::new(r"`[^`]+`").unwrap();
            text = inline_code_re.replace_all(&text, "").to_string();
        }

        // Collapse multiple whitespace/newlines into single space
        let whitespace_re = Regex::new(r"\s+").unwrap();
        text = whitespace_re.replace_all(&text, " ").to_string();
        text = text.trim().to_string();

        // If already within limit, return as-is
        if text.len() <= max_len {
            return text;
        }

        // Try to find a sentence boundary in the range [max_len*0.5, max_len]
        let min_pos = (max_len as f64 * 0.5) as usize;
        let search_range = &text[min_pos..max_len.min(text.len())];

        // Look for sentence endings: . 。 ！ ？
        let sentence_endings = ['.', '。', '！', '？'];
        let mut best_pos = None;

        for (i, c) in search_range.char_indices() {
            if sentence_endings.contains(&c) {
                // Position after the sentence ending
                best_pos = Some(min_pos + i + c.len_utf8());
            }
        }

        if let Some(pos) = best_pos {
            return text[..pos].trim().to_string();
        }

        // No sentence boundary found, hard truncate
        // Handle UTF-8 properly by finding char boundary
        let mut truncate_pos = max_len;
        while !text.is_char_boundary(truncate_pos) && truncate_pos > 0 {
            truncate_pos -= 1;
        }

        text[..truncate_pos].trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compress_long_description() {
        let compressor = SchemaCompressor::new();
        let schema = json!({
            "function": {
                "name": "test_func",
                "description": "This is a very long description that should be truncated. It contains a lot of text that goes on and on. The quick brown fox jumps over the lazy dog. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "param1": {
                            "type": "string",
                            "description": "Another long description for a parameter that should be truncated to a shorter length. This text is intentionally verbose to test the truncation logic properly."
                        }
                    }
                }
            }
        });

        let result = compressor.compress(&schema);

        // Function description should be truncated to <= 256
        let func_desc = result["function"]["description"].as_str().unwrap();
        assert!(func_desc.len() <= 256);

        // Parameter description should be truncated to <= 160
        let param_desc = result["function"]["parameters"]["properties"]["param1"]["description"]
            .as_str()
            .unwrap();
        assert!(param_desc.len() <= 160);
    }

    #[test]
    fn test_protected_fields_preserved() {
        let compressor = SchemaCompressor::new();
        let schema = json!({
            "function": {
                "name": "my_function",
                "parameters": {
                    "type": "object",
                    "required": ["field1"],
                    "properties": {
                        "field1": {
                            "type": "string",
                            "enum": ["a", "b", "c"],
                            "default": "a",
                            "const": "fixed_value"
                        }
                    }
                }
            }
        });

        let result = compressor.compress(&schema);

        // Verify protected fields are preserved
        assert_eq!(result["function"]["name"], "my_function");
        assert_eq!(result["function"]["parameters"]["type"], "object");
        assert!(result["function"]["parameters"]["required"].is_array());
        assert!(result["function"]["parameters"]["properties"]["field1"]["enum"].is_array());
        assert_eq!(
            result["function"]["parameters"]["properties"]["field1"]["default"],
            "a"
        );
        assert_eq!(
            result["function"]["parameters"]["properties"]["field1"]["const"],
            "fixed_value"
        );
    }

    #[test]
    fn test_title_and_examples_removed() {
        let compressor = SchemaCompressor::new();
        let schema = json!({
            "function": {
                "name": "test",
                "title": "Test Function Title",
                "parameters": {
                    "type": "object",
                    "title": "Parameters Title",
                    "properties": {
                        "field1": {
                            "type": "string",
                            "title": "Field Title",
                            "examples": ["example1", "example2"]
                        }
                    }
                }
            }
        });

        let result = compressor.compress(&schema);

        // Titles should be removed
        assert!(result["function"].get("title").is_none());
        assert!(result["function"]["parameters"].get("title").is_none());
        assert!(
            result["function"]["parameters"]["properties"]["field1"]
                .get("title")
                .is_none()
        );

        // Examples should be removed
        assert!(
            result["function"]["parameters"]["properties"]["field1"]
                .get("examples")
                .is_none()
        );
    }

    #[test]
    fn test_empty_schema_no_panic() {
        let compressor = SchemaCompressor::new();

        // Empty object
        let result = compressor.compress(&json!({}));
        assert!(result.is_object());

        // Null
        let result = compressor.compress(&Value::Null);
        assert!(result.is_null());

        // Empty function
        let result = compressor.compress(&json!({"function": {}}));
        assert!(result["function"].is_object());
    }

    #[test]
    fn test_nested_properties_recursive_compression() {
        let compressor = SchemaCompressor::new();
        let schema = json!({
            "function": {
                "name": "nested_test",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "level1": {
                            "type": "object",
                            "title": "Level 1 Title",
                            "description": "Level 1 description that is quite long and should be truncated according to the parameter max length setting.",
                            "properties": {
                                "level2": {
                                    "type": "object",
                                    "title": "Level 2 Title",
                                    "examples": ["ex1"],
                                    "properties": {
                                        "level3": {
                                            "type": "string",
                                            "title": "Level 3 Title"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let result = compressor.compress(&schema);

        // Check nested titles are removed
        assert!(
            result["function"]["parameters"]["properties"]["level1"]
                .get("title")
                .is_none()
        );
        assert!(
            result["function"]["parameters"]["properties"]["level1"]["properties"]["level2"]
                .get("title")
                .is_none()
        );
        assert!(
            result["function"]["parameters"]["properties"]["level1"]["properties"]["level2"]
                ["properties"]["level3"]
                .get("title")
                .is_none()
        );

        // Check nested examples are removed
        assert!(
            result["function"]["parameters"]["properties"]["level1"]["properties"]["level2"]
                .get("examples")
                .is_none()
        );
    }

    #[test]
    fn test_truncate_at_sentence_boundary() {
        let compressor = SchemaCompressor::new();
        // Sentence boundary at position ~40 which is in range [30, 60]
        let text = "Short intro text for testing. This sentence ends here. More text follows after that point.";

        let result = compressor.truncate_description(text, 60);

        // Should truncate at a sentence boundary
        assert!(
            result.ends_with('.'),
            "Result '{}' should end with '.'",
            result
        );
        assert!(result.len() <= 60);
    }

    #[test]
    fn test_markdown_removal() {
        let compressor = SchemaCompressor::new();
        let text = "Some text with ```code block``` and `inline code` markers.";

        let result = compressor.truncate_description(text, 256);

        assert!(!result.contains("```"));
        assert!(!result.contains('`'));
    }

    #[test]
    fn test_anyof_oneof_allof_compression() {
        let compressor = SchemaCompressor::new();
        let schema = json!({
            "function": {
                "name": "combo_test",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "field1": {
                            "anyOf": [
                                {"type": "string", "title": "String Option", "examples": ["ex"]},
                                {"type": "number", "title": "Number Option"}
                            ]
                        },
                        "field2": {
                            "oneOf": [
                                {"type": "boolean", "title": "Bool Option"}
                            ]
                        },
                        "field3": {
                            "allOf": [
                                {"type": "object", "title": "Obj Option"}
                            ]
                        }
                    }
                }
            }
        });

        let result = compressor.compress(&schema);

        // Check anyOf items are compressed
        assert!(
            result["function"]["parameters"]["properties"]["field1"]["anyOf"][0]
                .get("title")
                .is_none()
        );
        assert!(
            result["function"]["parameters"]["properties"]["field1"]["anyOf"][0]
                .get("examples")
                .is_none()
        );

        // Check oneOf items are compressed
        assert!(
            result["function"]["parameters"]["properties"]["field2"]["oneOf"][0]
                .get("title")
                .is_none()
        );

        // Check allOf items are compressed
        assert!(
            result["function"]["parameters"]["properties"]["field3"]["allOf"][0]
                .get("title")
                .is_none()
        );
    }
}
