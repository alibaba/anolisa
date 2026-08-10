//! Integration tests for tokenless-schema crate.
//!
//! Tests cover SchemaCompressor and ResponseCompressor functionality
//! including real-world fixture schemas.

use serde_json::{Value, json};
use std::sync::Arc;
use tokenless_ccr::{InMemoryStore, StashStore};
use tokenless_schema::{ResponseCompressor, SchemaCompressor};

// ============================================================
// Helper
// ============================================================

fn fixtures_dir() -> String {
    format!("{}/tests/fixtures/schemas", env!("CARGO_MANIFEST_DIR"))
}

fn load_schema(name: &str) -> Value {
    let path = format!("{}/{}", fixtures_dir(), name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to load schema {}: {}", path, e));
    serde_json::from_str(&content).unwrap()
}

fn response_fixtures_dir() -> String {
    format!("{}/tests/fixtures/responses", env!("CARGO_MANIFEST_DIR"))
}

fn load_response(name: &str) -> Value {
    let path = format!("{}/{}", response_fixtures_dir(), name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to load response {}: {}", path, e));
    serde_json::from_str(&content).unwrap()
}

// ============================================================
// SchemaCompressor – basic functionality
// ============================================================

#[test]
fn test_compress_simple_schema() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "greet",
            "description": "Say hello",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            }
        }
    });

    let result = compressor.compress(&schema);
    assert!(result.is_object());
    assert_eq!(result["function"]["name"], "greet");
}

#[test]
fn test_compress_nested_properties() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "nested",
            "parameters": {
                "type": "object",
                "properties": {
                    "address": {
                        "type": "object",
                        "title": "Address",
                        "properties": {
                            "street": {
                                "type": "string",
                                "title": "Street Name"
                            },
                            "city": {
                                "type": "string",
                                "title": "City Name"
                            }
                        }
                    }
                }
            }
        }
    });

    let result = compressor.compress(&schema);

    // Nested structure preserved
    assert!(
        result
            .pointer("/function/parameters/properties/address/properties/street")
            .is_some()
    );

    // Titles removed at all levels
    assert!(
        result
            .pointer("/function/parameters/properties/address")
            .unwrap()
            .get("title")
            .is_none()
    );
    assert!(
        result
            .pointer("/function/parameters/properties/address/properties/street")
            .unwrap()
            .get("title")
            .is_none()
    );
}

#[test]
fn test_protected_fields_preserved() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "calc",
            "parameters": {
                "type": "object",
                "required": ["op"],
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["add", "sub"],
                        "default": "add"
                    }
                }
            }
        }
    });

    let result = compressor.compress(&schema);

    assert_eq!(result["function"]["name"], "calc");
    assert_eq!(result["function"]["parameters"]["type"], "object");
    assert!(result["function"]["parameters"]["required"].is_array());
    assert!(result["function"]["parameters"]["properties"]["op"]["enum"].is_array());
    assert_eq!(
        result["function"]["parameters"]["properties"]["op"]["default"],
        "add"
    );
}

#[test]
fn test_description_truncation() {
    let compressor = SchemaCompressor::new();
    let long_desc = "A".repeat(500);
    let schema = json!({
        "function": {
            "name": "test",
            "description": long_desc,
            "parameters": { "type": "object" }
        }
    });

    let result = compressor.compress(&schema);
    let desc = result["function"]["description"].as_str().unwrap();
    assert!(
        desc.len() < 500,
        "Description should be truncated, got len={}",
        desc.len()
    );
}

#[test]
fn test_titles_removed() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "test",
            "title": "Should Go",
            "parameters": {
                "type": "object",
                "title": "Also Go",
                "properties": {
                    "x": {
                        "type": "string",
                        "title": "Gone Too"
                    }
                }
            }
        }
    });

    let result = compressor.compress(&schema);

    assert!(result["function"].get("title").is_none());
    assert!(result["function"]["parameters"].get("title").is_none());
    assert!(
        result["function"]["parameters"]["properties"]["x"]
            .get("title")
            .is_none()
    );
}

