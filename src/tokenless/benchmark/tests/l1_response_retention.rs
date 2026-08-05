// Copyright 2026 Alibaba Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Response compression quality — information retention & reversibility.
//!
//! Verifies truncation, field dropping, and the reversible stash round-trip
//! preserve exactly what they should. 11 tests.

use serde_json::{Value, json};
use std::sync::Arc;
use tokenless_ccr::{InMemoryStore, StashStore, extract_hash};
use tokenless_schema::ResponseCompressor;

#[test]
fn string_truncation_adds_marker() {
    let compressor = ResponseCompressor::new().with_truncate_strings_at(20);
    let long = "This is a very long string that should be truncated";
    let out = compressor.compress(&json!(long));
    let s = out.as_str().unwrap();
    assert!(s.contains("… (truncated)"));
}

#[test]
fn string_within_limit_is_untouched() {
    let compressor = ResponseCompressor::new();
    let short = "short value";
    let out = compressor.compress(&json!(short));
    assert_eq!(out, json!(short));
}

#[test]
fn array_truncation_default_limit_is_32() {
    let compressor = ResponseCompressor::new();
    let arr: Vec<i32> = (1..=50).collect();
    let out = compressor.compress(&json!(arr));
    // 32 kept items + 1 truncation marker.
    assert_eq!(out.as_array().unwrap().len(), 33);
}

#[test]
fn array_truncation_custom_limit() {
    let compressor = ResponseCompressor::new().with_truncate_arrays_at(3);
    let arr: Vec<i32> = (1..=10).collect();
    let out = compressor.compress(&json!(arr));
    let a = out.as_array().unwrap();
    assert_eq!(a.len(), 4);
    assert!(a[3].as_str().unwrap().contains("truncated"));
}

#[test]
fn debug_family_fields_are_dropped() {
    let compressor = ResponseCompressor::new();
    let obj = json!({
        "data": "keep",
        "debug": "x", "trace": "x", "traces": "x",
        "stack": "x", "stacktrace": "x", "logs": "x", "logging": "x"
    });
    let out = compressor.compress(&obj);
    let o = out.as_object().unwrap();
    assert_eq!(
        o["data"],
        json!("keep"),
        "preserved field must retain its value"
    );
    for k in [
        "debug",
        "trace",
        "traces",
        "stack",
        "stacktrace",
        "logs",
        "logging",
    ] {
        assert!(!o.contains_key(k), "{k} should be dropped");
    }
}

#[test]
fn nulls_are_dropped_by_default() {
    let compressor = ResponseCompressor::new();
    let out = compressor.compress(&json!({ "name": "t", "value": null, "count": 5 }));
    let o = out.as_object().unwrap();
    assert_eq!(o["name"], json!("t"), "name must retain its value");
    assert_eq!(o["count"], json!(5), "count must retain its value");
    assert!(!o.contains_key("value"));
}

#[test]
fn empty_fields_are_dropped_by_default() {
    let compressor = ResponseCompressor::new();
    let out = compressor.compress(&json!({
        "keep": "data", "es": "", "ea": [], "eo": {}
    }));
    let o = out.as_object().unwrap();
    assert_eq!(
        o["keep"],
        json!("data"),
        "preserved field must retain its value"
    );
    assert!(!o.contains_key("es"));
    assert!(!o.contains_key("ea"));
    assert!(!o.contains_key("eo"));
}

#[test]
fn custom_drop_field_is_removed() {
    let mut compressor = ResponseCompressor::new();
    compressor.add_drop_field("internal");
    let out = compressor.compress(&json!({ "keep": 1, "internal": "secret" }));
    let o = out.as_object().unwrap();
    assert_eq!(o["keep"], json!(1), "preserved field must retain its value");
    assert!(!o.contains_key("internal"));
}

#[test]
fn stash_round_trip_recovers_dropped_items_verbatim() {
    let store = Arc::new(InMemoryStore::new());
    let compressor = ResponseCompressor::new()
        .with_truncate_arrays_at(2)
        .with_stash_store(store.clone());
    let arr = json!(["a", "b", "c", "d", "e"]);
    let out = compressor.compress(&arr);
    let a = out.as_array().unwrap();
    assert_eq!(a[0], json!("a"));
    assert_eq!(a[1], json!("b"));
    let hash = extract_hash(a.last().unwrap().as_str().unwrap()).unwrap();
    let recovered: Vec<String> =
        serde_json::from_str(&store.retrieve(hash).unwrap().unwrap()).unwrap();
    assert_eq!(recovered, vec!["c", "d", "e"]);
    assert_eq!(compressor.stash_writes(), 1);
}

#[test]
fn stashed_items_keep_fields_the_compressor_would_strip() {
    let store = Arc::new(InMemoryStore::new());
    let compressor = ResponseCompressor::new()
        .with_truncate_arrays_at(1)
        .with_stash_store(store.clone());
    let arr = json!([
        { "id": 1, "debug": "stripped in kept item" },
        { "id": 2, "debug": "survives in stash" }
    ]);
    let out = compressor.compress(&arr);
    let a = out.as_array().unwrap();
    // Kept item is compressed: debug stripped.
    assert!(a[0].get("debug").is_none());
    let hash = extract_hash(a.last().unwrap().as_str().unwrap()).unwrap();
    let recovered: Vec<Value> =
        serde_json::from_str(&store.retrieve(hash).unwrap().unwrap()).unwrap();
    // Stashed item is verbatim: debug preserved.
    assert_eq!(recovered[0]["debug"], json!("survives in stash"));
}

#[test]
fn no_marker_when_array_within_limit() {
    let store = Arc::new(InMemoryStore::new());
    let compressor = ResponseCompressor::new()
        .with_truncate_arrays_at(10)
        .with_stash_store(store.clone());
    let out = compressor.compress(&json!([1, 2, 3]));
    assert!(out.as_array().unwrap().iter().all(|v| v.is_number()));
    assert_eq!(store.len(), 0);
    assert_eq!(compressor.stash_writes(), 0);
}
