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

//! Unit checks for ground-truth retention: substring/regex item matching and
//! the dynamic extraction patterns for live command output.

use tokenless_l2_bench::l2::retention::check;
use tokenless_l2_bench::l2::samples::extract_dynamic_ground_truth;
use tokenless_l2_bench::l2::{Category, GroundTruth, L2Error};

#[test]
fn substring_items_are_checked_individually() {
    let truth = vec![
        GroundTruth::Substring("req-8f3a91".to_string()),
        GroundTruth::Substring("E_TIMEOUT".to_string()),
        GroundTruth::Substring("not-in-there".to_string()),
    ];
    let result = check(&truth, "error E_TIMEOUT for request req-8f3a91").expect("check");
    assert_eq!(result.passed, 2);
    assert_eq!(result.total, 3);
    assert_eq!(result.failures.len(), 1);
    assert!(result.failures[0].contains("not-in-there"));
    assert!((result.rate() - 2.0 / 3.0).abs() < 1e-9);
}

#[test]
fn substring_matching_is_case_sensitive() {
    // Ids and error codes are case-significant facts; a case-folded match
    // would hide a real information loss.
    let truth = vec![GroundTruth::Substring("E_TIMEOUT".to_string())];
    let result = check(&truth, "error e_timeout occurred").expect("check");
    assert_eq!(result.passed, 0);
}

#[test]
fn regex_items_match_and_miss() {
    let truth = vec![
        GroundTruth::Pattern {
            regex: r"req-[0-9a-f]{6}".to_string(),
        },
        GroundTruth::Pattern {
            regex: r"^HTTP/2 500$".to_string(),
        },
    ];
    let result = check(&truth, "trace req-8f3a91 finished").expect("check");
    assert_eq!(result.passed, 1);
    assert_eq!(result.total, 2);
}

#[test]
fn empty_ground_truth_scores_full_retention() {
    let result = check(&[], "anything").expect("check");
    assert_eq!(result.total, 0);
    assert_eq!(result.rate(), 1.0);
}

#[test]
fn invalid_regex_surfaces_as_error_not_a_miss() {
    let truth = vec![GroundTruth::Pattern {
        regex: "[unclosed".to_string(),
    }];
    let err = check(&truth, "text").expect_err("bad pattern must error");
    assert!(matches!(err, L2Error::Regex(_)), "got {err:?}");
}

#[test]
fn dynamic_extraction_command_pulls_commit_hashes() {
    let raw = "abc1234 fix parser edge case\n\
               deadbee5 add l2 harness\n\
               abc1234 duplicated line\n";
    let items = extract_dynamic_ground_truth(Category::Command, raw).expect("extract");
    // Duplicates collapse: retention should not double-count one hash.
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], GroundTruth::Substring(s) if s == "abc1234"));
    assert!(matches!(&items[1], GroundTruth::Substring(s) if s == "deadbee5"));
}

#[test]
fn dynamic_extraction_command_accepts_git_show_headers() {
    let raw = "commit 0123456789abcdef0123456789abcdef01234567\nAuthor: someone\n";
    let items = extract_dynamic_ground_truth(Category::Command, raw).expect("extract");
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        GroundTruth::Substring(s) if s == "0123456789abcdef0123456789abcdef01234567"
    ));
}

#[test]
fn dynamic_extraction_grep_pulls_file_line_prefixes() {
    let raw = "src/bin/l2_compare.rs:41:fn main() -> Result<()> {\n\
               src/lib.rs:20:pub fn main_entry() {\n";
    let items = extract_dynamic_ground_truth(Category::Grep, raw).expect("extract");
    assert_eq!(items.len(), 2);
    assert!(matches!(
        &items[0],
        GroundTruth::Substring(s) if s == "src/bin/l2_compare.rs:41"
    ));
}

#[test]
fn dynamic_extraction_diff_pulls_file_names() {
    let raw = "diff --git a/src/l2.rs b/src/l2.rs\n\
               index 111..222 100644\n\
               diff --git a/l2/README.md b/l2/README.md\n";
    let items = extract_dynamic_ground_truth(Category::Diff, raw).expect("extract");
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], GroundTruth::Substring(s) if s == "src/l2.rs"));
    assert!(matches!(&items[1], GroundTruth::Substring(s) if s == "l2/README.md"));
}

#[test]
fn dynamic_extraction_caps_item_count() {
    // 8 distinct hashes in, at most 5 items out — keeps the retention
    // denominator comparable across differently sized outputs.
    let raw: String = (0..8)
        .map(|i| format!("{i}{i}{i}abcd subject {i}\n"))
        .collect();
    let items = extract_dynamic_ground_truth(Category::Command, &raw).expect("extract");
    assert_eq!(items.len(), 5);
}

#[test]
fn dynamic_extraction_is_empty_for_static_categories() {
    for category in [Category::Json, Category::Code] {
        let items = extract_dynamic_ground_truth(category, "abc1234 anything").expect("extract");
        assert!(items.is_empty(), "{category} must ship truth in its asset");
    }
}

#[test]
fn extracted_items_round_trip_through_retention_check() {
    let raw = "abc1234 fix parser\ndeadbee5 add harness\n";
    let items = extract_dynamic_ground_truth(Category::Command, raw).expect("extract");
    // The raw output trivially retains its own extracted facts.
    let full = check(&items, raw).expect("check");
    assert_eq!(full.passed, full.total);
    // A compressed view that drops one hash is caught.
    let partial = check(&items, "abc1234 fix parser").expect("check");
    assert_eq!(partial.passed, 1);
    assert_eq!(partial.total, 2);
}
