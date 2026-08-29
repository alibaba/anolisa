use std::sync::Arc;

use serde_json::Value;
use tokenless_ccr::{InMemoryStore, StashError, StashStore, StashWrite, extract_hash};

fn context<'a>(stash: Option<&'a dyn StashStore>) -> JsonCompressionContext<'a> {
    JsonCompressionContext {
        stash,
        allow_toon: false,
        preserve_top_level_shape: false,
        min_toon_chars: 500,
    }
}

fn output_value(outcome: &JsonOutcome) -> Value {
    serde_json::from_str(&outcome.output).expect("JSON representation")
}

#[test]
fn cleanup_reports_the_operation_without_side_channels() {
    let outcome = JsonCompressor::default()
        .compress(
            r#"{"data":"kept","debug":"drop","empty":null}"#,
            &context(None),
        )
        .unwrap();
    assert_eq!(outcome.output, r#"{"data":"kept"}"#);
    assert_eq!(outcome.operations, [JsonOperation::Cleanup]);
    assert_eq!(outcome.recoverability, Recoverability::Lossless);
    assert!(outcome.stash_writes.is_empty());
}

#[test]
fn string_truncation_is_unicode_safe_and_bounded() {
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_strings_at: 10,
        ..JsonCompressionConfig::default()
    });
    let outcome = compressor
        .compress(r#"{"value":"你好世界，这是一个很长的测试"}"#, &context(None))
        .unwrap();
    let value = output_value(&outcome);
    assert!(value["value"].as_str().unwrap().chars().count() <= 10);
    assert_eq!(
        outcome.operations,
        [JsonOperation::Truncation],
        "truncation is not mislabeled as cleanup"
    );
    assert_eq!(outcome.recoverability, Recoverability::Unrecoverable);
    assert_eq!(outcome.metrics.truncations, 1);
    assert_eq!(outcome.metrics.unrecoverable_truncations, 1);
}

#[test]
fn array_truncation_preserves_head_and_tail() {
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_arrays_at: 3,
        array_tail_preserve: 2,
        ..JsonCompressionConfig::default()
    });
    let input = serde_json::to_string(
        &(1..=10)
            .map(|index| format!("item-{index}-{}", "x".repeat(80)))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let outcome = compressor.compress(&input, &context(None)).unwrap();
    let output = output_value(&outcome);
    let array = output.as_array().unwrap();
    assert!(array[0].as_str().unwrap().starts_with("item-1-"));
    assert!(array[2].as_str().unwrap().starts_with("item-3-"));
    assert!(array[array.len() - 2].as_str().unwrap().starts_with("item-9-"));
    assert!(array[array.len() - 1].as_str().unwrap().starts_with("item-10-"));
    assert!(array[3].as_str().unwrap().contains("5 more items"));
    assert_eq!(outcome.recoverability, Recoverability::Unrecoverable);
    assert_eq!(outcome.metrics.unrecoverable_truncations, 1);
}

#[test]
fn drop_nulls_is_independent_from_empty_value_cleanup() {
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        drop_nulls: false,
        drop_empty_fields: true,
        ..JsonCompressionConfig::default()
    });
    let outcome = compressor
        .compress(
            r#"{"object_null":null,"empty":"","array":[null,""]}"#,
            &context(None),
        )
        .unwrap();

    assert_eq!(
        output_value(&outcome),
        serde_json::json!({"object_null": null, "array": [null]})
    );
}

#[test]
fn depth_truncation_stashes_the_exact_subtree() {
    let store = InMemoryStore::new();
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        max_depth: 0,
        ..JsonCompressionConfig::default()
    });
    let subtree = serde_json::json!({"value": "exact payload".repeat(40)});
    let input = serde_json::to_string(&serde_json::json!({"nested": subtree})).unwrap();
    let outcome = compressor
        .compress(&input, &context(Some(&store)))
        .unwrap();
    assert_eq!(outcome.operations, [JsonOperation::Truncation]);
    assert_eq!(outcome.recoverability, Recoverability::Retrievable);
    let hash = extract_hash(&outcome.output).unwrap();
    assert_eq!(
        store.retrieve(hash).unwrap().as_deref(),
        Some(serde_json::to_string(&subtree).unwrap().as_str())
    );
}

