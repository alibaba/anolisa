//! Subprocess wire-contract coverage for `uninstall --dry-run --json`.
//!
//! Plain uninstall runs the planner pipeline: for an absent component the
//! dry-run reports the same refusal as a real run would (an error envelope,
//! `NOT_INSTALLED`, exit 2), so previews never disagree with reality. The
//! code marks state absence only — it is not a statement about the name, as
//! `not_installed_does_not_distinguish_a_known_name_from_an_unknown_one`
//! pins. `--purge` keeps the legacy plan view: a unified `data.dry_run`,
//! plan fields flat under `data`. These tests drive the compiled binary and
//! assert the full envelope, which in-crate unit tests cannot cover.

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
    seed_local_repo_with_index(prefix, "components = []\n");
}

/// Same layout as [`seed_local_repo`] but with a caller-supplied component
/// index body, so a test can distinguish a name the index knows from one it
/// has never heard of.
fn seed_local_repo_with_index(prefix: &Path, components: &str) {
    let repo_v1 = prefix.join("repo/v1");
    std::fs::create_dir_all(&repo_v1).expect("local repo");
    std::fs::write(
        repo_v1.join("components.toml"),
        format!("schema_version = 1\n{components}"),
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
        Some("NOT_INSTALLED"),
        "an absent target is its own code, distinct from a malformed one: {value}",
    );
    assert!(
        error
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|reason| reason.contains("not installed")),
        "the reason must say not installed: {value}",
    );
}

/// `NOT_INSTALLED` reports state absence and nothing else. Resolution falls
/// back to the literal input when the component index has no entry for it
/// (`common::component_alias_from_repo_index`), so a name the index knows and
/// a name it has never seen reach this code indistinguishably — same `code`,
/// same exit status, differing only in the name echoed back.
///
/// This is pinned deliberately: callers must not read `NOT_INSTALLED` as
/// "the name was valid, it just isn't installed". Should the CLI ever grow
/// real identity validation, this test is the one that has to change, and
/// its failure is the signal to revisit every doc that describes the code.
#[test]
fn not_installed_does_not_distinguish_a_known_name_from_an_unknown_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_local_repo_with_index(
        tmp.path(),
        "components = [\n  { name = \"cosh\", package = \"copilot-shell\" },\n]\n",
    );
    let prefix = tmp.path().to_str().expect("utf-8 prefix");

    let mut codes = Vec::new();
    // "cosh" is in the index but not installed; "coshh" is in no index at all.
    for component in ["cosh", "coshh"] {
        let value = run_json(
            &[
                "--json",
                "--dry-run",
                "--install-mode",
                "system",
                "--prefix",
                prefix,
                "uninstall",
                component,
            ],
            2,
        );
        let error = value.get("error").expect("envelope must carry error");
        codes.push(
            error
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        );
    }

    assert_eq!(
        codes,
        vec!["NOT_INSTALLED".to_string(), "NOT_INSTALLED".to_string()],
        "an index-known name and an unknown one must be reported identically",
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
