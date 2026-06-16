//! Health-check execution engine.
//!
//! Single source of truth for "did this install actually come up?". The
//! engine is consumed by enable's `[7] Health Check` step today and is
//! shaped so `status` and `doctor` can reuse it. Every probe is bounded:
//! paths must resolve under an ANOLISA-owned root, command argv runs
//! *without* a shell, and spawned children are killed past a timeout — a
//! hostile manifest must not be able to read `/etc/shadow`, run a pipeline,
//! or hang the caller.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anolisa_platform::fs_layout::FsLayout;

use super::spec::{CheckOutcome, CheckSpec, CheckStatus};
use crate::path_safety::{PathBoundaryError, validate_owned_path};

/// Default per-process probe timeout. Generous for `--version`/`--help`
/// smoke probes while keeping a hostile or wedged child bounded.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll cadence for the spawn-wait loop — sub-second responsiveness without
/// burning a core.
const PROBE_POLL: Duration = Duration::from_millis(25);

/// Glyphs that turn an argv token into a shell expression. Probes never run
/// through `sh -c`, so a token requiring shell interpretation is, by
/// definition, not a valid probe and is refused.
const SHELL_METACHARS: &[char] = &[
    ';', '|', '&', '>', '<', '$', '`', '\\', '{', '}', '(', ')', '*', '?', '~', '!', '\n', '\r',
    '\'', '"',
];

/// Execution context for [`run_check`].
///
/// v1 needs only the layout (path bounding + placeholder expansion) and the
/// dry-run flag. `systemd_active` and friends will extend this with a
/// `ServiceManager` handle when those checks graduate from
/// [`CheckStatus::Unsupported`].
pub struct CheckEnv<'a> {
    /// Layout that bounds probe paths and expands `{bindir}`-style templates.
    pub layout: &'a FsLayout,
    /// When true, every node short-circuits to [`CheckStatus::Skipped`] and
    /// no process is spawned — owners never handle a dry-run flag themselves.
    pub dry_run: bool,
}

/// Run one (possibly aggregate) check spec and return a structured outcome.
///
/// Aggregates recurse, so a dry-run `all_of` yields a tree of `Skipped`
/// nodes (zero processes started) rather than a single opaque node.
pub fn run_check(spec: &CheckSpec, env: &CheckEnv<'_>) -> CheckOutcome {
    match spec {
        CheckSpec::AllOf { checks, .. } => {
            let children: Vec<CheckOutcome> = checks.iter().map(|c| run_check(c, env)).collect();
            let status = all_of_status(&children);
            CheckOutcome {
                spec_label: format!("all_of ({} checks)", checks.len()),
                status,
                detail: None,
                children,
            }
        }
        CheckSpec::AnyOf { checks, .. } => {
            let children: Vec<CheckOutcome> = checks.iter().map(|c| run_check(c, env)).collect();
            let status = any_of_status(&children);
            CheckOutcome {
                spec_label: format!("any_of ({} checks)", checks.len()),
                status,
                detail: None,
                children,
            }
        }
        leaf => {
            if env.dry_run {
                return CheckOutcome::leaf(label_for(leaf), CheckStatus::Skipped, None);
            }
            run_leaf(leaf, env)
        }
    }
}

/// `all_of`: any failure fails the group; all-ok passes; all-skipped (dry-run)
/// stays skipped; any remaining gap (e.g. an unsupported child) downgrades to
/// `Unsupported` because the group could not be fully verified.
fn all_of_status(children: &[CheckOutcome]) -> CheckStatus {
    if children.iter().any(|c| c.status == CheckStatus::Failed) {
        CheckStatus::Failed
    } else if children.iter().all(|c| c.status == CheckStatus::Ok) {
        CheckStatus::Ok
    } else if children.iter().all(|c| c.status == CheckStatus::Skipped) {
        CheckStatus::Skipped
    } else {
        CheckStatus::Unsupported
    }
}

/// `any_of`: one ok passes; otherwise all-skipped stays skipped, any failure
/// fails, and only-unsupported reports unsupported.
fn any_of_status(children: &[CheckOutcome]) -> CheckStatus {
    if children.iter().any(|c| c.status == CheckStatus::Ok) {
        CheckStatus::Ok
    } else if children.iter().all(|c| c.status == CheckStatus::Skipped) {
        CheckStatus::Skipped
    } else if children.iter().any(|c| c.status == CheckStatus::Failed) {
        CheckStatus::Failed
    } else {
        CheckStatus::Unsupported
    }
}