#[test]
fn stashed_array_tail_round_trips_exactly() {
    let store = InMemoryStore::new();
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_arrays_at: 3,
        array_tail_preserve: 0,
        ..JsonCompressionConfig::default()
    });
    let values = (1..=8)
        .map(|index| format!("item-{index}-{}", "x".repeat(80)))
        .collect::<Vec<_>>();
    let input = serde_json::to_string(&values).unwrap();
    let outcome = compressor.compress(&input, &context(Some(&store))).unwrap();
    assert_eq!(outcome.recoverability, Recoverability::Retrievable);
    assert_eq!(outcome.stash_writes.len(), 1);
    let hash = extract_hash(&outcome.output).unwrap();
    assert_eq!(
        store.retrieve(hash).unwrap().as_deref(),
        Some(serde_json::to_string(&values[3..]).unwrap().as_str())
    );
}

#[test]
fn structured_slots_restore_empty_top_level_fields() {
    let context = JsonCompressionContext {
        preserve_top_level_shape: true,
        ..context(None)
    };
    let outcome = JsonCompressor::default()
        .compress(r#"{"stdout":"value","stderr":"","debug":"drop"}"#, &context)
        .unwrap();
    assert_eq!(
        output_value(&outcome),
        serde_json::json!({"stdout": "value", "stderr": ""})
    );
}

#[test]
fn json_string_envelope_is_normalized_once() {
    let input = serde_json::to_string(r#"{"data":"kept","debug":"drop"}"#).unwrap();
    let outcome = JsonCompressor::default()
        .compress(&input, &context(None))
        .unwrap();
    assert_eq!(outcome.output, r#"{"data":"kept"}"#);
}

#[test]
fn invalid_json_is_an_error() {
    assert!(JsonCompressor::default()
        .compress("not json", &context(None))
        .is_err());
}

#[test]
fn json_scalar_is_a_no_op() {
    let outcome = JsonCompressor::default()
        .compress("42", &context(None))
        .unwrap();
    assert_eq!(outcome.output, "42");
    assert!(outcome.operations.is_empty());
}

#[test]
fn toon_is_an_internal_json_operation() {
    let input = format!(
        r#"{{"items":[{}]}}"#,
        (0..80)
            .map(|index| format!(r#"{{"id":{index},"name":"item-{index}"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let context = JsonCompressionContext {
        allow_toon: true,
        min_toon_chars: 0,
        ..context(None)
    };
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_arrays_at: 200,
        ..JsonCompressionConfig::default()
    });
    let outcome = compressor.compress(&input, &context).unwrap();
    assert_eq!(outcome.operations.last(), Some(&JsonOperation::Toon));
    assert!(serde_json::from_str::<Value>(&outcome.output).is_err());
}

struct AlwaysFail;

impl StashStore for AlwaysFail {
    fn stash(&self, _payload: &str) -> Result<StashWrite, StashError> {
        Err(StashError::Backend("simulated".to_owned()))
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

    fn delete(&self, _hash: &str, _generation: u64) -> Result<bool, StashError> {
        Ok(false)
    }
}

#[test]
fn stash_failure_is_visible_and_degrades_recovery() {
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_strings_at: 80,
        ..JsonCompressionConfig::default()
    });
    let outcome = compressor
        .compress(
            &serde_json::to_string(&"x".repeat(200)).unwrap(),
            &context(Some(&AlwaysFail)),
        )
        .unwrap();
    assert_eq!(outcome.metrics.stash_errors, 1);
    assert_eq!(outcome.metrics.unrecoverable_truncations, 1);
    assert_eq!(outcome.recoverability, Recoverability::Unrecoverable);
}

#[test]
fn duplicate_stash_payloads_return_every_write_for_the_runtime_ledger() {
    let store = Arc::new(InMemoryStore::new());
    let compressor = JsonCompressor::new(JsonCompressionConfig {
        truncate_arrays_at: 2,
        array_tail_preserve: 0,
        ..JsonCompressionConfig::default()
    });
    let outcome = compressor
        .compress(
            r#"{"a":[1,2,3,4,5],"b":[1,2,3,4,5]}"#,
            &context(Some(store.as_ref())),
        )
        .unwrap();
    assert_eq!(outcome.stash_writes.len(), 2);
    assert_eq!(store.len(), 1);
}
