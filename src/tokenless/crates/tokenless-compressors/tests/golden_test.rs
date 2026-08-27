//! Fixture goldens for the two-stage terminal-cleanup → build-log chain.
//!
//! Each `<name>.txt` input has a committed `<name>.expected.txt` baseline.
//! To re-baseline after an intentional engine change:
//! `REGEN_GOLDENS=1 cargo test -p tokenless-compressors --test golden_test`
//! then review the diff.

use std::fs;
use std::path::PathBuf;

use tokenless_ccr::{InMemoryStore, StashStore, compute_key, is_valid_hash, marker_for};
use tokenless_compressors::{BuildLogMode, BuildLogOutcome, clean_terminal, compress_log};

const FIXTURES: &[&str] = &[
    "cargo_success",
    "cargo_failure",
    "npm_success",
    "npm_failure",
    "pytest_success",
    "pytest_failure",
    "go_success",
    "go_failure",
    "shell_success",
    "shell_failure",
];

/// Task facts (§6.1) that must survive compression verbatim: error identity,
/// file:line references, exit state, summaries.
const PROBES: &[(&str, &[&str])] = &[
    (
        "cargo_success",
        &["Finished `release` profile [optimized] target(s) in 42.18s"],
    ),
    (
        "cargo_failure",
        &[
            "error[E0308]",
            "--> src/main.rs:12:5",
            "error: could not compile `app` (bin \"app\") due to 1 previous error",
        ],
    ),
    (
        "npm_success",
        &[
            "added 41 packages, and audited 42 packages in 3s",
            "found 0 vulnerabilities",
        ],
    ),
    (
        "npm_failure",
        &[
            "npm ERR! code E404",
            "'left-padd@^1.0.0' is not in this registry.",
        ],
    ),
    ("pytest_success", &["38 passed in 1.23s"]),
    (
        "pytest_failure",
        &[
            "FAILED tests/test_math.py::test_answer - assert 3 == 4",
            "E       assert 3 == 4",
            "1 failed, 33 passed in 2.14s",
        ],
    ),
    ("go_success", &["ok  \tgithub.com/acme/app\t0.123s"]),
    (
        "go_failure",
        &[
            "./main.go:10:2: undefined: fooBar",
            "FAIL\tgithub.com/acme/app [build failed]",
        ],
    ),
    (
        "shell_success",
        &["make: Leaving directory '/work/project'", "Exit code: 0"],
    ),
    (
        "shell_failure",
        &[
            "src/util.c:42:5: error: 'foo' undeclared (first use in this function)",
            "make: *** [Makefile:12: build/util.o] Error 1",
            "Exit code: 2",
        ],
    ),
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/build_logs")
}

fn load(name: &str) -> String {
    let path = fixtures_dir().join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The production shape: the lossless stage runs first, the lossy stage
/// compresses its output.
fn run_chain(input: &str, store: &InMemoryStore) -> (String, BuildLogOutcome) {
    let cleaned = clean_terminal(input);
    let outcome = compress_log(&cleaned, BuildLogMode::BuildLog, Some(store));
    (cleaned, outcome)
}

#[test]
fn outputs_match_committed_goldens_and_are_deterministic() {
    let regen = std::env::var("REGEN_GOLDENS").is_ok();
    for name in FIXTURES {
        let input = load(&format!("{name}.txt"));
        let (_, first) = run_chain(&input, &InMemoryStore::new());
        let (_, second) = run_chain(&input, &InMemoryStore::new());
        assert_eq!(
            first.output, second.output,
            "{name}: non-deterministic output"
        );

        let expected_path = fixtures_dir().join(format!("{name}.expected.txt"));
        if regen {
            fs::write(&expected_path, &first.output).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", expected_path.display()));
        assert_eq!(
            first.output, expected,
            "{name}: output diverged from golden"
        );
    }
}

#[test]
fn markers_are_valid_and_keys_match_payloads() {
    for name in FIXTURES {
        let input = load(&format!("{name}.txt"));
        let store = InMemoryStore::new();
        let (_, outcome) = run_chain(&input, &store);
        for write in &outcome.stash_writes {
            assert!(is_valid_hash(&write.key), "{name}: bad key {}", write.key);
            assert!(
                outcome.output.contains(&marker_for(&write.key)),
                "{name}: marker for {} missing from output",
                write.key
            );
            let payload = store.retrieve(&write.key).unwrap().expect("payload");
            assert_eq!(
                compute_key(payload.as_bytes()),
                write.key,
                "{name}: key mismatch"
            );
        }
    }
}

#[test]
fn reassembly_reproduces_the_lossy_stage_input_byte_exactly() {
    for name in FIXTURES {
        let input = load(&format!("{name}.txt"));
        let store = InMemoryStore::new();
        let (cleaned, outcome) = run_chain(&input, &store);
        let reassembled = reassemble(&outcome.output, &store);
        assert_eq!(reassembled, cleaned, "{name}: reassembly diverged");
    }
}

#[test]
fn task_facts_survive_compression() {
    for (name, probes) in PROBES {
        let input = load(&format!("{name}.txt"));
        let (_, outcome) = run_chain(&input, &InMemoryStore::new());
        for probe in *probes {
            assert!(
                outcome.output.contains(probe),
                "{name}: task fact missing from output: {probe}"
            );
        }
    }
}

#[test]
fn every_compressing_fixture_actually_saves() {
    // Success fixtures dominated by signal (pytest verbose) legitimately
    // pass through; the rest must shrink.
    for name in FIXTURES {
        let input = load(&format!("{name}.txt"));
        let store = InMemoryStore::new();
        let (cleaned, outcome) = run_chain(&input, &store);
        if outcome.omitted_blocks > 0 {
            assert!(
                outcome.output.chars().count() < cleaned.chars().count(),
                "{name}: markers without net savings"
            );
        } else {
            assert_eq!(
                outcome.output, cleaned,
                "{name}: no blocks but output changed?"
            );
        }
    }
}

/// Reassembly rule: a marker line (with its indented `N× …` template
/// summaries) swaps back to its stashed payload; a repeat annotation expands
/// to `N` more copies of the line above it.
fn reassemble(output: &str, store: &InMemoryStore) -> String {
    let mut result = String::new();
    let mut lines = output.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.contains("run: tokenless retrieve '") {
            let hash = tokenless_ccr::extract_hash(line).expect("marker on omission line");
            while let Some(next) = lines.peek() {
                if is_template_summary(next) {
                    lines.next();
                } else {
                    break;
                }
            }
            result.push_str(&store.retrieve(hash).unwrap().expect("stashed payload"));
        } else if let Some(repeats) = repeat_annotation(line) {
            let prev_start = result[..result.len() - 1]
                .rfind('\n')
                .map(|p| p + 1)
                .unwrap_or(0);
            let prev = result[prev_start..].to_string();
            for _ in 0..repeats {
                result.push_str(&prev);
            }
        } else {
            result.push_str(line);
        }
    }
    result
}

fn is_template_summary(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("  ") else {
        return false;
    };
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && rest[digits..].starts_with('×')
}

fn repeat_annotation(line: &str) -> Option<usize> {
    let rest = line.strip_prefix("[tokenless: previous line repeated ")?;
    let rest = rest
        .strip_suffix(" more times]\n")
        .or_else(|| rest.strip_suffix(" more times]"))?;
    rest.parse().ok()
}
