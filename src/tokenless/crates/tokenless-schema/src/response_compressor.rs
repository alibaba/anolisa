use serde_json::{Map, Value};
use std::collections::HashSet;

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
            truncate_strings_at: 512,
            truncate_arrays_at: 16,
            drop_nulls: true,
            drop_empty_fields: true,
            max_depth: 8,
            add_truncation_marker: true,
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

    /// Add a field name to the drop list
    pub fn add_drop_field(&mut self, field: &str) {
        self.drop_fields.insert(field.to_string());
    }

    /// Compress a JSON response value
    pub fn compress(&self, response: &Value) -> Value {
        self.compress_value(response, 0)
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

    /// Compress a string value, truncating if necessary
    fn compress_string(&self, s: &str) -> Value {
        if s.len() <= self.truncate_strings_at {
            return Value::String(s.to_string());
        }

        // Find a safe UTF-8 boundary
        let mut truncate_pos = self.truncate_strings_at;
        while !s.is_char_boundary(truncate_pos) && truncate_pos > 0 {
            truncate_pos -= 1;
        }

        let truncated = &s[..truncate_pos];

        if self.add_truncation_marker {
            Value::String(format!("{}… (truncated)", truncated))
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
            result.push(Value::String(format!(
                "<... {} more items truncated>",
                remaining
            )));
        }

        Value::Array(result)
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
    fn test_string_truncation_512_default() {
        let compressor = ResponseCompressor::new();

        let long_string = "x".repeat(600);
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
    fn test_array_truncation_16_default() {
        let compressor = ResponseCompressor::new();

        let arr: Vec<i32> = (1..=30).collect();
        let result = compressor.compress(&json!(arr));

        let arr_result = result.as_array().unwrap();
        // 16 items + 1 truncation marker = 17
        assert_eq!(arr_result.len(), 17);
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
            .with_truncate_strings_at(10)
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
        assert!(s.len() > 0);
    }
}