/// Dispatch a leaf check (callers guarantee `spec` is not an aggregate and
/// dry-run was already handled).
fn run_leaf(spec: &CheckSpec, env: &CheckEnv<'_>) -> CheckOutcome {
    match spec {
        CheckSpec::BinaryVersion {
            binary,
            expect_pattern,
            timeout_secs,
        } => check_binary(
            env,
            "binary_version",
            binary,
            &["--version"],
            expect_pattern.as_deref(),
            *timeout_secs,
        ),
        CheckSpec::BinaryHelp {
            binary,
            timeout_secs,
        } => check_binary(env, "binary_help", binary, &["--help"], None, *timeout_secs),
        CheckSpec::FileExists { path, mode, .. } => check_file_exists(env, path, mode.as_deref()),
        CheckSpec::Command {
            argv,
            expect_exit_code,
        } => check_command(env, argv, *expect_exit_code),
        // v1 stubs: interface frozen, execution deferred to the owning slice.
        CheckSpec::SystemdActive { .. }
        | CheckSpec::PortListen { .. }
        | CheckSpec::HttpGet { .. }
        | CheckSpec::BinaryCapabilities { .. } => CheckOutcome::leaf(
            label_for(spec),
            CheckStatus::Unsupported,
            Some("check type not yet implemented in this build".to_string()),
        ),
        // Aggregates are handled by `run_check`; never reached here.
        CheckSpec::AllOf { .. } | CheckSpec::AnyOf { .. } => {
            CheckOutcome::leaf(label_for(spec), CheckStatus::Unsupported, None)
        }
    }
}

/// Spawn `<binary> <args>` under the owned-root guard and resolve to
/// ok/failed/unsupported. Optionally require `expect_pattern` in stdout.
fn check_binary(
    env: &CheckEnv<'_>,
    kind: &str,
    binary: &str,
    args: &[&str],
    expect_pattern: Option<&str>,
    timeout_secs: Option<u64>,
) -> CheckOutcome {
    let expanded = expand_placeholders(binary, env.layout);
    let label = format!("{kind} binary={expanded}");
    let exe = Path::new(&expanded);
    if let Some(reason) = reject_unowned_executable(env.layout, exe) {
        return CheckOutcome::leaf(label, CheckStatus::Unsupported, Some(reason));
    }
    let capture = expect_pattern.is_some();
    let timeout = timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PROBE_TIMEOUT);
    match spawn_and_wait(exe, args, capture, timeout) {
        SpawnResult::Exited {
            success,
            code,
            stdout,
        } => {
            if !success {
                return CheckOutcome::leaf(
                    label,
                    CheckStatus::Failed,
                    Some(format!(
                        "`{expanded} {}` exited with status {code}",
                        args.join(" ")
                    )),
                );
            }
            if let Some(pattern) = expect_pattern
                && !stdout.contains(pattern)
            {
                return CheckOutcome::leaf(
                    label,
                    CheckStatus::Failed,
                    Some(format!("version output did not contain '{pattern}'")),
                );
            }
            CheckOutcome::leaf(label, CheckStatus::Ok, None)
        }
        SpawnResult::Timeout => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!(
                "`{expanded}` exceeded {}s probe timeout",
                timeout.as_secs()
            )),
        ),
        SpawnResult::SpawnError(err) => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!("failed to spawn '{expanded}': {err}")),
        ),
    }
}

