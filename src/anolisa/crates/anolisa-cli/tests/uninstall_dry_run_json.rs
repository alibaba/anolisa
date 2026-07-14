//! Subprocess wire-contract coverage for the generic lifecycle plan view of
//! `uninstall --dry-run --json` (#1471).
//!
//! The in-crate payload unit tests serialize [`PlanDryRunPayload`] directly, so
//! they cannot catch a regression that reverts the handler to `render_json(&plan)`
//! (which would drop `data.dry_run`). These tests drive the real
//! `handle → render_json` routing through the compiled binary and assert the
//! full envelope, closing that gap.

use std::process::Output;

mod common;

/// Run the CLI and parse its stdout as a JSON envelope, asserting a clean exit.
fn run_json(arguments: &[&str]) -> serde_json::Value {
    let output: Output = common::run(arguments);
    assert_eq!(
        Some(0),
        output.status.code(),
        "dry-run uninstall of an absent component must exit 0; stderr: {}",
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

#[test]
fn uninstall_dry_run_json_absent_component_carries_dry_run_and_empty_phases() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prefix = tmp.path().to_str().expect("utf-8 prefix");
    let value = run_json(&absent_uninstall_args(prefix, &[]));

    assert_eq!(
        value.get("ok"),
        Some(&serde_json::Value::Bool(true)),
        "absent-component dry-run is a successful preview: {value}",
    );
    assert_eq!(
        value.get("command").and_then(|v| v.as_str()),
        Some("uninstall"),
    );
    let data = value.get("data").expect("envelope must carry data");
    assert_eq!(
        data.get("operation").and_then(|v| v.as_str()),
        Some("uninstall"),
    );
    assert_plan_dry_run_contract(data);

    let warnings = data
        .get("warnings")
        .and_then(|v| v.as_array())
        .expect("plan carries its own warnings list");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().is_some_and(|s| s.contains("not installed"))),
        "the not-installed warning must remain: {data}",
    );
}

#[test]
fn uninstall_purge_dry_run_json_uses_same_contract() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prefix = tmp.path().to_str().expect("utf-8 prefix");
    let value = run_json(&absent_uninstall_args(prefix, &["--purge"]));

    let data = value.get("data").expect("envelope must carry data");
    assert_eq!(
        data.get("operation").and_then(|v| v.as_str()),
        Some("purge"),
        "purge shares the generic plan view: {data}",
    );
    assert_plan_dry_run_contract(data);
}
