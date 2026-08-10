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

//! Sample loading: static samples (`assets/samples/*.json`), command specs and
//! semantic-probe question sets, plus dynamic ground-truth extraction for
//! live-captured command output.
//!
//! Static JSON/code samples ship their ground truth in the asset file;
//! command/grep/diff samples cannot (their output depends on the repository
//! state at run time), so their ground truth is extracted from the raw
//! output of the very run being measured — guaranteeing the retention check
//! asserts facts that were actually present.

use crate::l2::{Category, GroundTruth, L2Error, SampleRecord};
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Maximum dynamically-extracted ground-truth items per sample. Keeps the
/// retention denominator comparable across samples of very different sizes.
const MAX_DYNAMIC_ITEMS: usize = 5;

#[derive(Deserialize)]
struct SampleFile {
    category: String,
    samples: Vec<RawSample>,
}

#[derive(Deserialize)]
struct RawSample {
    id: String,
    #[serde(default)]
    content_json: Option<serde_json::Value>,
    #[serde(default)]
    content_path: Option<String>,
    #[serde(default)]
    content_lines: Option<Vec<String>>,
    #[serde(default)]
    ground_truth: Vec<GroundTruth>,
}

/// One executable command spec from `samples/command_specs.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandSpec {
    /// Stable id referenced by task simulations and report rows.
    pub id: String,
    /// One of `command` / `grep` / `diff`.
    pub category: String,
    /// Program + args, executed without a shell so quoting stays exact.
    pub argv: Vec<String>,
    /// Working directory relative to the repository root.
    pub cwd_rel: String,
    /// Always `"dynamic"` — kept explicit in the asset so a future static
    /// spec is a deliberate schema change, not an accident.
    pub ground_truth_source: String,
}

impl CommandSpec {
    /// Parses this spec's category string.
    ///
    /// # Errors
    ///
    /// Returns [`L2Error::InvalidSample`] for unknown category names.
    pub fn parsed_category(&self) -> Result<Category, L2Error> {
        Category::parse(&self.category).ok_or_else(|| {
            L2Error::InvalidSample(format!(
                "command spec {:?} has unknown category {:?}",
                self.id, self.category
            ))
        })
    }
}

/// One semantic-probe question with its literal answer check.
#[derive(Debug, Clone, Deserialize)]
pub struct ProbeQuestion {
    pub question: String,
    /// Substring the model's answer must contain to count as correct.
    pub expected_contains: String,
}

/// Loads the static `json` samples from `samples/json_api.json`.
///
/// `content_path` entries are resolved relative to the sample file itself so
/// the canonical fixture stays byte-identical to the L0/L1 suites.
///
/// # Errors
///
/// Fails on unreadable/malformed asset files or samples missing content.
pub fn load_json_samples(l2_dir: &Path) -> Result<Vec<SampleRecord>, L2Error> {
    load_static(l2_dir, "samples/json_api.json", Category::Json)
}

/// Loads the static `code` samples from `samples/source_code.json`.
///
/// # Errors
///
/// Fails on unreadable/malformed asset files or samples missing content.
pub fn load_code_samples(l2_dir: &Path) -> Result<Vec<SampleRecord>, L2Error> {
    load_static(l2_dir, "samples/source_code.json", Category::Code)
}

fn load_static(l2_dir: &Path, rel: &str, expect: Category) -> Result<Vec<SampleRecord>, L2Error> {
    let path = l2_dir.join(rel);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| L2Error::InvalidSample(format!("cannot read {}: {e}", path.display())))?;
    let file: SampleFile = serde_json::from_str(&text)?;
    if file.category != expect.name() {
        return Err(L2Error::InvalidSample(format!(
            "{rel} declares category {:?}, expected {:?}",
            file.category,
            expect.name()
        )));
    }
    let base = path.parent().map(PathBuf::from).unwrap_or_default();
    file.samples
        .into_iter()
        .map(|raw| {
            let content = resolve_content(&raw, &base)?;
            Ok(SampleRecord {
                id: raw.id,
                category: expect,
                content,
                ground_truth: raw.ground_truth,
            })
        })
        .collect()
}