#[test]
fn test_examples_removed() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "test",
            "parameters": {
                "type": "object",
                "properties": {
                    "email": {
                        "type": "string",
                        "examples": ["a@b.com", "c@d.com"]
                    }
                }
            }
        }
    });

    let result = compressor.compress(&schema);
    assert!(
        result
            .pointer("/function/parameters/properties/email")
            .unwrap()
            .get("examples")
            .is_none()
    );
}

#[test]
fn test_enum_values_preserved() {
    let compressor = SchemaCompressor::new();
    let schema = json!({
        "function": {
            "name": "calculate",
            "parameters": {
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["add", "subtract", "multiply", "divide"]
                    }
                }
            }
        }
    });

    let result = compressor.compress(&schema);
    let op = result
        .pointer("/function/parameters/properties/operation")
        .unwrap();
    assert!(op.get("enum").is_some());
    assert_eq!(op["enum"].as_array().unwrap().len(), 4);
}

#[test]
fn test_empty_input_no_panic() {
    let compressor = SchemaCompressor::new();

    // Empty object
    let r = compressor.compress(&json!({}));
    assert!(r.is_object());

    // Null
    let r = compressor.compress(&Value::Null);
    assert!(r.is_null());

    // Empty function wrapper
    let r = compressor.compress(&json!({"function": {}}));
    assert!(r["function"].is_object());
}

// ============================================================
// ResponseCompressor – basic functionality
// ============================================================

#[test]
fn test_response_compress_long_strings() {
    let compressor = ResponseCompressor::new().with_truncate_strings_at(20);
    let response = json!({
        "message": "This is a very long message that should definitely be truncated by the compressor"
    });

    let result = compressor.compress(&response);
    let msg = result["message"].as_str().unwrap();
    assert!(msg.contains("truncated"));
}

#[test]
fn test_response_array_truncation() {
    let compressor = ResponseCompressor::new().with_truncate_arrays_at(3);
    let arr: Vec<i32> = (1..=10).collect();
    let result = compressor.compress(&json!(arr));

    let arr_result = result.as_array().unwrap();
    // 3 items + 1 truncation marker
    assert_eq!(arr_result.len(), 4);
    assert!(arr_result[3].as_str().unwrap().contains("truncated"));
}

#[test]
fn test_response_null_removal() {
    let compressor = ResponseCompressor::new();
    let response = json!({
        "name": "test",
        "value": null,
        "count": 5
    });

    let result = compressor.compress(&response);
    let obj = result.as_object().unwrap();
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("count"));
    assert!(!obj.contains_key("value"));
}

#[test]
fn test_response_depth_limit() {
    let compressor = ResponseCompressor::new().with_max_depth(2);
    let deep = json!({
        "l1": {
            "l2": {
                "l3": {
                    "l4": "deep"
                }
            }
        }
    });

    let result = compressor.compress(&deep);
    let l3 = &result["l1"]["l2"]["l3"];
    assert!(l3.as_str().unwrap().contains("truncated at depth"));
}

#[test]
fn test_response_drop_debug_fields() {
    let compressor = ResponseCompressor::new();
    let response = json!({
        "data": "important",
        "debug": "should go",
        "trace": "should go",
        "stack": "should go"
    });

    let result = compressor.compress(&response);
    let obj = result.as_object().unwrap();
    assert!(obj.contains_key("data"));
    assert!(!obj.contains_key("debug"));
    assert!(!obj.contains_key("trace"));
    assert!(!obj.contains_key("stack"));
}

#[test]
fn test_response_basic_object() {
    let compressor = ResponseCompressor::new();
    let response = json!({
        "status": "success",
        "data": {
            "items": [
                {"id": 1, "name": "Item 1"},
                {"id": 2, "name": "Item 2"}
            ],
            "total": 2
        }
    });

    let result = compressor.compress(&response);
    assert!(result.is_object());
    assert_eq!(result["status"], "success");
}

// ============================================================
// Fixture-based tests
// ============================================================