/// Existence (and optional mode) check for a regular file, with the
/// owned-root + symlink guards `status` already relied on.
fn check_file_exists(env: &CheckEnv<'_>, path: &str, mode: Option<&str>) -> CheckOutcome {
    let expanded = expand_placeholders(path, env.layout);
    let label = format!("file_exists path={expanded}");
    let target = Path::new(&expanded);
    if let Err(err) = validate_owned_path(env.layout, target) {
        return CheckOutcome::leaf(
            label,
            CheckStatus::Unsupported,
            Some(format!(
                "path '{expanded}' rejected: {}",
                boundary_reason(&err)
            )),
        );
    }
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => CheckOutcome::leaf(
            label,
            CheckStatus::Unsupported,
            Some(format!(
                "path '{expanded}' is a symlink — refusing to follow"
            )),
        ),
        Ok(meta) if !meta.file_type().is_file() => CheckOutcome::leaf(
            label,
            CheckStatus::Unsupported,
            Some(format!("path '{expanded}' is not a regular file")),
        ),
        Ok(meta) => {
            if let Some(want) = mode {
                match parse_octal_mode(want) {
                    Some(want_bits) => {
                        let actual = meta.permissions().mode() & 0o777;
                        if actual != want_bits {
                            return CheckOutcome::leaf(
                                label,
                                CheckStatus::Failed,
                                Some(format!("mode {actual:04o} != expected {want_bits:04o}")),
                            );
                        }
                    }
                    None => {
                        return CheckOutcome::leaf(
                            label,
                            CheckStatus::Unsupported,
                            Some(format!("invalid expected mode '{want}'")),
                        );
                    }
                }
            }
            CheckOutcome::leaf(label, CheckStatus::Ok, None)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!("missing file: {expanded}")),
        ),
        Err(err) => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!("stat failed for '{expanded}': {err}")),
        ),
    }
}

/// Explicit-argv command probe. No shell is involved, but every token is
/// still placeholder-expanded and screened for shell metacharacters as
/// defense in depth, and `argv[0]` must be an owned-root executable.
fn check_command(
    env: &CheckEnv<'_>,
    argv: &[String],
    expect_exit_code: Option<i32>,
) -> CheckOutcome {
    let label = format!("command argv={}", argv.join(" "));
    let Some((exe_raw, rest)) = argv.split_first() else {
        return CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some("command argv is empty".to_string()),
        );
    };
    let expanded: Vec<String> = std::iter::once(exe_raw)
        .chain(rest)
        .map(|t| expand_placeholders(t, env.layout))
        .collect();
    if let Some(meta) = expanded
        .iter()
        .flat_map(|t| t.chars())
        .find(|c| SHELL_METACHARS.contains(c))
    {
        return CheckOutcome::leaf(
            label,
            CheckStatus::Unsupported,
            Some(format!(
                "argv contains shell metacharacter '{meta}' — commands run without a shell"
            )),
        );
    }
    let exe = Path::new(&expanded[0]);
    if let Some(reason) = reject_unowned_executable(env.layout, exe) {
        return CheckOutcome::leaf(label, CheckStatus::Unsupported, Some(reason));
    }
    let args: Vec<&str> = expanded[1..].iter().map(String::as_str).collect();
    match spawn_and_wait(exe, &args, false, DEFAULT_PROBE_TIMEOUT) {
        SpawnResult::Exited { code, .. } => {
            let want = expect_exit_code.unwrap_or(0);
            if code == want {
                CheckOutcome::leaf(label, CheckStatus::Ok, None)
            } else {
                CheckOutcome::leaf(
                    label,
                    CheckStatus::Failed,
                    Some(format!("exit code {code} != expected {want}")),
                )
            }
        }
        SpawnResult::Timeout => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!(
                "exceeded {}s probe timeout",
                DEFAULT_PROBE_TIMEOUT.as_secs()
            )),
        ),
        SpawnResult::SpawnError(err) => CheckOutcome::leaf(
            label,
            CheckStatus::Failed,
            Some(format!("failed to spawn: {err}")),
        ),
    }
}

/// Outcome of [`spawn_and_wait`].
enum SpawnResult {
    /// Child exited; `code` is the exit status (`-1` for signal termination).
    Exited {
        success: bool,
        code: i32,
        stdout: String,
    },
    /// Child exceeded the timeout and was killed.
    Timeout,
    /// Spawn itself failed (missing/non-executable binary).
    SpawnError(std::io::Error),
}

/// Spawn `exe args`, poll until exit or timeout, and (when `capture`) return
/// stdout. stdin/stderr are always null; stdout is null unless captured.
fn spawn_and_wait(exe: &Path, args: &[&str], capture: bool, timeout: Duration) -> SpawnResult {
    let stdout_cfg = if capture {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    // spawn_retry_etxtbsy: health probes exec binaries ANOLISA installed;
    // a concurrent fork elsewhere can hold the write descriptor for a
    // moment and fail exec with ETXTBSY.
    let mut child = match crate::process::spawn_retry_etxtbsy(
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout_cfg)
            .stderr(Stdio::null()),
    ) {
        Ok(c) => c,
        Err(err) => return SpawnResult::SpawnError(err),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = if capture {
                    use std::io::Read;
                    let mut buf = String::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_string(&mut buf);
                    }
                    buf
                } else {
                    String::new()
                };
                return SpawnResult::Exited {
                    success: status.success(),
                    code: status.code().unwrap_or(-1),
                    stdout,
                };
            }
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return SpawnResult::Timeout;
                }
                std::thread::sleep(PROBE_POLL);
            }
            Err(err) => return SpawnResult::SpawnError(err),
        }
    }
}

