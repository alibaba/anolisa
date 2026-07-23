//! Subprocess wire-contract coverage for `uninstall --dry-run --json`.
//!
//! Plain uninstall runs the planner pipeline: for an absent component the
//! dry-run reports the same refusal as a real run would (an error envelope,
//! exit 2), so previews never disagree with reality. `--purge` keeps the
//! legacy plan view (#1471): a unified `data.dry_run`, plan fields flat
//! under `data`. These tests drive the compiled binary and assert the full
//! envelope, which in-crate unit tests cannot cover.

use std::path::Path;
use std::process::Output;

mod common;

/// Run the CLI and parse its stdout as a JSON envelope, asserting `expected`
/// as the exit code.
fn run_json(arguments: &[&str], expected: i32) -> serde_json::Value {
    let output: Output = common::run(arguments);
    assert_eq!(
        Some(expected),
        output.status.code(),
        "unexpected exit code; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout must be a JSON envelope: {error}; stdout: {}",
            String::from_utf8_lossy(&output.stdout),
        )
    })
}

/// The #1471 contract every generic plan-view dry-run must satisfy: a unified
/// `data.dry_run`, a genuinely empty `phases` for an absent target, and plan
/// fields kept flat under `data` (never nested behind a `plan` key).
fn assert_plan_dry_run_contract(data: &serde_json::Value) {
    assert_eq!(
        data.get("dry_run"),
        Some(&serde_json::Value::Bool(true)),
        "data.dry_run must be true across the plan view: {data}",
    );
    assert_eq!(
        data.get("phases"),
        Some(&serde_json::Value::Array(Vec::new())),
        "absent-component phases must be empty: {data}",
    );
    assert!(
        data.get("plan").is_none(),
        "plan fields must stay flat under data, not nested under 'plan': {data}",
    );
}

/// A temp prefix + system mode isolates the run from real state; a name that
/// is neither installed nor visible falls through to the absent-plan branch.
fn absent_uninstall_args<'a>(prefix: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "--json",
        "--dry-run",
        "--install-mode",
        "system",
        "--prefix",
        prefix,
        "uninstall",
    ];
    args.extend_from_slice(extra);
    args.push("definitely-missing");
    args
}

fn seed_local_repo(prefix: &Path) {
    let repo_v1 = prefix.join("repo/v1");
    std::fs::create_dir_all(&repo_v1).expect("local repo");
    std::fs::write(
        repo_v1.join("components.toml"),
        "schema_version = 1\ncomponents = []\n",
    )
    .expect("component index");
    let etc = prefix.join("etc/anolisa");
    std::fs::create_dir_all(&etc).expect("config dir");
    std::fs::write(
        etc.join("repo.toml"),
        format!(
            "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"file://{}\"\n",
            repo_v1.display()
        ),
    )
    .expect("repo config");
}

/// A dry-run of an absent component must report the same refusal a real run
/// would — an error envelope with the actionable "not installed" reason —
/// never a hollow successful preview.
#[test]
fn uninstall_dry_run_json_absent_component_reports_not_installed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_local_repo(tmp.path());
    let prefix = tmp.path().to_str().expect("utf-8 prefix");
    let value = run_json(&absent_uninstall_args(prefix, &[]), 2);

    assert_eq!(
        value.get("ok"),
        Some(&serde_json::Value::Bool(false)),
        "an absent component refuses on dry-run exactly like a real run: {value}",
    );
    let error = value.get("error").expect("envelope must carry error");
    assert_eq!(
        error.get("code").and_then(|v| v.as_str()),
        Some("INVALID_ARGUMENT"),
    );
    assert!(
        error
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|reason| reason.contains("not installed")),
        "the reason must say not installed: {value}",
    );
}

#[test]
fn uninstall_purge_dry_run_json_uses_same_contract() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_local_repo(tmp.path());
    let prefix = tmp.path().to_str().expect("utf-8 prefix");
    let value = run_json(&absent_uninstall_args(prefix, &["--purge"]), 0);

    let data = value.get("data").expect("envelope must carry data");
    assert_eq!(
        data.get("operation").and_then(|v| v.as_str()),
        Some("purge"),
        "purge keeps the generic plan view: {data}",
    );
    assert_plan_dry_run_contract(data);
}
