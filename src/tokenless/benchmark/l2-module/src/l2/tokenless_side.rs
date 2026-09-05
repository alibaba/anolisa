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

//! Tokenless side: direct in-process calls to `JsonCompressor` — the same
//! code path the L1 suite measures, so L2
//! numbers stay comparable with L1 rather than adding CLI subprocess noise.
//!
//! Latency basis: **in-process** (`Instant` around the compress call only).

use crate::l2::{Category, L2Error};
use serde_json::{Value, json};
use std::time::Instant;
use tokenless_compressors::{JsonCompressionContext, JsonCompressor, RecoveryMethod};

/// Latency-basis label stamped on every tokenless-side result row.
pub const LATENCY_BASIS: &str = "in-process";

/// Output of one in-process compression call.
#[derive(Debug, Clone)]
pub struct TokenlessOutput {
    /// Compact-JSON wire form of the compressed value.
    pub compressed: String,
    /// Pure compression time in seconds (serialization excluded).
    pub latency_s: f64,
}

/// Compresses `content` with the tokenless `JsonCompressor`.
///
/// JSON samples are parsed and compressed as-is. Non-JSON text (source code,
/// command output) is wrapped as `{"content": text}` — the engine's generic
/// fallback envelope — because the compressor operates on JSON values; the
/// wrapper is part of the measured payload on BOTH the before and after
/// side, so it cannot inflate the compression rate.
///
/// # Errors
///
/// Returns [`L2Error::InvalidSample`] when a `json`-category sample fails to
/// parse, and [`L2Error::Json`] if the compressed value cannot serialize.
pub fn compress(category: Category, content: &str) -> Result<TokenlessOutput, L2Error> {
    let value: Value = if category == Category::Json {
        serde_json::from_str(content)
            .map_err(|e| L2Error::InvalidSample(format!("json sample is not valid JSON: {e}")))?
    } else {
        json!({ "content": content })
    };

    let input = serde_json::to_string(&value)?;
    let compressor = JsonCompressor::default();
    // Matches L1's default CLI entry point; with no Stash, this does not
    // affect output or compression metrics.
    let recovery = RecoveryMethod::Shell;
    let context = JsonCompressionContext {
        recovery: &recovery,
        stash: None,
        allow_toon: false,
        preserve_top_level_shape: false,
        min_toon_chars: usize::MAX,
        allow_unrecoverable: true,
    };
    // Time only the compress call: the wire-form serialization below is
    // measurement plumbing, not engine work.
    let start = Instant::now();
    let compressed = compressor
        .compress(&input, &context)
        .map_err(|error| L2Error::InvalidSample(error.to_string()))?;
    let latency_s = start.elapsed().as_secs_f64();

    Ok(TokenlessOutput {
        compressed: compressed.output,
        latency_s,
    })
}

/// The wire form the tokenless side counts "before" tokens on.
///
/// For JSON samples this is the compacted original; for text samples it is
/// the same `{"content": ...}` envelope handed to the compressor, keeping
/// before/after counts symmetrical.
///
/// # Errors
///
/// Same failure modes as [`compress`].
pub fn wire_before(category: Category, content: &str) -> Result<String, L2Error> {
    if category == Category::Json {
        let value: Value = serde_json::from_str(content)
            .map_err(|e| L2Error::InvalidSample(format!("json sample is not valid JSON: {e}")))?;
        Ok(serde_json::to_string(&value)?)
    } else {
        Ok(serde_json::to_string(&json!({ "content": content }))?)
    }
}