/// Reject an executable that is not an absolute path under an ANOLISA-owned
/// root. Returns the rejection reason, or `None` when the path is acceptable.
fn reject_unowned_executable(layout: &FsLayout, exe: &Path) -> Option<String> {
    if !exe.is_absolute() {
        return Some(format!(
            "executable '{}' is not absolute — declare the full `{{bindir}}/...` path",
            exe.display()
        ));
    }
    match validate_owned_path(layout, exe) {
        Ok(()) => None,
        Err(err) => Some(format!(
            "executable '{}' rejected: {}",
            exe.display(),
            boundary_reason(&err)
        )),
    }
}

/// Human-readable rendering of a path-boundary rejection.
fn boundary_reason(err: &PathBoundaryError) -> String {
    match err {
        PathBoundaryError::External { path } => {
            format!("'{}' is outside ANOLISA-owned roots", path.display())
        }
        PathBoundaryError::Traversal { path } => {
            format!("'{}' contains '.' or '..' segments", path.display())
        }
    }
}

/// Expand the FHS / file-hierarchy layout placeholders a manifest may use in a probe path.
/// The minimal-schema names (`{sysconfdir}`/`{sharedir}`) and the legacy ones
/// (`{etcdir}`/`{datadir}`) both resolve to the same roots during the
/// additive-compat window.
fn expand_placeholders(input: &str, layout: &FsLayout) -> String {
    let bin = layout.bin_dir.display().to_string();
    let libexec = layout.libexec_dir.display().to_string();
    let lib = layout.lib_dir.display().to_string();
    let data = layout.datadir.display().to_string();
    let etc = layout.etc_dir.display().to_string();
    let state = layout.state_dir.display().to_string();
    let cache = layout.cache_dir.display().to_string();
    let log = layout.log_dir.display().to_string();
    input
        .replace("{bindir}", &bin)
        .replace("{libexecdir}", &libexec)
        .replace("{libdir}", &lib)
        .replace("{sharedir}", &data)
        .replace("{datadir}", &data)
        .replace("{sysconfdir}", &etc)
        .replace("{etcdir}", &etc)
        .replace("{statedir}", &state)
        .replace("{cachedir}", &cache)
        .replace("{logdir}", &log)
}

/// Parse an octal mode string (`"0755"`, `"755"`) into its low 12 bits.
fn parse_octal_mode(raw: &str) -> Option<u32> {
    u32::from_str_radix(raw.trim_start_matches("0o"), 8)
        .ok()
        .map(|m| m & 0o7777)
}