// Content precedence: content_lines > content_json > content_path. Exactly
// one is expected per sample; the precedence only matters if an asset is
// over-specified, and then the most explicit (inline) form wins.
fn resolve_content(raw: &RawSample, base: &Path) -> Result<String, L2Error> {
    if let Some(lines) = &raw.content_lines {
        return Ok(lines.join("\n"));
    }
    if let Some(value) = &raw.content_json {
        return Ok(serde_json::to_string(value)?);
    }
    if let Some(rel) = &raw.content_path {
        let p = base.join(rel);
        let text = std::fs::read_to_string(&p).map_err(|e| {
            L2Error::InvalidSample(format!(
                "sample {:?}: cannot read content_path {}: {e}",
                raw.id,
                p.display()
            ))
        })?;
        // Re-serialize compactly when the referenced file is JSON so token
        // counts are measured on wire form, matching the L0/L1 convention.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            return Ok(serde_json::to_string(&v)?);
        }
        return Ok(text);
    }
    Err(L2Error::InvalidSample(format!(
        "sample {:?} has no content_lines/content_json/content_path",
        raw.id
    )))
}

/// Loads `samples/command_specs.json`.
///
/// # Errors
///
/// Fails on unreadable/malformed spec files.
pub fn load_command_specs(l2_dir: &Path) -> Result<Vec<CommandSpec>, L2Error> {
    let path = l2_dir.join("samples/command_specs.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| L2Error::InvalidSample(format!("cannot read {}: {e}", path.display())))?;
    Ok(serde_json::from_str(&text)?)
}

/// Loads the probe question set for one category from `probes/<name>.json`.
///
/// # Errors
///
/// Fails on unreadable/malformed probe files.
pub fn load_probe_questions(l2_dir: &Path, file_stem: &str) -> Result<Vec<ProbeQuestion>, L2Error> {
    let path = l2_dir.join(format!("probes/{file_stem}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| L2Error::InvalidSample(format!("cannot read {}: {e}", path.display())))?;
    Ok(serde_json::from_str(&text)?)
}

/// Probe file stem for a category (matches `assets/probes/*.json` names).
pub fn probe_file_stem(category: Category) -> &'static str {
    match category {
        Category::Json => "json_api",
        Category::Command => "command_output",
        Category::Grep => "grep_search",
        Category::Code => "source_code",
        Category::Diff => "git_diff",
    }
}

/// Extracts ground truth from live command output.
///
/// Patterns per category (first `MAX_DYNAMIC_ITEMS` hits, deduplicated):
/// * `command` — commit hashes at line start (`git log --oneline`,
///   `git show`), so retention asserts the hashes the agent would act on;
/// * `grep` — `file:line` prefixes from `rg -n` output;
/// * `diff` — file names from `diff --git a/<path>` headers.
///
/// Static categories return an empty vector: their truth ships in the asset.
///
/// # Errors
///
/// Returns [`L2Error::Regex`] only if a built-in pattern is invalid, which
/// the `l2_retention` test guards against.
pub fn extract_dynamic_ground_truth(
    category: Category,
    raw_output: &str,
) -> Result<Vec<GroundTruth>, L2Error> {
    let pattern = match category {
        Category::Command => r"(?m)^(?:commit )?([0-9a-f]{7,40})\b",
        Category::Grep => r"(?m)^([^:\s][^:\n]*:\d+)[:-]",
        Category::Diff => r"(?m)^diff --git a/(\S+)",
        Category::Json | Category::Code => return Ok(Vec::new()),
    };
    let re = Regex::new(pattern)?;
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for cap in re.captures_iter(raw_output) {
        if let Some(m) = cap.get(1) {
            let s = m.as_str().to_string();
            if seen.insert(s.clone()) {
                items.push(GroundTruth::Substring(s));
                if items.len() >= MAX_DYNAMIC_ITEMS {
                    break;
                }
            }
        }
    }
    Ok(items)
}
