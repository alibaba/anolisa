//! Managed mount supervisor for SkillFS.
//!
//! `skillfs mount --managed <SOURCE> <MOUNTPOINT>` keeps the mount desired
//! state as "mounted" until an explicit `skillfs stop <MOUNTPOINT>` clears
//! it. This survives the caller's process group being torn down (for
//! example when the OpenClaw gateway that launched SkillFS restarts).
//!
//! Three roles share the `skillfs` binary:
//!
//! * **client** — `skillfs mount --managed ...`: writes managed state,
//!   spawns a detached supervisor in its own session (`setsid`), waits for
//!   the mount to become ready, then returns.
//! * **supervisor** — `skillfs supervise --instance <id>` (hidden): runs the
//!   foreground FUSE worker and remounts it after a bounded backoff whenever
//!   it exits while the desired state is still "mounted".
//! * **worker** — `skillfs mount --foreground ...`: the ordinary foreground
//!   mount path, unchanged.
//!
//! State lives under `$XDG_RUNTIME_DIR/skillfs/` (or `/run/user/<uid>/skillfs/`,
//! falling back to `/tmp/skillfs-<uid>/`). The instance id is derived from the
//! canonical mountpoint so `mount` and `stop` agree on the same instance.

use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Schema version for the on-disk managed state file.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// Initial remount backoff after an unexpected worker exit.
const INITIAL_BACKOFF_MS: u64 = 200;
/// Upper bound on the remount backoff.
const MAX_BACKOFF_MS: u64 = 5_000;
/// A worker that ran at least this long is treated as a healthy mount whose
/// later exit is not part of a crash loop, so the backoff resets.
const STABLE_RUN_SECS: u64 = 10;
/// Poll interval while waiting on the worker child.
const WORKER_POLL_MS: u64 = 200;
/// How long the client waits for the mount to become ready before failing.
const READY_TIMEOUT_MS: u64 = 10_000;
/// How long `stop` waits for processes to exit / the mount to disappear.
const STOP_TIMEOUT_MS: u64 = 10_000;

/// Desired lifecycle state for a managed mount.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DesiredState {
    /// The supervisor should keep the mount alive, remounting on exit.
    Mounted,
    /// An explicit stop cleared the desired state; do not remount.
    Stopped,
}

/// On-disk record describing a managed mount instance.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ManagedState {
    pub schema_version: u32,
    pub instance_id: String,
    pub mountpoint: String,
    pub source: String,
    pub worker_program: String,
    pub worker_args: Vec<String>,
    pub desired_state: DesiredState,
}

impl ManagedState {
    fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let raw = std::fs::read_to_string(path)?;
        let state: ManagedState = serde_json::from_str(&raw)?;
        Ok(state)
    }

    fn save(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        let raw = serde_json::to_string_pretty(self)?;
        // Write-and-rename for atomicity so a concurrent reader never sees a
        // half-written file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Filesystem paths for a managed mount instance.
pub struct ManagedPaths {
    pub dir: PathBuf,
    pub state: PathBuf,
    pub supervisor_pid: PathBuf,
    pub worker_pid: PathBuf,
    pub supervisor_log: PathBuf,
    pub worker_log: PathBuf,
}

impl ManagedPaths {
    fn new(instance_id: &str) -> Self {
        let dir = runtime_dir();
        ManagedPaths {
            state: dir.join(format!("{instance_id}.state.json")),
            supervisor_pid: dir.join(format!("{instance_id}.supervisor.pid")),
            worker_pid: dir.join(format!("{instance_id}.worker.pid")),
            supervisor_log: dir.join(format!("{instance_id}.supervisor.log")),
            worker_log: dir.join(format!("{instance_id}.worker.log")),
            dir,
        }
    }
}

/// Resolve the managed-state runtime directory.
///
/// Prefers `$XDG_RUNTIME_DIR/skillfs`, then `/run/user/<uid>/skillfs`, and
/// falls back to `/tmp/skillfs-<uid>` only if neither is usable.
pub fn runtime_dir() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    if let Some(base) = std::env::var_os("XDG_RUNTIME_DIR") {
        let base = PathBuf::from(base);
        if !base.as_os_str().is_empty() {
            return base.join("skillfs");
        }
    }
    let run_user = PathBuf::from(format!("/run/user/{uid}"));
    if run_user.is_dir() {
        return run_user.join("skillfs");
    }
    PathBuf::from(format!("/tmp/skillfs-{uid}"))
}

