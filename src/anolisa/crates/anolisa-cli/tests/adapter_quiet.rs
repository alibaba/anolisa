//! Subprocess coverage for global `--quiet` on adapter read-only printers.
//!
//! `adapter scan` already hid warnings under `--quiet` while still printing
//! the empty-state line and table. `adapter status` never consulted the flag.
//! Both must stay silent without `--json`, and `--json --quiet` must still
//! emit the standard envelope.

use serde_json::Value;

mod common;

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &std::process::Output) {
    assert_eq!(
        Some(0),
        output.status.code(),
        "unexpected exit; stdout: {}; stderr: {}",
        stdout(output),
        stderr(output),
    );
}

fn parse_envelope(output: &std::process::Output, command: &str) -> Value {
    assert_success(output);
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be a JSON envelope");
    assert_eq!(value.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(value.get("command").and_then(Value::as_str), Some(command));
    value
}

#[test]
fn adapter_scan_quiet_suppresses_human_stdout() {
    let output = common::run(&["--quiet", "--no-color", "adapter", "scan"]);
    assert_success(&output);
    assert!(
        output.stdout.is_empty(),
        "--quiet adapter scan must not print the table or empty-state line; stdout: {}",
        stdout(&output)
    );
    assert!(
        !stderr(&output).contains("No adapter declarations"),
        "empty-state text belongs on stdout, not stderr: {}",
        stderr(&output)
    );
}

#[test]
fn adapter_scan_without_quiet_prints_human_empty_state() {
    let output = common::run(&["--no-color", "adapter", "scan"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("No adapter declarations or resources found."),
        "human scan should report the empty state; stdout: {}",
        stdout(&output)
    );
}

#[test]
fn adapter_scan_quiet_json_still_emits_envelope() {
    let output = common::run(&["--quiet", "--json", "adapter", "scan"]);
    let value = parse_envelope(&output, "adapter scan");
    assert!(
        value["data"]["adapters"].is_array(),
        "JSON scan payload must keep adapters under data: {value}"
    );
}

#[test]
fn adapter_status_quiet_suppresses_human_stdout() {
    let output = common::run(&["--quiet", "--no-color", "adapter", "status"]);
    assert_success(&output);
    assert!(
        output.stdout.is_empty(),
        "--quiet adapter status must not print receipts or the empty-state line; stdout: {}",
        stdout(&output)
    );
}

#[test]
fn adapter_status_without_quiet_prints_human_empty_state() {
    let output = common::run(&["--no-color", "adapter", "status"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("No adapter receipts."),
        "human status should report the empty state; stdout: {}",
        stdout(&output)
    );
}

#[test]
fn adapter_status_quiet_json_still_emits_envelope() {
    let output = common::run(&["--quiet", "--json", "adapter", "status"]);
    let value = parse_envelope(&output, "adapter status");
    assert!(
        value["data"]["receipts"].is_array(),
        "JSON status payload must keep receipts under data: {value}"
    );
}
