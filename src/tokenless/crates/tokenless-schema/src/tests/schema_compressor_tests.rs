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

#[test]
fn max_depth_stops_recursion() {
    // Build a 100-level schema and verify with_max_depth bounds the
    // recursive descent — descriptions below the limit must be left
    // untouched, descriptions above must be truncated.
    let compressor = SchemaCompressor::new().with_max_depth(5);
    let long_desc = "x".repeat(400);
    let mut schema = json!({
        "type": "string",
        "description": long_desc.clone(),
    });
    for _ in 0..100 {
        schema = json!({
            "type": "object",
            "description": long_desc.clone(),
            "properties": {"nested": schema},
        });
    }
    let result = compressor.compress(&schema);
    // Top-level description (depth 0) must be truncated.
    let top = result["description"].as_str().unwrap();
    assert!(top.chars().count() <= 256);
    // Walk down 10 levels — well past max_depth — and confirm we still
    // see the original 400-char description (recursion stopped early).
    let mut node = &result;
    for _ in 0..10 {
        node = &node["properties"]["nested"];
    }
    let deep = node["description"].as_str().unwrap();
    assert_eq!(deep.chars().count(), 400);
}

#[test]
fn truncate_description_cjk_no_panic() {
    let compressor = SchemaCompressor::new();
    // 100 CJK chars fit within 256-char limit — no truncation needed
    let cjk = "中".repeat(100);
    let result = compressor.truncate_description(&cjk, 256);
    assert!(result.chars().all(|c| c == '中'));
    assert!(result.chars().count() <= 256);

    // 300 CJK chars exceed 256-char limit — should be truncated
    let cjk_long = "中".repeat(300);
    let result_long = compressor.truncate_description(&cjk_long, 256);
    assert!(result_long.chars().count() <= 256);
}

#[test]
fn builder_with_func_desc_max_len() {
    let c = SchemaCompressor::new().with_func_desc_max_len(50);
    let long = "A".repeat(100);
    let schema = json!({
        "function": {
            "name": "test",
            "description": long,
            "parameters": {"type": "object", "properties": {}}
        }
    });
    let result = c.compress(&schema);
    let desc = result["function"]["description"].as_str().unwrap();
    assert!(desc.chars().count() <= 50);
}

#[test]
fn builder_with_param_desc_max_len() {
    let c = SchemaCompressor::new().with_param_desc_max_len(30);
    let long = "B".repeat(100);
    let schema = json!({
        "function": {
            "name": "test",
            "parameters": {
                "type": "object",
                "properties": {
                    "p": {"type": "string", "description": long}
                }
            }
        }
    });
    let result = c.compress(&schema);
    let desc = result["function"]["parameters"]["properties"]["p"]["description"]
        .as_str()
        .unwrap();
    assert!(desc.chars().count() <= 30);
}

#[test]
fn builder_with_drop_examples_false_preserves() {
    let c = SchemaCompressor::new().with_drop_examples(false);
    let schema = json!({
        "function": {
            "name": "test",
            "parameters": {
                "type": "object",
                "properties": {
                    "p": {"type": "string", "examples": ["a", "b"]}
                }
            }
        }
    });
    let result = c.compress(&schema);
    assert!(
        result["function"]["parameters"]["properties"]["p"]
            .get("examples")
            .is_some()
    );
}

#[test]
fn builder_with_drop_titles_false_preserves() {
    let c = SchemaCompressor::new().with_drop_titles(false);
    let schema = json!({
        "function": {
            "name": "test",
            "title": "Keep This",
            "parameters": {
                "type": "object",
                "title": "Params",
                "properties": {}
            }
        }
    });
    let result = c.compress(&schema);
    assert_eq!(result["function"]["title"], "Keep This");
    assert_eq!(result["function"]["parameters"]["title"], "Params");
}

#[test]
fn builder_with_drop_markdown_false_preserves() {
    let c = SchemaCompressor::new().with_drop_markdown(false);
    let text = "Use `code` in description.";
    let result = c.truncate_description(text, 256);
    assert!(result.contains('`'));
}

#[test]
fn compress_direct_schema_no_function_wrapper() {
    let c = SchemaCompressor::new();
    let long = "D".repeat(400);
    let schema = json!({
        "type": "object",
        "title": "TopLevel",
        "description": long,
        "properties": {
            "name": {"type": "string", "title": "FieldTitle"}
        }
    });
    let result = c.compress(&schema);
    assert!(result.get("title").is_none());
    let desc = result["description"].as_str().unwrap();
    assert!(desc.chars().count() <= 256);
    assert!(result["properties"]["name"].get("title").is_none());
}

#[test]
fn char_index_empty_string() {
    assert_eq!(char_index("", 0), 0);
    assert_eq!(char_index("", 5), 0);
}

#[test]
fn char_index_beyond_length() {
    assert_eq!(char_index("abc", 10), 3);
}

#[test]
fn char_index_multibyte() {
    let text = "你好world";
    assert_eq!(char_index(text, 0), 0);
    assert_eq!(char_index(text, 2), 6); // 2 CJK chars = 6 bytes
}

#[test]
fn test_description_truncation_with_stash() {
    use std::sync::Arc;
    use tokenless_ccr::{InMemoryStore, StashStore, extract_hash};

    let store = Arc::new(InMemoryStore::new());
    let compressor = SchemaCompressor::new()
        .with_func_desc_max_len(100)
        .with_stash_store(store.clone());
    let long_desc = "A".repeat(300);
    let schema = json!({
        "function": {
            "name": "test",
            "description": long_desc,
            "parameters": {"type": "object", "properties": {}}
        }
    });
    let result = compressor.compress(&schema);
    let desc = result["function"]["description"].as_str().unwrap();
    assert!(desc.chars().count() <= 100);
    assert!(desc.contains("tokenless:"));
    let hash = extract_hash(desc).unwrap();
    let retrieved = store.retrieve(hash).unwrap().unwrap();
    assert_eq!(retrieved, long_desc);
}

#[test]
fn test_compress_parameters_with_nested_schema() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "test",
            "parameters": {
                "type": "object",
                "properties": {
                    "config": {
                        "type": "object",
                        "title": "Config Title",
                        "examples": ["ex1"],
                        "properties": {
                            "nested": {
                                "type": "string",
                                "title": "Nested Title",
                                "description": "B".repeat(200)
                            }
                        }
                    }
                }
            }
        }
    });
    let result = compressor.compress(&schema);
    let props = &result["function"]["parameters"]["properties"];
    assert!(props["config"].get("title").is_none());
    assert!(props["config"].get("examples").is_none());
    assert!(props["config"]["properties"]["nested"].get("title").is_none());
    let nested_desc = props["config"]["properties"]["nested"]["description"]
        .as_str()
        .unwrap();
    assert!(nested_desc.chars().count() <= 160);
}
