//! CLI startup-gate coverage for `--decision-command`.
//!
//! These tests run the actual `skillfs mount` binary so the early
//! validation gates can be observed end-to-end:
//!
//! * `--security` without `--decision-command` must reject the
//!   startup before any FUSE side effect runs.
//! * An empty / whitespace-only `--decision-command` must reject
//!   startup with a clear error.
//!
//! Running the real binary is the cheapest available signal: the CLI
//! parser plus the up-front error path is exactly what an operator
//! sees, and a unit test would only exercise the parser shape, not the
//! wiring in `cmd_mount`.

use std::path::Path;
use std::process::Command;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_skillfs")
}

fn empty_source() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("source tempdir");
    // Make the source path a real directory so the "Source directory
    // does not exist" gate is not the one that fires first.
    assert!(Path::new(dir.path()).is_dir());
    dir
}

#[test]
fn security_without_decision_command_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(
        !out.status.success(),
        "expected non-zero exit, stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--security requires --decision-command")
            || combined.contains("--security requires"),
        "expected startup error message, got: {combined}"
    );
}

#[test]
fn empty_decision_command_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--decision-command",
            "   ",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("invalid --decision-command"),
        "expected decision-command parse error, got: {combined}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation mode startup gates
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn activation_mode_file_without_security_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--activation-mode",
            "file",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--activation-mode file requires --security"),
        "expected activation-requires-security error, got: {combined}"
    );
}

#[test]
fn activation_mode_file_with_decision_command_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--decision-command",
            "agent-sec-cli skill-ledger",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("mutually exclusive"),
        "expected dual-source conflict error, got: {combined}"
    );
}

#[test]
fn activation_mode_file_with_events_log_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let events_log = tempfile::NamedTempFile::new().expect("events log");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--events-log",
            events_log.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("not supported with --activation-mode file"),
        "expected events-log-not-supported error, got: {combined}"
    );
}

#[test]
fn invalid_activation_mode_value_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "auto",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("invalid --activation-mode"),
        "expected invalid mode error, got: {combined}"
    );
}

#[test]
fn config_activation_file_overridden_by_cli_off() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("security.toml");
    std::fs::write(&config_path, "[activation]\nmode = \"file\"\n").unwrap();

    // CLI --activation-mode off should override config file's "file".
    // Without --decision-command or --activation-mode file, --security
    // should fail asking for a source, NOT try to load activation files.
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "off",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // When activation is off and no decision-command, should get the
    // "requires --decision-command or --activation-mode file" error,
    // proving the CLI off overrode the config's file.
    assert!(
        combined.contains("--security requires"),
        "expected security-requires error (proving CLI off overrode config file), got: {combined}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation events log startup gates (N3)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn activation_events_log_without_security_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let log = tempfile::NamedTempFile::new().expect("log file");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--activation-events-log",
            log.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--activation-events-log") && combined.contains("requires --security"),
        "expected requires-security error, got: {combined}"
    );
}

#[test]
fn activation_events_log_without_activation_mode_file_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let log = tempfile::NamedTempFile::new().expect("log file");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--decision-command",
            "echo",
            "--activation-events-log",
            log.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--activation-events-log")
            && combined.contains("requires --activation-mode file"),
        "expected requires-activation-mode-file error, got: {combined}"
    );
}

#[test]
fn activation_events_log_with_decision_command_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let log = tempfile::NamedTempFile::new().expect("log file");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--decision-command",
            "echo",
            "--activation-events-log",
            log.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("mutually exclusive"),
        "expected mutually-exclusive error, got: {combined}"
    );
}

#[test]
fn activation_events_log_inside_source_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let log_path = source.path().join("events.jsonl");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--activation-events-log",
            log_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("lies inside the SkillFS source root")
            || combined.contains("--activation-events-log"),
        "expected inside-source rejection, got: {combined}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation reload mode startup gates (A3)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reload_poll_without_notify_source_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--activation-reload-mode",
            "poll",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains(
            "--activation-reload-mode poll requires --notify-socket or --activation-events-log"
        ),
        "expected reload-requires-trigger error, got: {combined}"
    );
}

#[test]
fn config_reload_poll_without_notify_source_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("security.toml");
    std::fs::write(
        &config_path,
        "[activation]\nmode = \"file\"\nreload = \"poll\"\n",
    )
    .unwrap();

    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains(
            "--activation-reload-mode poll requires --notify-socket or --activation-events-log"
        ),
        "expected reload-requires-trigger error from config, got: {combined}"
    );
}

