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
    // One truncated array → one stash write.
    assert_eq!(compressor.stash_writes(), 1);
}

#[test]
fn test_stash_writes_counter_zero_without_store() {
    // No stash store attached → counter stays zero even when arrays are
    // truncated (lossy path).
    let compressor = ResponseCompressor::new().with_truncate_arrays_at(3);
    let arr: Vec<i32> = (1..=10).collect();
    compressor.compress(&json!(arr));
    assert_eq!(compressor.stash_writes(), 0);
}

#[test]
fn test_stash_writes_counter_resets_per_compress() {
    use std::sync::Arc;
    use tokenless_ccr::InMemoryStore;

    let store = Arc::new(InMemoryStore::new());
    let compressor = ResponseCompressor::new()
        .with_truncate_arrays_at(3)
        .with_stash_store(store);
    let arr: Vec<i32> = (1..=10).collect();
    compressor.compress(&json!(arr));
    assert_eq!(compressor.stash_writes(), 1);
    // Second call resets, then writes again — still 1, not 2.
    compressor.compress(&json!(arr));
    assert_eq!(compressor.stash_writes(), 1);
    // A call that doesn't truncate (within limit) resets to 0.
    compressor.compress(&json!([1, 2, 3]));
    assert_eq!(compressor.stash_writes(), 0);
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
    // The failed write is surfaced via the error counter so a persistent
    // backend failure isn't invisible.
    assert_eq!(compressor.stash_errors(), 1);
    assert_eq!(compressor.stash_writes(), 0);
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

#[test]
fn test_add_drop_field() {
    let mut compressor = ResponseCompressor::new();
    compressor.add_drop_field("custom_debug");
    let obj = json!({
        "data": "keep",
        "custom_debug": "drop this"
    });
    let result = compressor.compress(&obj);
    let obj_result = result.as_object().unwrap();
    assert!(obj_result.contains_key("data"));
    assert!(!obj_result.contains_key("custom_debug"));
}

#[test]
fn test_with_add_truncation_marker_false() {
    let compressor = ResponseCompressor::new()
        .with_truncate_strings_at(5)
        .with_add_truncation_marker(false);
    let long = "abcdefghij";
    let result = compressor.compress(&json!(long));
    let s = result.as_str().unwrap();
    assert!(!s.contains("truncated"));
    assert_eq!(s.len(), 5);
}

#[test]
fn test_stash_errors_counter() {
    let compressor = ResponseCompressor::new();
    assert_eq!(compressor.stash_errors(), 0);
    assert_eq!(compressor.stash_writes(), 0);
}

#[test]
fn test_compress_null_preserves() {
    let compressor = ResponseCompressor::new().with_drop_nulls(false);
    let result = compressor.compress(&Value::Null);
    assert!(result.is_null());
}

#[test]
fn test_is_empty_value() {
    let compressor = ResponseCompressor::new();
    assert!(compressor.is_empty_value(&json!("")));
    assert!(compressor.is_empty_value(&json!([])));
    assert!(compressor.is_empty_value(&json!({})));
    assert!(!compressor.is_empty_value(&json!("x")));
    assert!(!compressor.is_empty_value(&json!(0)));
    assert!(!compressor.is_empty_value(&json!(null)));
}
