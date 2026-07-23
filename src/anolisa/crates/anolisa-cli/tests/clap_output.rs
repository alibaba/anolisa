//! Subprocess coverage for Clap-owned output and diagnostics.

#[cfg(target_os = "linux")]
use std::fs::OpenOptions;
use std::process::Output;
#[cfg(target_os = "linux")]
use std::process::Stdio;

mod common;

fn run(arguments: &[&str]) -> Output {
    common::run(arguments)
}

#[test]
fn help_renders_to_stdout() {
    // Given a request for top-level help.
    let arguments = ["--help"];

    // When the CLI handles the request.
    let output = run(&arguments);

    // Then the rendered text remains on stdout with a successful status.
    assert_eq!(Some(0), output.status.code());
    assert!(!output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn version_renders_to_stdout() {
    // Given a request for the CLI version.
    let arguments = ["--version"];

    // When the CLI handles the request.
    let output = run(&arguments);

    // Then the rendered text remains on stdout with a successful status.
    assert_eq!(Some(0), output.status.code());
    assert!(!output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_subcommand_renders_clap_diagnostic_to_stderr() {
    // Given an invocation without a required subcommand.
    let arguments = [];

    // When the CLI parses the invocation.
    let output = run(&arguments);

    // Then Clap preserves its stderr destination and usage-error status.
    assert_eq!(Some(2), output.status.code());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn unknown_option_renders_clap_diagnostic_to_stderr() {
    // Given an invocation with an unknown option.
    let arguments = ["--definitely-invalid"];

    // When the CLI parses the invocation.
    let output = run(&arguments);

    // Then Clap preserves its stderr destination and usage-error status.
    assert_eq!(Some(2), output.status.code());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
}

#[test]
fn doctor_fix_help_matches_reserved_behavior() {
    let help = run(&["doctor", "--help"]);
    assert_eq!(Some(0), help.status.code());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("returns `NOT_IMPLEMENTED`"));
    assert!(!help.contains("executes the fix plan"));

    let fix = run(&["doctor", "--fix"]);
    assert_eq!(Some(64), fix.status.code());
    assert!(String::from_utf8_lossy(&fix.stderr).contains("NOT_IMPLEMENTED"));
}

#[cfg(target_os = "linux")]
#[test]
fn help_reports_stdout_failure_when_output_device_is_full() {
    // Given stdout is connected to a device that rejects every write.
    let full = OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("Linux must provide /dev/full");

    // When Clap renders top-level help.
    let output = common::run_with_stdout(&["--help"], Stdio::from(full));

    // Then the CLI reports the failed stdout write instead of exiting successfully.
    assert_eq!(Some(1), output.status.code());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("error[EXECUTION_FAILED]: failed writing to stdout:")
    );
}