#[test]
fn test_fixture_simple_calculator() {
    let schema = load_schema("simple_calculator.json");
    let compressor = SchemaCompressor::new();
    let compressed = compressor.compress(&schema);

    assert!(compressed.is_object());
    assert_eq!(compressed["function"]["name"], "calculate");

    // Enum preserved
    let op = compressed
        .pointer("/function/parameters/properties/operation")
        .unwrap();
    assert!(op.get("enum").is_some());
}

#[test]
fn test_fixture_hubspot_contact() {
    let schema = load_schema("hubspot_contact.json");
    let compressor = SchemaCompressor::new();
    let compressed = compressor.compress(&schema);

    assert!(compressed.is_object());

    // Structure preserved
    assert!(
        compressed
            .pointer("/function/parameters/properties")
            .is_some()
    );
    assert_eq!(compressed["function"]["name"], "create_or_update_contact");

    // Compression occurred
    let orig_len = serde_json::to_string(&schema).unwrap().len();
    let comp_len = serde_json::to_string(&compressed).unwrap().len();
    assert!(
        comp_len < orig_len,
        "Should compress: original={}, compressed={}",
        orig_len,
        comp_len
    );

    // Enum preserved on lifecyclestage
    if let Some(ls) =
        compressed.pointer("/function/parameters/properties/properties/properties/lifecyclestage")
    {
        assert!(ls.get("enum").is_some(), "Enum should be preserved");
    }
}

#[test]
fn test_fixture_stripe_payment() {
    let schema = load_schema("stripe_payment.json");
    let compressor = SchemaCompressor::new();
    let compressed = compressor.compress(&schema);

    assert!(compressed.is_object());
    assert_eq!(compressed["function"]["name"], "create_payment_intent");

    let orig_len = serde_json::to_string(&schema).unwrap().len();
    let comp_len = serde_json::to_string(&compressed).unwrap().len();
    assert!(
        comp_len < orig_len,
        "Should compress: original={}, compressed={}",
        orig_len,
        comp_len
    );
}

#[test]
fn test_all_fixtures_produce_valid_json() {
    let compressor = SchemaCompressor::new();
    let files = [
        "simple_calculator.json",
        "hubspot_contact.json",
        "stripe_payment.json",
        "github_create_issue.json",
        "slack_send_message.json",
        "aws_describe_instances.json",
    ];

    for name in files {
        let schema = load_schema(name);
        let compressed = compressor.compress(&schema);

        // Must be valid JSON object
        assert!(compressed.is_object(), "{} should compress to object", name);

        // Serialization round-trip must succeed
        let json_str = serde_json::to_string(&compressed).unwrap();
        let reparsed: Value = serde_json::from_str(&json_str).unwrap();
        assert!(reparsed.is_object(), "{} round-trip failed", name);

        // Compression ratio > 0
        let orig_len = serde_json::to_string(&schema).unwrap().len();
        let comp_len = json_str.len();
        assert!(
            comp_len <= orig_len,
            "{}: compressed ({}) should be <= original ({})",
            name,
            comp_len,
            orig_len
        );

        // The top-level "type": "function" discriminant must be preserved when present.
        if schema.get("type").is_some() {
            assert_eq!(
                compressed.get("type"),
                schema.get("type"),
                "{}: top-level \"type\" field must survive compression",
                name
            );
        }
    }
}

#[test]
fn test_fixture_compression_ratio_benchmark() {
    let compressor = SchemaCompressor::new();
    let files = [
        "simple_calculator.json",
        "hubspot_contact.json",
        "stripe_payment.json",
        "github_create_issue.json",
        "slack_send_message.json",
        "aws_describe_instances.json",
    ];

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    for name in files {
        let schema = load_schema(name);
        let compressed = compressor.compress(&schema);

        let orig = serde_json::to_string(&schema).unwrap().len();
        let comp = serde_json::to_string(&compressed).unwrap().len();

        let saved = (1.0 - comp as f64 / orig as f64) * 100.0;
        println!(
            "{:<30} {} -> {} bytes ({:.1}% saved)",
            name, orig, comp, saved
        );

        total_orig += orig;
        total_comp += comp;
    }

    let avg_saved = (1.0 - total_comp as f64 / total_orig as f64) * 100.0;
    println!(
        "TOTAL: {} -> {} ({:.1}% saved)",
        total_orig, total_comp, avg_saved
    );

    assert!(
        avg_saved >= 5.0,
        "Average compression should be >= 5%, got {:.1}%",
        avg_saved
    );
}

