//! CLI-level tests for SLS ops JSONL logging.
//!
//! Each `skillfs` subcommand appends one ops record to the deployment-owned
//! ops log. Tests point the writer at a temp file via `SKILLFS_SLS_OPS_PATH`
//! (validated to live under `/tmp/`) and assert the appended record.

use std::path::Path;
use std::process::Command;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_skillfs")
}

const VALID_SKILL: &str = r#"---
name: good-skill
description: A valid skill
version: "1.0"
---
# Good Skill

This skill works correctly.
"#;

/// SKILL.md with invalid YAML frontmatter → ParseStatus::Error.
const ERROR_SKILL: &str = r#"---
name: [invalid yaml
  broken: {{{}
---
Body text.
"#;

fn create_skill_dir(parent: &Path, name: &str, content: &str) {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(dir.join("SKILL.md"), content).expect("write SKILL.md");
}

/// Create the deployment-owned ops log under /tmp so the CLI (which never
/// creates the file) will append to it.
fn make_ops_log(dir: &Path) -> std::path::PathBuf {
    let ops_log = dir.join("skillfs-ops.jsonl");
    std::fs::File::create(&ops_log).expect("pre-create ops log");
    ops_log
}

/// Read all JSONL records from the ops log.
fn read_records(ops_log: &Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(ops_log).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect()
}

fn run_skillfs(args: &[&str], ops_log: &Path) -> std::process::Output {
    Command::new(bin_path())
        .args(args)
        // Temp dirs live under /tmp on Linux, an allowed override prefix.
        .env("SKILLFS_SLS_OPS_PATH", ops_log)
        .output()
        .expect("invoke skillfs")
}

#[test]
fn list_appends_ops_record_on_success() {
    // /tmp-based temp dir required so the override prefix check accepts it.
    let dir = tempfile::tempdir_in("/tmp").expect("tempdir");
    let ops_log = make_ops_log(dir.path());

    let source = tempfile::tempdir_in("/tmp").expect("source tempdir");
    create_skill_dir(source.path(), "good-skill", VALID_SKILL);

    let out = run_skillfs(&["list", source.path().to_str().unwrap()], &ops_log);
    assert!(out.status.success(), "list should succeed");

    let records = read_records(&ops_log);
    assert_eq!(records.len(), 1, "expected one ops record");
    assert_eq!(records[0]["component.name"], "skillfs");
    assert_eq!(records[0]["component.agent_name"], "cli");
    assert_eq!(records[0]["ops_name"], "list");
    assert_eq!(records[0]["err_reason"], "none");
    assert!(
        !records[0]["component.version"].as_str().unwrap().is_empty(),
        "component.version must be populated"
    );
}

#[test]
fn validate_appends_ops_record_on_success() {
    let dir = tempfile::tempdir_in("/tmp").expect("tempdir");
    let ops_log = make_ops_log(dir.path());

    let source = tempfile::tempdir_in("/tmp").expect("source tempdir");
    create_skill_dir(source.path(), "good-skill", VALID_SKILL);

    let out = run_skillfs(&["validate", source.path().to_str().unwrap()], &ops_log);
    assert!(out.status.success(), "validate should succeed");

    let records = read_records(&ops_log);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["ops_name"], "validate");
    assert_eq!(records[0]["err_reason"], "none");
}

#[test]
fn validate_appends_record_before_nonzero_exit() {
    let dir = tempfile::tempdir_in("/tmp").expect("tempdir");
    let ops_log = make_ops_log(dir.path());

    let source = tempfile::tempdir_in("/tmp").expect("source tempdir");
    create_skill_dir(source.path(), "bad-yaml", ERROR_SKILL);

    let out = run_skillfs(&["validate", source.path().to_str().unwrap()], &ops_log);
    assert!(
        !out.status.success(),
        "validate with a bad skill must exit non-zero"
    );

    let records = read_records(&ops_log);
    assert_eq!(records.len(), 1, "record must be written before exiting");
    assert_eq!(records[0]["ops_name"], "validate");
    assert_ne!(
        records[0]["err_reason"], "none",
        "failed validation must set a non-none err_reason"
    );
}

#[test]
fn classify_dry_run_appends_ops_record_on_success() {
    let dir = tempfile::tempdir_in("/tmp").expect("tempdir");
    let ops_log = make_ops_log(dir.path());

    let source = tempfile::tempdir_in("/tmp").expect("source tempdir");
    create_skill_dir(source.path(), "good-skill", VALID_SKILL);

    let out = run_skillfs(
        &["classify", source.path().to_str().unwrap(), "--dry-run"],
        &ops_log,
    );
    assert!(out.status.success(), "classify --dry-run should succeed");

    let records = read_records(&ops_log);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["ops_name"], "classify");
    assert_eq!(records[0]["err_reason"], "none");
}

#[test]
fn missing_ops_log_is_not_created_and_command_succeeds() {
    let dir = tempfile::tempdir_in("/tmp").expect("tempdir");
    // Do NOT create the ops log — the CLI must skip writing without creating it.
    let ops_log = dir.path().join("absent-ops.jsonl");

    let source = tempfile::tempdir_in("/tmp").expect("source tempdir");
    create_skill_dir(source.path(), "good-skill", VALID_SKILL);

    let out = run_skillfs(&["list", source.path().to_str().unwrap()], &ops_log);
    assert!(
        out.status.success(),
        "list should still succeed when the ops log is absent"
    );
    assert!(
        !ops_log.exists(),
        "CLI must not create the missing ops log file"
    );
}