/// One-line label used when no richer context is available (stubs, dry-run).
fn label_for(spec: &CheckSpec) -> String {
    match spec {
        CheckSpec::BinaryVersion { binary, .. } => format!("binary_version binary={binary}"),
        CheckSpec::BinaryHelp { binary, .. } => format!("binary_help binary={binary}"),
        CheckSpec::SystemdActive { service } => format!("systemd_active service={service}"),
        CheckSpec::FileExists { path, .. } => format!("file_exists path={path}"),
        CheckSpec::PortListen { port, .. } => format!("port_listen port={port}"),
        CheckSpec::HttpGet { url, .. } => format!("http_get url={url}"),
        CheckSpec::BinaryCapabilities { binary, .. } => {
            format!("binary_capabilities binary={binary}")
        }
        CheckSpec::Command { argv, .. } => format!("command argv={}", argv.join(" ")),
        CheckSpec::AllOf { checks, .. } => format!("all_of ({} checks)", checks.len()),
        CheckSpec::AnyOf { checks, .. } => format!("any_of ({} checks)", checks.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn layout_for(home: &Path) -> FsLayout {
        FsLayout::user(home.to_path_buf())
    }

    /// Write a 0755 shell script under `dir` and return its absolute path.
    fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).expect("mkdir");
        let path = dir.join(name);
        fs::write(&path, body).expect("write script");
        let mut perms = fs::metadata(&path).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    #[test]
    fn binary_version_ok_for_owned_executable() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let exe = write_exec(
            &layout.bin_dir,
            "tool",
            "#!/bin/sh\necho 'tool 1.2.3'\nexit 0\n",
        );
        let spec = CheckSpec::BinaryVersion {
            binary: exe.display().to_string(),
            expect_pattern: None,
            timeout_secs: None,
        };
        let out = run_check(
            &spec,
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Ok, "detail={:?}", out.detail);
    }

    #[test]
    fn binary_version_missing_binary_fails_with_path_in_detail() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let missing = layout.bin_dir.join("absent");
        let spec = CheckSpec::BinaryVersion {
            binary: missing.display().to_string(),
            expect_pattern: None,
            timeout_secs: None,
        };
        let out = run_check(
            &spec,
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Failed);
        assert!(out.detail.unwrap().contains("absent"));
    }

    #[test]
    fn binary_version_expect_pattern_mismatch_fails() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let exe = write_exec(&layout.bin_dir, "tool", "#!/bin/sh\necho 'tool 9.9.9'\n");
        let spec = CheckSpec::BinaryVersion {
            binary: exe.display().to_string(),
            expect_pattern: Some("1.2.3".to_string()),
            timeout_secs: None,
        };
        let out = run_check(
            &spec,
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Failed);
    }

    #[test]
    fn file_exists_ok_and_missing() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let present = write_exec(&layout.bin_dir, "present", "x");
        let ok = run_check(
            &CheckSpec::FileExists {
                path: present.display().to_string(),
                mode: None,
                owner: None,
            },
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(ok.status, CheckStatus::Ok);

        let missing = layout.bin_dir.join("nope");
        let out = run_check(
            &CheckSpec::FileExists {
                path: missing.display().to_string(),
                mode: None,
                owner: None,
            },
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Failed);
    }

    #[test]
    fn command_rejects_external_executable() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let out = run_check(
            &CheckSpec::Command {
                argv: vec!["/usr/bin/true".to_string()],
                expect_exit_code: None,
            },
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(
            out.status,
            CheckStatus::Unsupported,
            "detail={:?}",
            out.detail
        );
    }

    #[test]
    fn command_rejects_shell_metacharacters() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let exe = write_exec(&layout.bin_dir, "t", "#!/bin/sh\n");
        let out = run_check(
            &CheckSpec::Command {
                argv: vec![exe.display().to_string(), "a; rm -rf /".to_string()],
                expect_exit_code: None,
            },
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Unsupported);
    }

    #[test]
    fn all_of_fails_if_any_child_fails_and_keeps_children() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let present = write_exec(&layout.bin_dir, "present", "x");
        let spec = CheckSpec::AllOf {
            checks: vec![
                CheckSpec::FileExists {
                    path: present.display().to_string(),
                    mode: None,
                    owner: None,
                },
                CheckSpec::FileExists {
                    path: layout.bin_dir.join("gone").display().to_string(),
                    mode: None,
                    owner: None,
                },
            ],
            timeout_secs: None,
        };
        let out = run_check(
            &spec,
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Failed);
        assert_eq!(out.children.len(), 2);
        assert_eq!(out.children[0].status, CheckStatus::Ok);
        assert_eq!(out.children[1].status, CheckStatus::Failed);
    }

    #[test]
    fn dry_run_skips_all_nodes_and_starts_no_process() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let marker = home.path().join("ran.marker");
        let exe = write_exec(
            &layout.bin_dir,
            "tool",
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let spec = CheckSpec::AllOf {
            checks: vec![CheckSpec::BinaryVersion {
                binary: exe.display().to_string(),
                expect_pattern: None,
                timeout_secs: None,
            }],
            timeout_secs: None,
        };
        let out = run_check(
            &spec,
            &CheckEnv {
                layout: &layout,
                dry_run: true,
            },
        );
        assert_eq!(out.status, CheckStatus::Skipped);
        assert_eq!(out.children[0].status, CheckStatus::Skipped);
        assert!(!marker.exists(), "dry-run must not spawn the probe binary");
    }

    #[test]
    fn systemd_active_is_unsupported_in_v1() {
        let home = tempdir().expect("tempdir");
        let layout = layout_for(home.path());
        let out = run_check(
            &CheckSpec::SystemdActive {
                service: "anything.service".to_string(),
            },
            &CheckEnv {
                layout: &layout,
                dry_run: false,
            },
        );
        assert_eq!(out.status, CheckStatus::Unsupported);
    }
}