// ============================================================
// Tokenless Benchmark: L1–L4 layered compression
// ============================================================
//
// L1 — Individual compressors (each compressor alone)
// L2 — Pairs (schema + response combined on tool call data)
// L3 — Triple (schema + response + stash/CCR reversible)
// L4 — Full pipeline (all optimizations combined)

const ALL_SCHEMA_FIXTURES: &[&str] = &[
    "simple_calculator.json",
    "hubspot_contact.json",
    "stripe_payment.json",
    "github_create_issue.json",
    "slack_send_message.json",
    "aws_describe_instances.json",
];

const RESPONSE_FIXTURES: &[&str] = &["github_issues.json"];

// github_issues_stashable.json has 35 items, which exceeds the default
// truncate_arrays_at=32, so the array tail gets stashed reversibly.
// Use this fixture wherever we need to assert store.len() > 0.
const RESPONSE_STASH_FIXTURES: &[&str] = &["github_issues_stashable.json"];

fn byte_len(v: &Value) -> usize {
    serde_json::to_string(v).unwrap().len()
}

#[test]
fn bench_l1_schema_compressor() {
    println!("\n=== L1: SchemaCompressor (individual) ===");
    let compressor = SchemaCompressor::new();

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    for name in ALL_SCHEMA_FIXTURES {
        let schema = load_schema(name);
        let compressed = compressor.compress(&schema);
        let orig = byte_len(&schema);
        let comp = byte_len(&compressed);
        let saved = (1.0 - comp as f64 / orig as f64) * 100.0;
        println!(
            "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
            name, orig, comp, saved
        );
        total_orig += orig;
        total_comp += comp;
    }

    let overall = (1.0 - total_comp as f64 / total_orig as f64) * 100.0;
    println!(
        "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
        "TOTAL", total_orig, total_comp, overall
    );

    assert!(
        overall >= 5.0,
        "L1 schema: expected >= 5% savings, got {:.1}%",
        overall
    );
}

#[test]
fn bench_l1_response_compressor() {
    println!("\n=== L1: ResponseCompressor (individual) ===");
    let compressor = ResponseCompressor::new();

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    for name in RESPONSE_FIXTURES {
        let response = load_response(name);
        let compressed = compressor.compress(&response);
        let orig = byte_len(&response);
        let comp = byte_len(&compressed);
        let saved = (1.0 - comp as f64 / orig as f64) * 100.0;
        println!(
            "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
            name, orig, comp, saved
        );
        total_orig += orig;
        total_comp += comp;
    }

    let overall = (1.0 - total_comp as f64 / total_orig as f64) * 100.0;
    println!(
        "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
        "TOTAL", total_orig, total_comp, overall
    );

    assert!(
        overall > 0.0,
        "L1 response: expected some savings, got {:.1}%",
        overall
    );
}

#[test]
fn bench_l1_stash_schema() {
    println!("\n=== L1: SchemaCompressor + Stash (reversible) ===");
    let store = Arc::new(InMemoryStore::new());
    let compressor = SchemaCompressor::new().with_stash_store(store.clone());

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    for name in ALL_SCHEMA_FIXTURES {
        let schema = load_schema(name);
        let compressed = compressor.compress(&schema);
        let orig = byte_len(&schema);
        let comp = byte_len(&compressed);
        let saved = (1.0 - comp as f64 / orig as f64) * 100.0;
        println!(
            "  {:<35} {:>6} -> {:>6} ({:.1}% saved, stash={})",
            name,
            orig,
            comp,
            saved,
            store.len()
        );
        total_orig += orig;
        total_comp += comp;
    }

    let overall = (1.0 - total_comp as f64 / total_orig as f64) * 100.0;
    println!(
        "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
        "TOTAL", total_orig, total_comp, overall
    );

    assert!(store.len() > 0, "L1 stash schema: expected stashed entries");
}