/// Normalize a mountpoint path for identity purposes: canonicalize if it
/// exists, otherwise fall back to an absolute (lexical) path so `mount` and
/// `stop` derive the same instance id before/after the directory exists.
pub fn normalize_mountpoint(mountpoint: &Path) -> PathBuf {
    if let Ok(canon) = mountpoint.canonicalize() {
        return canon;
    }
    std::path::absolute(mountpoint).unwrap_or_else(|_| mountpoint.to_path_buf())
}

/// Stable, collision-resistant instance id derived from a normalized path.
///
/// Combines a sanitized basename (for human readability) with a hash of the
/// full path (for uniqueness).
pub fn instance_id_for(normalized: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    normalized.as_os_str().hash(&mut hasher);
    let hash = hasher.finish();

    let base: String = normalized
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    let base = if base.is_empty() {
        "root".to_string()
    } else {
        base
    };
    format!("{base}-{hash:016x}")
}

/// Build the worker argument vector from the client's raw arguments.
///
/// The worker runs the ordinary foreground mount path, so we drop `--managed`
/// and ensure `--foreground` is present. `raw_args` excludes the program name.
pub fn build_worker_args(raw_args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = raw_args
        .iter()
        .filter(|a| a.as_str() != "--managed")
        .cloned()
        .collect();
    if !out.iter().any(|a| a == "--foreground") {
        // Insert right after the `mount` subcommand token so it lands in the
        // subcommand's argument list rather than being read as a global.
        if let Some(pos) = out.iter().position(|a| a == "mount") {
            out.insert(pos + 1, "--foreground".to_string());
        } else {
            out.insert(0, "--foreground".to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Process / mount helpers
// ---------------------------------------------------------------------------

fn pid_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

fn send_signal(pid: i32, sig: i32) {
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

fn read_pid(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
}

fn write_pid(path: &Path, pid: u32) -> Result<(), Box<dyn Error>> {
    std::fs::write(path, format!("{pid}\n"))?;
    Ok(())
}

/// Whether the mountpoint currently appears in `/proc/mounts`.
pub fn is_mounted(mountpoint: &Path) -> bool {
    let target = mountpoint.to_string_lossy();
    match std::fs::read_to_string("/proc/mounts") {
        Ok(info) => info
            .lines()
            .any(|line| line.split_whitespace().nth(1) == Some(&*target)),
        Err(_) => false,
    }
}

/// Readiness = present in `/proc/mounts` and a minimal readdir succeeds.
fn is_mount_ready(mountpoint: &Path) -> bool {
    is_mounted(mountpoint) && std::fs::read_dir(mountpoint).is_ok()
}

fn force_unmount(mountpoint: &Path) {
    let _ = std::process::Command::new("fusermount3")
        .args(["-u", &mountpoint.to_string_lossy()])
        .output();
}

// ---------------------------------------------------------------------------
// Client: `skillfs mount --managed ...`
// ---------------------------------------------------------------------------

/// Entry point for a managed mount request. Validates the source, writes the
/// managed state, spawns a detached supervisor, and waits for readiness.
pub fn run_client(
    raw_args: &[String],
    source: &Path,
    mountpoint: &Path,
) -> Result<(), Box<dyn Error>> {
    // Fast, client-side validation so operators get an immediate error
    // instead of a readiness timeout when the source is wrong. Deeper
    // validation still happens in the worker.
    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    // Create the mountpoint up front (mirrors the compat-mode mount UX) so it
    // can be canonicalized into a stable instance id.
    if !mountpoint.exists() {
        std::fs::create_dir_all(mountpoint).map_err(|e| {
            format!(
                "failed to create mount point '{}': {e}",
                mountpoint.display()
            )
        })?;
    }

    let normalized = normalize_mountpoint(mountpoint);
    let source_norm = normalize_mountpoint(source);
    let instance_id = instance_id_for(&normalized);
    let paths = ManagedPaths::new(&instance_id);

    // If a live supervisor already owns this mountpoint, do not start a
    // second one.
    if let Some(pid) = read_pid(&paths.supervisor_pid) {
        if pid_alive(pid) && is_mounted(&normalized) {
            info!(
                mountpoint = %normalized.display(),
                supervisor_pid = pid,
                "managed mount already active; nothing to do"
            );
            println!(
                "skillfs: managed mount already active at {}",
                normalized.display()
            );
            return Ok(());
        }
    }

    std::fs::create_dir_all(&paths.dir).map_err(|e| {
        format!(
            "failed to create runtime dir '{}': {e}",
            paths.dir.display()
        )
    })?;

    let worker_program = std::env::current_exe()
        .map_err(|e| format!("failed to resolve current executable: {e}"))?;
    let worker_args = build_worker_args(raw_args);

    let state = ManagedState {
        schema_version: STATE_SCHEMA_VERSION,
        instance_id: instance_id.clone(),
        mountpoint: normalized.to_string_lossy().to_string(),
        source: source_norm.to_string_lossy().to_string(),
        worker_program: worker_program.to_string_lossy().to_string(),
        worker_args,
        desired_state: DesiredState::Mounted,
    };
    state.save(&paths.state)?;

    spawn_supervisor(&worker_program, &instance_id, &paths)?;

    // Wait for readiness.
    let deadline = Instant::now() + Duration::from_millis(READY_TIMEOUT_MS);
    loop {
        if is_mount_ready(&normalized) {
            info!(
                mountpoint = %normalized.display(),
                instance = %instance_id,
                "managed mount ready"
            );
            println!(
                "skillfs: managed mount ready at {} (stop with: skillfs stop {})",
                normalized.display(),
                normalized.display()
            );
            return Ok(());
        }
        // If the supervisor died before the mount came up, surface the
        // worker log so the failure is diagnosable.
        if let Some(pid) = read_pid(&paths.supervisor_pid) {
            if !pid_alive(pid) {
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let worker_log = std::fs::read_to_string(&paths.worker_log).unwrap_or_default();
    let mut tail_lines: Vec<&str> = worker_log.lines().rev().take(20).collect();
    tail_lines.reverse();
    let tail = tail_lines.join("\n");
    Err(format!(
        "managed mount did not become ready within {}ms.\n--- worker log tail ---\n{}",
        READY_TIMEOUT_MS, tail
    )
    .into())
}

/// Spawn the supervisor detached in its own session so a restart of the
/// caller's process group does not tear it down.
fn spawn_supervisor(
    program: &Path,
    instance_id: &str,
    paths: &ManagedPaths,
) -> Result<(), Box<dyn Error>> {
    use std::os::unix::process::CommandExt;

    let sup_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.supervisor_log)
        .map_err(|e| {
            format!(
                "failed to open supervisor log '{}': {e}",
                paths.supervisor_log.display()
            )
        })?;
    let sup_log_err = sup_log.try_clone()?;

    let mut cmd = std::process::Command::new(program);
    cmd.arg("supervise")
        .arg("--instance")
        .arg(instance_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(sup_log))
        .stderr(std::process::Stdio::from(sup_log_err));

    // Detach: new session + new process group, no controlling terminal.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn supervisor: {e}"))?;
    info!(
        supervisor_pid = child.id(),
        instance = %instance_id,
        "spawned detached managed supervisor"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Supervisor: `skillfs supervise --instance <id>`
// ---------------------------------------------------------------------------

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_term(_sig: i32) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_term_handler() {
    let handler = handle_term as extern "C" fn(i32) as *const () as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

/// Entry point for the detached supervisor process.
pub fn run_supervisor(instance_id: &str) -> Result<(), Box<dyn Error>> {
    let paths = ManagedPaths::new(instance_id);
    let state = ManagedState::load(&paths.state)
        .map_err(|e| format!("failed to load managed state for '{instance_id}': {e}"))?;
    let mountpoint = PathBuf::from(&state.mountpoint);

    write_pid(&paths.supervisor_pid, std::process::id())?;
    install_term_handler();
    info!(
        instance = %instance_id,
        mountpoint = %mountpoint.display(),
        "managed supervisor started"
    );

    let mut backoff_ms = INITIAL_BACKOFF_MS;

    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break;
        }
        if current_desired_state(&paths.state) == DesiredState::Stopped {
            break;
        }

        let worker_out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.worker_log)
            .map_err(|e| format!("failed to open worker log: {e}"))?;
        let worker_err = worker_out.try_clone()?;

        let start = Instant::now();
        let mut child = std::process::Command::new(&state.worker_program)
            .args(&state.worker_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(worker_out))
            .stderr(std::process::Stdio::from(worker_err))
            .spawn()
            .map_err(|e| format!("failed to spawn worker: {e}"))?;
        let worker_pid = child.id();
        let _ = write_pid(&paths.worker_pid, worker_pid);
        info!(worker_pid, "managed worker started");

        // Wait for the worker, watching for shutdown.
        loop {
            if SHUTDOWN.load(Ordering::SeqCst)
                || current_desired_state(&paths.state) == DesiredState::Stopped
            {
                info!(
                    worker_pid,
                    "stopping managed worker (desired state cleared)"
                );
                send_signal(worker_pid as i32, libc::SIGTERM);
                let _ = wait_child_timeout(&mut child, Duration::from_millis(STOP_TIMEOUT_MS));
                let _ = std::fs::remove_file(&paths.worker_pid);
                finish(&paths, &mountpoint);
                return Ok(());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    warn!(worker_pid, ?status, "managed worker exited");
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(WORKER_POLL_MS)),
                Err(e) => {
                    warn!(error = %e, "error waiting on managed worker");
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&paths.worker_pid);

        // Worker exited on its own. Decide whether to remount.
        if SHUTDOWN.load(Ordering::SeqCst)
            || current_desired_state(&paths.state) == DesiredState::Stopped
        {
            break;
        }

        if start.elapsed() >= Duration::from_secs(STABLE_RUN_SECS) {
            backoff_ms = INITIAL_BACKOFF_MS;
        }
        info!(
            backoff_ms,
            "managed worker exited unexpectedly; remounting after backoff"
        );
        // Sleep in small slices so shutdown is responsive during backoff.
        let backoff_deadline = Instant::now() + Duration::from_millis(backoff_ms);
        while Instant::now() < backoff_deadline {
            if SHUTDOWN.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        backoff_ms = (backoff_ms.saturating_mul(2)).min(MAX_BACKOFF_MS);
    }

    finish(&paths, &mountpoint);
    Ok(())
}

fn current_desired_state(state_path: &Path) -> DesiredState {
    // A missing/unreadable state file means the instance was torn down; treat
    // it as stopped so the supervisor exits rather than remounting forever.
    ManagedState::load(state_path)
        .map(|s| s.desired_state)
        .unwrap_or(DesiredState::Stopped)
}

fn wait_child_timeout(child: &mut std::process::Child, timeout: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(_) => return Ok(()),
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(WORKER_POLL_MS));
            }
        }
    }
}

/// Final cleanup: ensure the mount is gone and remove instance state.
fn finish(paths: &ManagedPaths, mountpoint: &Path) {
    if is_mounted(mountpoint) {
        force_unmount(mountpoint);
    }
    let _ = std::fs::remove_file(&paths.worker_pid);
    let _ = std::fs::remove_file(&paths.supervisor_pid);
    let _ = std::fs::remove_file(&paths.state);
    info!(mountpoint = %mountpoint.display(), "managed supervisor exiting");
}

// ---------------------------------------------------------------------------
// Stop: `skillfs stop <MOUNTPOINT>`
// ---------------------------------------------------------------------------

/// Entry point for `skillfs stop`. Clears the desired state, terminates the
/// managed supervisor/worker, and unmounts. Idempotent: tolerates an
/// already-unmounted mount and a missing managed instance.
pub fn run_stop(mountpoint: &Path) -> Result<(), Box<dyn Error>> {
    let normalized = normalize_mountpoint(mountpoint);
    let instance_id = instance_id_for(&normalized);
    let paths = ManagedPaths::new(&instance_id);

    let had_state = paths.state.exists();

    // Clear desired state first so the supervisor will not remount even if a
    // remount races with our signal.
    if had_state {
        if let Ok(mut state) = ManagedState::load(&paths.state) {
            state.desired_state = DesiredState::Stopped;
            let _ = state.save(&paths.state);
        }
    }

    // Prefer signaling the supervisor: it owns the worker and unmounts it
    // cleanly. Fall back to signaling the worker directly.
    let sup_pid = read_pid(&paths.supervisor_pid);
    let worker_pid = read_pid(&paths.worker_pid);

    if let Some(pid) = sup_pid {
        if pid_alive(pid) {
            info!(supervisor_pid = pid, "stopping managed supervisor");
            send_signal(pid, libc::SIGTERM);
        }
    }
    if sup_pid.is_none_or(|p| !pid_alive(p)) {
        if let Some(pid) = worker_pid {
            if pid_alive(pid) {
                info!(worker_pid = pid, "stopping managed worker directly");
                send_signal(pid, libc::SIGTERM);
            }
        }
    }

    // Wait for processes to exit and the mount to disappear.
    let deadline = Instant::now() + Duration::from_millis(STOP_TIMEOUT_MS);
    loop {
        let sup_gone = sup_pid.is_none_or(|p| !pid_alive(p));
        let worker_gone = worker_pid.is_none_or(|p| !pid_alive(p));
        let unmounted = !is_mounted(&normalized);
        if sup_gone && worker_gone && unmounted {
            break;
        }
        if Instant::now() >= deadline {
            warn!("stop timed out waiting for clean shutdown; forcing");
            if let Some(p) = sup_pid {
                if pid_alive(p) {
                    send_signal(p, libc::SIGKILL);
                }
            }
            if let Some(p) = worker_pid {
                if pid_alive(p) {
                    send_signal(p, libc::SIGKILL);
                }
            }
            if is_mounted(&normalized) {
                force_unmount(&normalized);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Best-effort final cleanup of any residual state files.
    let _ = std::fs::remove_file(&paths.worker_pid);
    let _ = std::fs::remove_file(&paths.supervisor_pid);
    let _ = std::fs::remove_file(&paths.state);

    if had_state {
        println!("skillfs: stopped managed mount at {}", normalized.display());
    } else if is_mounted(&normalized) {
        // No managed instance recorded, but something is mounted here — still
        // best-effort unmount so `stop` is a reliable teardown.
        force_unmount(&normalized);
        println!("skillfs: unmounted {}", normalized.display());
    } else {
        println!(
            "skillfs: no managed mount at {} (already stopped)",
            normalized.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_id_is_stable_for_same_path() {
        let p = Path::new("/tmp/skillfs-test-mount");
        let a = instance_id_for(p);
        let b = instance_id_for(p);
        assert_eq!(a, b, "instance id must be deterministic");
    }

    #[test]
    fn instance_id_differs_for_different_paths() {
        let a = instance_id_for(Path::new("/tmp/mount-a"));
        let b = instance_id_for(Path::new("/tmp/mount-b"));
        assert_ne!(a, b);
    }

    #[test]
    fn instance_id_includes_sanitized_basename() {
        let id = instance_id_for(Path::new("/var/run/my.mount point"));
        // Non-alphanumerics in the basename become dashes.
        assert!(id.starts_with("my-mount-point-"), "got: {id}");
    }

    #[test]
    fn build_worker_args_drops_managed_and_adds_foreground() {
        let raw = vec![
            "mount".to_string(),
            "/src".to_string(),
            "/mnt".to_string(),
            "--managed".to_string(),
        ];
        let out = build_worker_args(&raw);
        assert!(!out.iter().any(|a| a == "--managed"));
        assert_eq!(out, vec!["mount", "--foreground", "/src", "/mnt"]);
    }

    #[test]
    fn build_worker_args_keeps_existing_foreground_without_duplicating() {
        let raw = vec![
            "mount".to_string(),
            "--foreground".to_string(),
            "/src".to_string(),
            "/mnt".to_string(),
            "--managed".to_string(),
        ];
        let out = build_worker_args(&raw);
        assert_eq!(out.iter().filter(|a| *a == "--foreground").count(), 1);
        assert!(!out.iter().any(|a| a == "--managed"));
    }

    #[test]
    fn build_worker_args_preserves_other_flags() {
        let raw = vec![
            "mount".to_string(),
            "/src".to_string(),
            "/mnt".to_string(),
            "--managed".to_string(),
            "--security-mode".to_string(),
            "--audit-log".to_string(),
            "/var/log/a.jsonl".to_string(),
        ];
        let out = build_worker_args(&raw);
        assert!(out.iter().any(|a| a == "--security-mode"));
        assert!(out.iter().any(|a| a == "--audit-log"));
        assert!(out.iter().any(|a| a == "/var/log/a.jsonl"));
    }

    #[test]
    fn state_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = ManagedState {
            schema_version: STATE_SCHEMA_VERSION,
            instance_id: "mnt-000000000000dead".to_string(),
            mountpoint: "/mnt/skillfs".to_string(),
            source: "/srv/skills".to_string(),
            worker_program: "/usr/bin/skillfs".to_string(),
            worker_args: vec![
                "mount".to_string(),
                "--foreground".to_string(),
                "/srv/skills".to_string(),
                "/mnt/skillfs".to_string(),
            ],
            desired_state: DesiredState::Mounted,
        };
        state.save(&path).unwrap();
        let loaded = ManagedState::load(&path).unwrap();
        assert_eq!(loaded.instance_id, state.instance_id);
        assert_eq!(loaded.desired_state, DesiredState::Mounted);
        assert_eq!(loaded.worker_args, state.worker_args);
    }

    #[test]
    fn desired_state_serializes_lowercase() {
        let json = serde_json::to_string(&DesiredState::Stopped).unwrap();
        assert_eq!(json, "\"stopped\"");
        let parsed: DesiredState = serde_json::from_str("\"mounted\"").unwrap();
        assert_eq!(parsed, DesiredState::Mounted);
    }

    #[test]
    fn current_desired_state_missing_file_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(current_desired_state(&missing), DesiredState::Stopped);
    }

    #[test]
    fn runtime_dir_prefers_xdg_runtime_dir() {
        // Safe: single-threaded unit test process; we restore afterward.
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/12345");
        }
        assert_eq!(runtime_dir(), PathBuf::from("/run/user/12345/skillfs"));
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }
}
