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

//! Asset sanity checks: every shipped sample, command spec, and probe file
//! must parse with all required fields — a malformed asset should fail here,
//! not mid-run on the remote host.

use std::path::PathBuf;
use tokenless_l2_bench::l2::Category;
use tokenless_l2_bench::l2::samples::{
    load_code_samples, load_command_specs, load_json_samples, load_probe_questions, probe_file_stem,
};

fn l2_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

#[test]
fn json_samples_load_with_content_and_ground_truth() {
    let samples = load_json_samples(&l2_dir()).expect("load json samples");
    assert_eq!(samples.len(), 3);
    for s in &samples {
        assert_eq!(s.category, Category::Json);
        assert!(!s.id.is_empty());
        assert!(!s.content.is_empty(), "sample {} has empty content", s.id);
        assert!(
            !s.ground_truth.is_empty(),
            "sample {} has no ground truth",
            s.id
        );
        // json samples must carry valid wire-form JSON — the tokenless side
        // parses them before compressing.
        serde_json::from_str::<serde_json::Value>(&s.content)
            .unwrap_or_else(|e| panic!("sample {} is not valid JSON: {e}", s.id));
    }
    // The canonical fixture arrives via content_path and must resolve.
    assert!(samples.iter().any(|s| s.id == "tool_response_main"));
}

#[test]
fn code_samples_load_with_content_and_ground_truth() {
    let samples = load_code_samples(&l2_dir()).expect("load code samples");
    assert_eq!(samples.len(), 9);
    for s in &samples {
        assert_eq!(s.category, Category::Code);
        assert!(!s.content.is_empty(), "sample {} has empty content", s.id);
        assert!(
            !s.ground_truth.is_empty(),
            "sample {} has no ground truth",
            s.id
        );
    }
}

#[test]
fn static_samples_retain_their_own_ground_truth() {
    // Every ground-truth item must hold against the uncompressed content,
    // otherwise the retention metric starts from a broken baseline.
    let mut all = load_json_samples(&l2_dir()).expect("json");
    all.extend(load_code_samples(&l2_dir()).expect("code"));
    for s in &all {
        let result =
            tokenless_l2_bench::l2::retention::check(&s.ground_truth, &s.content).expect("check");
        assert_eq!(
            result.passed, result.total,
            "sample {} loses items pre-compression: {:?}",
            s.id, result.failures
        );
    }
}

#[test]
fn command_specs_are_well_formed() {
    let specs = load_command_specs(&l2_dir()).expect("load command specs");
    assert_eq!(specs.len(), 6);
    for spec in &specs {
        assert!(!spec.id.is_empty());
        assert!(!spec.argv.is_empty(), "spec {} has empty argv", spec.id);
        assert_eq!(spec.ground_truth_source, "dynamic", "spec {}", spec.id);
        assert!(!spec.cwd_rel.is_empty(), "spec {}", spec.id);
        let category = spec.parsed_category().expect("category parses");
        assert!(
            category.is_dynamic(),
            "spec {} must be a dynamic category",
            spec.id
        );
    }
    // Ids must be unique — task simulations reference them by name.
    let mut ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), specs.len(), "duplicate spec ids");
}

#[test]
fn every_category_has_a_probe_file_with_enough_questions() {
    for category in Category::ALL {
        let stem = probe_file_stem(category);
        let questions = load_probe_questions(&l2_dir(), stem)
            .unwrap_or_else(|e| panic!("probes/{stem}.json failed to load: {e}"));
        assert!(
            questions.len() >= 8,
            "probes/{stem}.json has only {} questions, need >= 8",
            questions.len()
        );
        for (i, q) in questions.iter().enumerate() {
            assert!(!q.question.trim().is_empty(), "{stem}[{i}] question empty");
            assert!(
                !q.expected_contains.trim().is_empty(),
                "{stem}[{i}] expected_contains empty"
            );
        }
    }
}

/// `JsonCompressor` drops noise fields (`logs`, `debug`, `trace`, ...)
/// by design, so JSON ground truth must never reference their contents —
/// otherwise retention would penalise the compressor for doing its job
/// (the miscalibration behind an early 40/60-vs-60/60 smoke reading).
#[test]
fn json_ground_truth_never_references_droppable_noise() {
    use tokenless_l2_bench::l2::GroundTruth;

    let noise_markers = ["log entry", "logs", "debug", "trace", "stacktrace"];
    let samples = load_json_samples(&l2_dir()).expect("load json samples");
    for sample in &samples {
        for item in &sample.ground_truth {
            let needle = match item {
                GroundTruth::Substring(s) => s.as_str(),
                GroundTruth::Pattern { regex } => regex.as_str(),
            };
            let lower = needle.to_lowercase();
            for marker in noise_markers {
                assert!(
                    !lower.contains(marker),
                    "{}: ground truth {needle:?} references droppable noise ({marker})",
                    sample.id
                );
            }
        }
    }
}