#[test]
fn bench_l1_stash_response() {
    println!("\n=== L1: ResponseCompressor + Stash (reversible) ===");
    let store = Arc::new(InMemoryStore::new());
    let compressor = ResponseCompressor::new().with_stash_store(store.clone());

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    // Use a fixture with 35 items so the array tail (items 33-35) is stashed
    // reversibly. github_issues.json has only 5 items, which never triggers
    // array truncation, so stash=0 and the lossless label would be incorrect.
    for name in RESPONSE_STASH_FIXTURES {
        let response = load_response(name);
        let compressed = compressor.compress(&response);
        let orig = byte_len(&response);
        let comp = byte_len(&compressed);
        let saved = (1.0 - comp as f64 / orig as f64) * 100.0;
        println!(
            "  {:<35} {:>6} -> {:>6} ({:.1}% saved, stash={})",
            name,
            orig,
            comp,
            saved,
            store.len()
        );
        total_orig += orig;
        total_comp += comp;
    }

    let overall = (1.0 - total_comp as f64 / total_orig as f64) * 100.0;
    println!(
        "  {:<35} {:>6} -> {:>6} ({:.1}% saved)",
        "TOTAL", total_orig, total_comp, overall
    );

    assert!(
        store.len() > 0,
        "L1 stash response: expected stash entries from array truncation (fixture has 35 items, limit=32)"
    );
    assert!(overall > 0.0, "L1 stash response: expected some savings");

    // Verify round-trip: the compressed output must contain a <<tokenless:…>> marker
    // and the store must return the exact original dropped payload for that key.
    let response = load_response(RESPONSE_STASH_FIXTURES[0]);
    let items = response["items"]
        .as_array()
        .expect("fixture must have items array");
    let dropped_original = Value::Array(items[32..].to_vec());

    let compressed = compressor.compress(&response);
    let compressed_str = serde_json::to_string(&compressed).unwrap();
    let hash = tokenless_ccr::extract_hash(&compressed_str).expect(
        "compressed output must contain a <<tokenless:KEY>> marker for the truncated array tail",
    );
    let retrieved = store
        .retrieve(hash)
        .expect("store retrieve must not error")
        .expect("stash key must be present and non-expired");
    let tail: serde_json::Value =
        serde_json::from_str(&retrieved).expect("stashed payload must be valid JSON");
    assert!(
        tail.is_array(),
        "stashed payload must be an array of the dropped items, got: {:?}",
        tail
    );
    assert!(
        !tail.as_array().unwrap().is_empty(),
        "stashed array must be non-empty"
    );
    assert_eq!(
        tail, dropped_original,
        "retrieved tail must match the original dropped items exactly"
    );
}

#[test]
fn bench_l2_schema_response_pipeline() {
    println!("\n=== L2: Schema + Response pipeline ===");
    let schema_compressor = SchemaCompressor::new();
    let response_compressor = ResponseCompressor::new();

    for schema_name in ALL_SCHEMA_FIXTURES {
        let schema = load_schema(schema_name);
        let compressed_schema = schema_compressor.compress(&schema);
        let schema_saved =
            (1.0 - byte_len(&compressed_schema) as f64 / byte_len(&schema) as f64) * 100.0;

        for resp_name in RESPONSE_FIXTURES {
            let response = load_response(resp_name);
            let compressed_response = response_compressor.compress(&response);
            let resp_saved =
                (1.0 - byte_len(&compressed_response) as f64 / byte_len(&response) as f64) * 100.0;

            let combined_orig = byte_len(&schema) + byte_len(&response);
            let combined_comp = byte_len(&compressed_schema) + byte_len(&compressed_response);
            let combined_saved = (1.0 - combined_comp as f64 / combined_orig as f64) * 100.0;

            println!(
                "  {} + {}: schema {:.1}% | response {:.1}% | combined {:.1}% ({} -> {})",
                schema_name,
                resp_name,
                schema_saved,
                resp_saved,
                combined_saved,
                combined_orig,
                combined_comp
            );
        }
    }
}