#[test]
fn reload_poll_without_security_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--activation-mode",
            "file",
            "--activation-reload-mode",
            "poll",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("requires --security"),
        "expected requires-security error, got: {combined}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// A6/B1: Ledger backing root startup gates
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ledger_backing_root_without_security_fails_startup() {
    // Don't pass --activation-mode file either (its own gate fires first).
    // Just pass --ledger-backing-root without --security; activation mode
    // stays off so the backing root's "requires --security" gate fires.
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let backing = tempfile::tempdir().expect("backing tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--ledger-backing-root",
            backing.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--ledger-backing-root") && combined.contains("requires --security"),
        "expected requires-security error, got: {combined}"
    );
}

#[test]
fn ledger_backing_root_without_activation_mode_file_fails_startup() {
    // Pass --security --decision-command echo to satisfy the "security
    // requires a source" gate, then the backing root check fires for
    // activation_mode != File.
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let backing = tempfile::tempdir().expect("backing tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--decision-command",
            "echo",
            "--ledger-backing-root",
            backing.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // The backing root check fires: activation_mode != File.
    // But note: --decision-command is also present, so "mutually exclusive"
    // may fire instead. Both are acceptable — the point is startup is rejected.
    assert!(
        (combined.contains("--ledger-backing-root")
            && combined.contains("requires --activation-mode file"))
            || combined.contains("mutually exclusive"),
        "expected requires-activation-mode-file or mutually-exclusive error, got: {combined}"
    );
}

#[test]
fn ledger_backing_root_with_decision_command_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let backing = tempfile::tempdir().expect("backing tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--decision-command",
            "echo",
            "--ledger-backing-root",
            backing.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("mutually exclusive"),
        "expected mutually-exclusive error, got: {combined}"
    );
}

#[test]
fn ledger_backing_root_config_overridden_by_cli() {
    // Config has [ledger].backing_root = "/config/path".
    // CLI provides --ledger-backing-root /cli/path.
    // The CLI value should take precedence. We verify this by checking
    // that the CLI path appears in the startup error (not the config path).
    // The CLI path is inside source (which is a tempdir), so it should
    // be rejected with an inside-source/mount error.
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("security.toml");
    std::fs::write(
        &config_path,
        "[activation]\nmode = \"file\"\n[ledger]\nbacking_root = \"/nonexistent/config/path\"\n",
    )
    .unwrap();

    // CLI provides a path inside source — should be rejected.
    let cli_backing = source.path().join("backing_root");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--config",
            config_path.to_str().unwrap(),
            "--ledger-backing-root",
            cli_backing.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // The CLI path should be the one that's rejected, not the config path.
    assert!(
        combined.contains("backing root") && combined.contains("backing_root"),
        "expected CLI backing root path in error, got: {combined}"
    );
    assert!(
        !combined.contains("/nonexistent/config/path"),
        "config path should not appear when CLI overrides it, got: {combined}"
    );
}

#[test]
fn ledger_backing_root_inside_source_fails_before_mount() {
    // In non-in-place mode, backing root inside source is rejected.
    // (In non-in-place, source != mount, so inside-source is not the same
    // as inside-mount. But the backing root inside the source tree is
    // still a bad idea for in-place; here we test non-in-place where
    // backing root == source is allowed.)
    //
    // Instead, test that a backing root inside the mount path is rejected.
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let backing_inside_mount = mount.path().join("backing_root");

    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--activation-mode",
            "file",
            "--ledger-backing-root",
            backing_inside_mount.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("backing root") && combined.contains("mount path"),
        "expected inside-mount-path rejection, got: {combined}"
    );
    // The mount point should NOT have a backing_root directory created.
    assert!(
        !backing_inside_mount.exists(),
        "backing root dir should not be created when path check fails"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// PID file cleanup tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn pid_file_not_left_behind_on_startup_failure() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let pid_dir = tempfile::tempdir().expect("pid dir");
    let pid_path = pid_dir.path().join("skillfs.pid");

    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--security",
            "--pid-file",
            pid_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success());
    assert!(
        !pid_path.exists(),
        "pid file must not be left behind after startup failure"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Trusted writer exe startup gates
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn trusted_writer_exe_nonexistent_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--trusted-writer-exe",
            "/nonexistent/binary/path",
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("--trusted-writer-exe"),
        "expected trusted-writer-exe error, got: {combined}"
    );
}

#[test]
fn trusted_writer_exe_directory_fails_startup() {
    let source = empty_source();
    let mount = tempfile::tempdir().expect("mount tempdir");
    let dir = tempfile::tempdir().expect("directory for test");
    let out = Command::new(bin_path())
        .args([
            "mount",
            source.path().to_str().unwrap(),
            mount.path().to_str().unwrap(),
            "--trusted-writer-exe",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke skillfs");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        combined.contains("not a regular file"),
        "expected not-a-regular-file error, got: {combined}"
    );
}