#[test]
fn bench_l3_schema_response_stash_pipeline() {
    println!("\n=== L3: Schema + Response + Stash pipeline ===");
    let store = Arc::new(InMemoryStore::new());
    let schema_compressor = SchemaCompressor::new().with_stash_store(store.clone());
    let response_compressor = ResponseCompressor::new().with_stash_store(store.clone());

    for schema_name in ALL_SCHEMA_FIXTURES {
        let schema = load_schema(schema_name);
        let compressed_schema = schema_compressor.compress(&schema);

        // Use the 35-item stashable response fixture so the response compressor
        // actually writes stash entries. The 5-item fixture never triggers
        // array truncation and would make store.len()>0 come from schema only.
        for resp_name in RESPONSE_STASH_FIXTURES {
            let response = load_response(resp_name);
            let compressed_response = response_compressor.compress(&response);

            let combined_orig = byte_len(&schema) + byte_len(&response);
            let combined_comp = byte_len(&compressed_schema) + byte_len(&compressed_response);
            let combined_saved = (1.0 - combined_comp as f64 / combined_orig as f64) * 100.0;

            println!(
                "  {} + {}: combined {:.1}% ({} -> {}, stash={})",
                schema_name,
                resp_name,
                combined_saved,
                combined_orig,
                combined_comp,
                store.len()
            );
        }
    }

    assert!(
        store.len() > 0,
        "L3: expected stashed entries for reversibility"
    );
}

#[test]
fn bench_l4_full_pipeline_aggregate() {
    println!("\n=== L4: Full pipeline aggregate benchmark ===");
    println!("Schema fixtures + response fixtures with all optimizations enabled");

    let store = Arc::new(InMemoryStore::new());
    let schema_compressor = SchemaCompressor::new().with_stash_store(store.clone());
    let response_compressor = ResponseCompressor::new().with_stash_store(store.clone());

    let mut grand_orig = 0usize;
    let mut grand_comp = 0usize;

    println!("  --- Schema compression ---");
    for name in ALL_SCHEMA_FIXTURES {
        let schema = load_schema(name);
        let compressed = schema_compressor.compress(&schema);
        let orig = byte_len(&schema);
        let comp = byte_len(&compressed);
        println!(
            "    {:<35} {:>6} -> {:>6} ({:.1}%)",
            name,
            orig,
            comp,
            (1.0 - comp as f64 / orig as f64) * 100.0
        );
        grand_orig += orig;
        grand_comp += comp;
    }

    println!("  --- Response compression ---");
    for name in RESPONSE_FIXTURES {
        let response = load_response(name);
        let compressed = response_compressor.compress(&response);
        let orig = byte_len(&response);
        let comp = byte_len(&compressed);
        println!(
            "    {:<35} {:>6} -> {:>6} ({:.1}%)",
            name,
            orig,
            comp,
            (1.0 - comp as f64 / orig as f64) * 100.0
        );
        grand_orig += orig;
        grand_comp += comp;
    }

    let overall = (1.0 - grand_comp as f64 / grand_orig as f64) * 100.0;
    println!(
        "\n  GRAND TOTAL: {} -> {} bytes ({:.1}% saved)",
        grand_orig, grand_comp, overall
    );
    println!(
        "  Stash entries: {} (schema fields stashed reversibly; response savings include lossy field removal)",
        store.len()
    );
    println!("  L4 layers: SchemaCompressor + ResponseCompressor + CCR Stash");
    println!(
        "  Note: TOON encoding (4th layer) requires external binary, not included in this Rust bench"
    );

    assert!(overall > 0.0, "L4: expected positive overall savings");
    assert!(
        store.len() > 0,
        "L4: stash should contain reversible entries"
    );
}
