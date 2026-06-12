//! cosh-cli wrapper tools: thin adapters around `cosh pkg/svc/checkpoint`
//! subcommands. Each tool spawns the `cosh` binary, captures its JSON
//! stdout (via `CoshResponse` envelope), and returns it verbatim to the
//! LLM — the model then parses the structured response directly.
//!
//! cosh-tui is the reference agent consumer of cosh-cli. By funneling
//! system operations through structured tools, the LLM sees deterministic
//! JSON (state, pid, memory, recent_logs, ...) instead of unstructured
//! shell output, which it parses much more reliably.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{Tool, ToolRegistry};

/// Locate the `cosh` binary. Priority:
/// 1. `COSH_BIN` environment variable
/// 2. Sibling of the current executable (shipped-together install)
/// 3. `which cosh` via `$PATH`
pub fn locate_cosh_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("COSH_BIN") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("cosh");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    which::which("cosh").ok()
}

/// Timeout for cosh CLI invocations (seconds). The binary has its own
/// internal command timeouts (120s pkg / 30s svc), so this outer limit
/// is a safety net against unexpected hangs (e.g. IPC stalls).
const COSH_CLI_TIMEOUT_SECS: u64 = 180;

/// Maximum bytes of stdout/stderr we feed back to the LLM per cosh
/// invocation. `cosh pkg list` on a fully-populated system can emit
/// hundreds of KiB of structured JSON; if we relay all of it the LLM
/// blows past its context window and the response either gets truncated
/// at the wrong place or rejected outright. 64 KiB is large enough for
/// the legitimate JSON envelopes cosh-cli currently produces and small
/// enough that no single tool call dominates the model's window.
const COSH_CLI_MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Run `cosh <args...>` and return its stdout (the `CoshResponse` JSON).
/// If the binary cannot be located, returns a friendly error string.
pub fn run_cosh(args: &[&str]) -> Result<String, String> {
    let bin = locate_cosh_binary().ok_or_else(|| {
        "cosh binary not found. Set COSH_BIN env var or ensure `cosh` is in PATH.".to_string()
    })?;

    let mut child = std::process::Command::new(&bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn cosh: {}", e))?;

    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(COSH_CLI_TIMEOUT_SECS);

    let stdout_handle = child.stdout.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
            buf
        })
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "cosh command timed out after {}s",
                        COSH_CLI_TIMEOUT_SECS
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("failed to wait for cosh: {}", e)),
        }
    };

    let stdout_bytes = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr_bytes = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stdout = truncate_lossy(&stdout_bytes, COSH_CLI_MAX_OUTPUT_BYTES);
    let stderr = truncate_lossy(&stderr_bytes, COSH_CLI_MAX_OUTPUT_BYTES);

    // cosh-cli uses exit(1) for structured errors — not a spawn failure.
    let _ = status;

    validate_cosh_output(&stdout, &stderr)
}

/// Decode UTF-8 (lossy) and truncate to `max` bytes, appending a marker
/// when truncation actually happened. Keeps the LLM context bounded for
/// chatty subsystems like `cosh pkg list`.
fn truncate_lossy(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let head = String::from_utf8_lossy(&bytes[..max]).into_owned();
    format!("{}\n...[truncated {} bytes]", head, bytes.len() - max)
}

/// Validate that cosh's captured output is a parseable `CoshResponse` JSON.
///
/// Returns:
/// - `Ok(stdout)` when stdout parses as JSON (the contract cosh-cli promises).
/// - `Ok(synthetic_envelope)` when stdout is non-empty but NOT valid JSON.
///   We wrap the raw stdout/stderr inside a synthetic
///   `{"ok":false,"error":{"code":"E_NON_JSON",...}}` envelope so the LLM
///   sees a structured tool-side failure instead of being misled by raw
///   help/error text (which it tends to mis-attribute as "permission denied").
/// - `Err(msg)` only when there is literally nothing to feed back (stdout
///   empty); the caller surfaces that as a regular tool error.
pub fn validate_cosh_output(stdout: &str, stderr: &str) -> Result<String, String> {
    if stdout.trim().is_empty() {
        if !stderr.trim().is_empty() {
            return Err(format!("cosh stderr: {}", stderr.trim()));
        }
        return Err("cosh produced no output".to_string());
    }
    if serde_json::from_str::<Value>(stdout).is_ok() {
        return Ok(stdout.to_string());
    }
    // Stdout exists but is not JSON — wrap into a synthetic CoshResponse so
    // downstream (the LLM) gets a structured signal rather than free-form
    // text it might mis-interpret.
    let envelope = json!({
        "ok": false,
        "error": {
            "code": "E_NON_JSON",
            "message": "cosh-cli returned non-JSON output. The `cosh` binary on PATH may not be a current cosh-ng build, or the subcommand printed raw help/error text instead of a CoshResponse envelope.",
            "stdout": stdout,
            "stderr": stderr,
        }
    });
    Ok(envelope.to_string())
}

/// Register all cosh-cli wrapper tools into the given registry.
pub fn register_all(reg: &mut ToolRegistry) {
    // pkg subsystem
    reg.register(Box::new(PkgSearch));
    reg.register(Box::new(PkgInstall));
    reg.register(Box::new(PkgRemove));
    reg.register(Box::new(PkgList));
    // svc subsystem
    reg.register(Box::new(SvcStatus));
    reg.register(Box::new(SvcList));
    reg.register(Box::new(SvcAction));
    // checkpoint subsystem
    reg.register(Box::new(CheckpointInit));
    reg.register(Box::new(CheckpointCreate));
    reg.register(Box::new(CheckpointList));
    reg.register(Box::new(CheckpointRestore));
    reg.register(Box::new(CheckpointStatus));
    reg.register(Box::new(CheckpointDelete));
    reg.register(Box::new(CheckpointDiff));
    reg.register(Box::new(CheckpointCleanup));
    reg.register(Box::new(CheckpointRecover));
}

// ---------------------------------------------------------------------------
// pkg
// ---------------------------------------------------------------------------

pub struct PkgSearch;
impl Tool for PkgSearch {
    fn name(&self) -> &str {
        "cosh_pkg_search"
    }
    fn description(&self) -> &str {
        "Search available packages across the user's package manager \
         (dnf/apt/zypper/brew). Returns a structured JSON list including \
         name, summary, and install status."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Package name or keyword" }
            },
            "required": ["query"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("?");
        format!("cosh pkg search {}", q)
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let q = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: query".to_string())?;
        run_cosh(&["pkg", "search", q])
    }
}

pub struct PkgInstall;
impl Tool for PkgInstall {
    fn name(&self) -> &str {
        "cosh_pkg_install"
    }
    fn description(&self) -> &str {
        "Install a package. Defaults to dry_run=true for safety — the LLM \
         should preview first and only re-invoke with dry_run=false after \
         user confirmation."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "package": { "type": "string", "description": "Package name" },
                "dry_run": { "type": "boolean", "description": "Preview without installing (default: true)" }
            },
            "required": ["package"]
        })
    }
    fn is_safe(&self, args: &Value) -> bool {
        // Safe only when dry_run is true (or omitted — default is dry-run).
        args.get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
    fn preview(&self, args: &Value) -> String {
        let p = args.get("package").and_then(|v| v.as_str()).unwrap_or("?");
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        if dry {
            format!("cosh pkg install --dry-run {}", p)
        } else {
            format!("cosh pkg install {}", p)
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let p = args
            .get("package")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: package".to_string())?;
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        if dry {
            run_cosh(&["pkg", "install", p, "--dry-run"])
        } else {
            run_cosh(&["pkg", "install", p])
        }
    }
}

pub struct PkgRemove;
impl Tool for PkgRemove {
    fn name(&self) -> &str {
        "cosh_pkg_remove"
    }
    fn description(&self) -> &str {
        "Remove a package. Defaults to dry_run=true for safety."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "package": { "type": "string" },
                "dry_run": { "type": "boolean" }
            },
            "required": ["package"]
        })
    }
    fn is_safe(&self, args: &Value) -> bool {
        args.get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
    fn preview(&self, args: &Value) -> String {
        let p = args.get("package").and_then(|v| v.as_str()).unwrap_or("?");
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        if dry {
            format!("cosh pkg remove --dry-run {}", p)
        } else {
            format!("cosh pkg remove {}", p)
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let p = args
            .get("package")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: package".to_string())?;
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        if dry {
            run_cosh(&["pkg", "remove", p, "--dry-run"])
        } else {
            run_cosh(&["pkg", "remove", p])
        }
    }
}

pub struct PkgList;
impl Tool for PkgList {
    fn name(&self) -> &str {
        "cosh_pkg_list"
    }
    fn description(&self) -> &str {
        "List installed packages. Returns a structured JSON array with \
         name, version, arch, and repo for each package."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, _args: &Value) -> String {
        "cosh pkg list --installed".to_string()
    }
    fn execute(&self, _args: &Value) -> Result<String, String> {
        run_cosh(&["pkg", "list", "--installed"])
    }
}

// ---------------------------------------------------------------------------
// svc
// ---------------------------------------------------------------------------

pub struct SvcStatus;
impl Tool for SvcStatus {
    fn name(&self) -> &str {
        "cosh_svc_status"
    }
    fn description(&self) -> &str {
        "Get structured status of a systemd service. Returns JSON with \
         state (running/stopped/failed/activating/...), active, enabled, \
         pid, description, memory_bytes, and recent_logs."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "description": "Service unit name (e.g. sshd, nginx)" }
            },
            "required": ["service"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        let s = args.get("service").and_then(|v| v.as_str()).unwrap_or("?");
        format!("cosh svc status {}", s)
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let s = args
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: service".to_string())?;
        run_cosh(&["svc", "status", s])
    }
}

pub struct SvcList;
impl Tool for SvcList {
    fn name(&self) -> &str {
        "cosh_svc_list"
    }
    fn description(&self) -> &str {
        "List services, optionally filtered by state (running/failed/...)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "state": { "type": "string", "description": "Optional state filter (running, failed, ...)" }
            }
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        match args.get("state").and_then(|v| v.as_str()) {
            Some(s) => format!("cosh svc list --state {}", s),
            None => "cosh svc list".to_string(),
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        match args.get("state").and_then(|v| v.as_str()) {
            Some(s) => run_cosh(&["svc", "list", "--state", s]),
            None => run_cosh(&["svc", "list"]),
        }
    }
}

pub struct SvcAction;
impl Tool for SvcAction {
    fn name(&self) -> &str {
        "cosh_svc_action"
    }
    fn description(&self) -> &str {
        "Perform a lifecycle action on a systemd service: start, stop, \
         restart, enable, or disable. Defaults dry_run=true for the \
         mutating actions (start/stop/restart)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string" },
                "action": {
                    "type": "string",
                    "enum": ["start", "stop", "restart", "enable", "disable"]
                },
                "dry_run": { "type": "boolean" }
            },
            "required": ["service", "action"]
        })
    }
    fn is_safe(&self, args: &Value) -> bool {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        match action {
            "start" | "stop" | "restart" | "enable" | "disable" => dry,
            _ => false,
        }
    }
    fn preview(&self, args: &Value) -> String {
        let s = args.get("service").and_then(|v| v.as_str()).unwrap_or("?");
        let a = args.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        if dry {
            format!("cosh svc {} --dry-run {}", a, s)
        } else {
            format!("cosh svc {} {}", a, s)
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let s = args
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: service".to_string())?;
        let a = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: action".to_string())?;
        let dry = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true);
        match a {
            "start" | "stop" | "restart" | "enable" | "disable" => {
                if dry {
                    run_cosh(&["svc", a, s, "--dry-run"])
                } else {
                    run_cosh(&["svc", a, s])
                }
            }
            other => Err(format!(
                "invalid action '{}': must be start/stop/restart/enable/disable",
                other
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// checkpoint
// ---------------------------------------------------------------------------

fn resolve_workspace(args: &Value) -> String {
    args.get("workspace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string())
        })
}

pub struct CheckpointInit;
impl Tool for CheckpointInit {
    fn name(&self) -> &str {
        "cosh_checkpoint_init"
    }
    fn description(&self) -> &str {
        "Initialize a workspace for checkpointing via ws-ckpt. Must be \
         called before creating checkpoints in a new workspace."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "description": "Workspace path (default: cwd)" }
            }
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        format!("cosh checkpoint init --workspace {}", resolve_workspace(args))
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        run_cosh(&["checkpoint", "init", "--workspace", &w])
    }
}

pub struct CheckpointCreate;
impl Tool for CheckpointCreate {
    fn name(&self) -> &str {
        "cosh_checkpoint_create"
    }
    fn description(&self) -> &str {
        "Create a workspace checkpoint via ws-ckpt. If `workspace` is \
         omitted, the current working directory is used."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "description": "Workspace path (default: cwd)" },
                "id": { "type": "string", "description": "Snapshot ID" },
                "message": { "type": "string", "description": "Optional checkpoint message" }
            },
            "required": ["id"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        // Creating a snapshot is low-risk (reversible).
        true
    }
    fn preview(&self, args: &Value) -> String {
        let w = resolve_workspace(args);
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let m = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        if m.is_empty() {
            format!("cosh checkpoint create --workspace {} --id {}", w, id)
        } else {
            format!("cosh checkpoint create --workspace {} --id {} -m '{}'", w, id, m)
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: id".to_string())?;
        match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => run_cosh(&["checkpoint", "create", "--workspace", &w, "--id", id, "-m", m]),
            None => run_cosh(&["checkpoint", "create", "--workspace", &w, "--id", id]),
        }
    }
}

pub struct CheckpointList;
impl Tool for CheckpointList {
    fn name(&self) -> &str {
        "cosh_checkpoint_list"
    }
    fn description(&self) -> &str {
        "List all checkpoints for a workspace. Defaults to cwd."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string" }
            }
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        format!("cosh checkpoint list --workspace {}", resolve_workspace(args))
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        run_cosh(&["checkpoint", "list", "--workspace", &w])
    }
}

pub struct CheckpointRestore;
impl Tool for CheckpointRestore {
    fn name(&self) -> &str {
        "cosh_checkpoint_restore"
    }
    fn description(&self) -> &str {
        "Restore a specific checkpoint by ID. Always requires user approval \
         since this mutates workspace contents."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "workspace": { "type": "string" }
            },
            "required": ["id"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        false
    }
    fn preview(&self, args: &Value) -> String {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let w = resolve_workspace(args);
        format!("cosh checkpoint restore {} --workspace {}", id, w)
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: id".to_string())?;
        let w = resolve_workspace(args);
        run_cosh(&["checkpoint", "restore", id, "--workspace", &w])
    }
}

pub struct CheckpointStatus;
impl Tool for CheckpointStatus {
    fn name(&self) -> &str {
        "cosh_checkpoint_status"
    }
    fn description(&self) -> &str {
        "Show checkpoint daemon status and stats."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string" }
            }
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        match args.get("workspace").and_then(|v| v.as_str()) {
            Some(w) => format!("cosh checkpoint status --workspace {}", w),
            None => "cosh checkpoint status".into(),
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        match args.get("workspace").and_then(|v| v.as_str()) {
            Some(w) => run_cosh(&["checkpoint", "status", "--workspace", w]),
            None => run_cosh(&["checkpoint", "status"]),
        }
    }
}

pub struct CheckpointDelete;
impl Tool for CheckpointDelete {
    fn name(&self) -> &str {
        "cosh_checkpoint_delete"
    }
    fn description(&self) -> &str {
        "Delete a specific checkpoint snapshot. Requires user approval as \
         the snapshot data is permanently removed."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "snapshot": { "type": "string", "description": "Snapshot ID to delete" },
                "workspace": { "type": "string", "description": "Workspace path (optional)" },
                "force": { "type": "boolean", "description": "Force deletion without confirmation (default: false)" }
            },
            "required": ["snapshot"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        false
    }
    fn preview(&self, args: &Value) -> String {
        let s = args.get("snapshot").and_then(|v| v.as_str()).unwrap_or("?");
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut cmd = format!("cosh checkpoint delete -s {}", s);
        if let Some(w) = args.get("workspace").and_then(|v| v.as_str()) {
            cmd.push_str(&format!(" --workspace {}", w));
        }
        if force {
            cmd.push_str(" --force");
        }
        cmd
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let s = args
            .get("snapshot")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: snapshot".to_string())?;
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut cmd_args = vec!["checkpoint", "delete", "-s", s];
        let w_owned;
        if let Some(w) = args.get("workspace").and_then(|v| v.as_str()) {
            w_owned = w.to_string();
            cmd_args.push("--workspace");
            cmd_args.push(&w_owned);
        }
        if force {
            cmd_args.push("--force");
        }
        run_cosh(&cmd_args)
    }
}

pub struct CheckpointDiff;
impl Tool for CheckpointDiff {
    fn name(&self) -> &str {
        "cosh_checkpoint_diff"
    }
    fn description(&self) -> &str {
        "Show diff between two checkpoint snapshots in a workspace. \
         Returns a structured list of changed files."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "description": "Workspace path" },
                "from": { "type": "string", "description": "Source snapshot ID" },
                "to": { "type": "string", "description": "Target snapshot ID" }
            },
            "required": ["workspace", "from", "to"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        let w = resolve_workspace(args);
        let from = args.get("from").and_then(|v| v.as_str()).unwrap_or("?");
        let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("?");
        format!("cosh checkpoint diff --workspace {} -f {} -t {}", w, from, to)
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: from".to_string())?;
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing arg: to".to_string())?;
        run_cosh(&["checkpoint", "diff", "--workspace", &w, "-f", from, "-t", to])
    }
}

pub struct CheckpointCleanup;
impl Tool for CheckpointCleanup {
    fn name(&self) -> &str {
        "cosh_checkpoint_cleanup"
    }
    fn description(&self) -> &str {
        "Cleanup old checkpoint snapshots in a workspace. Optionally specify \
         how many to keep. Requires user approval as snapshots are deleted."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "description": "Workspace path" },
                "keep": { "type": "integer", "description": "Number of snapshots to keep (optional)" }
            },
            "required": ["workspace"]
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        false
    }
    fn preview(&self, args: &Value) -> String {
        let w = resolve_workspace(args);
        match args.get("keep").and_then(|v| v.as_u64()) {
            Some(k) => format!("cosh checkpoint cleanup --workspace {} --keep {}", w, k),
            None => format!("cosh checkpoint cleanup --workspace {}", w),
        }
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        match args.get("keep").and_then(|v| v.as_u64()) {
            Some(k) => {
                let k_str = k.to_string();
                run_cosh(&["checkpoint", "cleanup", "--workspace", &w, "--keep", &k_str])
            }
            None => run_cosh(&["checkpoint", "cleanup", "--workspace", &w]),
        }
    }
}

pub struct CheckpointRecover;
impl Tool for CheckpointRecover {
    fn name(&self) -> &str {
        "cosh_checkpoint_recover"
    }
    fn description(&self) -> &str {
        "Recover a workspace's checkpoint state after a daemon restart or \
         crash. Re-registers the workspace with the checkpoint daemon."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "description": "Workspace path (default: cwd)" }
            }
        })
    }
    fn is_safe(&self, _: &Value) -> bool {
        true
    }
    fn preview(&self, args: &Value) -> String {
        format!("cosh checkpoint recover --workspace {}", resolve_workspace(args))
    }
    fn execute(&self, args: &Value) -> Result<String, String> {
        let w = resolve_workspace(args);
        run_cosh(&["checkpoint", "recover", "--workspace", &w])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locate_respects_env_override_missing_file() {
        // An env var pointing to a non-existent file should fall through.
        std::env::set_var("COSH_BIN", "/definitely/not/a/real/path/cosh-xyz");
        // Cannot assert exact result without a real `cosh` on PATH, but
        // it must not panic and must not return the fake path.
        let located = locate_cosh_binary();
        if let Some(p) = located {
            assert_ne!(p, PathBuf::from("/definitely/not/a/real/path/cosh-xyz"));
        }
        std::env::remove_var("COSH_BIN");
    }

    #[test]
    fn test_resolve_workspace_defaults_to_cwd() {
        let v = json!({});
        let w = resolve_workspace(&v);
        assert!(!w.is_empty());
    }

    #[test]
    fn test_resolve_workspace_uses_arg() {
        let v = json!({"workspace": "/tmp/foo"});
        assert_eq!(resolve_workspace(&v), "/tmp/foo");
    }

    #[test]
    fn test_pkg_search_metadata() {
        let t = PkgSearch;
        assert_eq!(t.name(), "cosh_pkg_search");
        assert!(t.is_safe(&json!({})));
        assert_eq!(t.preview(&json!({"query": "vim"})), "cosh pkg search vim");
    }

    #[test]
    fn test_pkg_install_is_safe_default_dry_run() {
        let t = PkgInstall;
        assert!(t.is_safe(&json!({"package": "vim"})));
        assert!(t.is_safe(&json!({"package": "vim", "dry_run": true})));
        assert!(!t.is_safe(&json!({"package": "vim", "dry_run": false})));
    }

    #[test]
    fn test_pkg_install_preview_shows_dry_run() {
        let t = PkgInstall;
        assert!(t.preview(&json!({"package": "vim"})).contains("--dry-run"));
        assert!(!t
            .preview(&json!({"package": "vim", "dry_run": false}))
            .contains("--dry-run"));
    }

    #[test]
    fn test_pkg_remove_missing_arg() {
        let t = PkgRemove;
        assert!(t.execute(&json!({})).is_err());
    }

    #[test]
    fn test_svc_status_is_safe() {
        let t = SvcStatus;
        assert!(t.is_safe(&json!({})));
        assert_eq!(
            t.preview(&json!({"service": "sshd"})),
            "cosh svc status sshd"
        );
    }

    #[test]
    fn test_svc_list_no_args_preview() {
        let t = SvcList;
        assert_eq!(t.preview(&json!({})), "cosh svc list");
        assert_eq!(
            t.preview(&json!({"state": "running"})),
            "cosh svc list --state running"
        );
    }

    #[test]
    fn test_svc_action_enable_disable_dry_run() {
        let t = SvcAction;
        // default dry_run=true → safe
        assert!(t.is_safe(&json!({"service":"x","action":"enable"})));
        assert!(t.is_safe(&json!({"service":"x","action":"disable"})));
        assert!(t.is_safe(&json!({"service":"x","action":"enable","dry_run":true})));
        // explicit dry_run=false → unsafe
        assert!(!t.is_safe(&json!({"service":"x","action":"enable","dry_run":false})));
        assert!(!t.is_safe(&json!({"service":"x","action":"disable","dry_run":false})));
    }

    #[test]
    fn test_svc_action_start_dry_run_is_safe() {
        let t = SvcAction;
        assert!(t.is_safe(&json!({"service":"x","action":"start"}))); // default dry_run=true
        assert!(t.is_safe(&json!({"service":"x","action":"start","dry_run":true})));
        assert!(!t.is_safe(&json!({"service":"x","action":"start","dry_run":false})));
    }

    #[test]
    fn test_svc_action_invalid() {
        let t = SvcAction;
        let err = t
            .execute(&json!({"service":"x","action":"bogus"}))
            .unwrap_err();
        assert!(err.contains("invalid action") || err.contains("not found"));
    }

    #[test]
    fn test_checkpoint_restore_requires_id() {
        let t = CheckpointRestore;
        assert!(t.execute(&json!({})).is_err());
    }

    #[test]
    fn test_checkpoint_restore_is_unsafe() {
        let t = CheckpointRestore;
        assert!(!t.is_safe(&json!({"id":"x"})));
    }

    #[test]
    fn test_checkpoint_status_is_safe() {
        let t = CheckpointStatus;
        assert!(t.is_safe(&json!({})));
    }

    #[test]
    fn test_checkpoint_list_preview_uses_cwd() {
        let t = CheckpointList;
        let p = t.preview(&json!({}));
        assert!(p.starts_with("cosh checkpoint list --workspace"));
    }

    #[test]
    fn test_pkg_list_is_safe() {
        let t = PkgList;
        assert!(t.is_safe(&json!({})));
        assert_eq!(t.preview(&json!({})), "cosh pkg list --installed");
    }

    #[test]
    fn test_register_all_adds_expected_count() {
        let mut reg = ToolRegistry::empty();
        register_all(&mut reg);
        assert_eq!(reg.len(), 16); // 4 pkg + 3 svc + 9 checkpoint
        assert!(reg.find("cosh_pkg_search").is_some());
        assert!(reg.find("cosh_pkg_list").is_some());
        assert!(reg.find("cosh_svc_action").is_some());
        assert!(reg.find("cosh_checkpoint_restore").is_some());
        assert!(reg.find("cosh_checkpoint_init").is_some());
        assert!(reg.find("cosh_checkpoint_delete").is_some());
        assert!(reg.find("cosh_checkpoint_diff").is_some());
        assert!(reg.find("cosh_checkpoint_cleanup").is_some());
        assert!(reg.find("cosh_checkpoint_recover").is_some());
    }

    // ---- validate_cosh_output ------------------------------------------

    #[test]
    fn validate_passes_through_valid_json() {
        let stdout = r#"{"ok":true,"data":{"matches":[]}}"#;
        let out = validate_cosh_output(stdout, "").unwrap();
        assert_eq!(out, stdout);
    }

    #[test]
    fn validate_passes_through_valid_error_envelope() {
        // cosh-cli's own ok:false response is still valid JSON — must NOT be
        // re-wrapped by us.
        let stdout = r#"{"ok":false,"error":{"code":"E_PKG_NOT_FOUND","message":"x"}}"#;
        let out = validate_cosh_output(stdout, "").unwrap();
        assert_eq!(out, stdout);
    }

    #[test]
    fn validate_wraps_non_json_stdout_into_envelope() {
        let raw = "如果是 RHEL/CentOS:\n  yum search nginx";
        let out = validate_cosh_output(raw, "").unwrap();
        let v: Value = serde_json::from_str(&out).expect("wrapped output must be valid JSON");
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"]["code"], json!("E_NON_JSON"));
        assert_eq!(v["error"]["stdout"], json!(raw));
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("non-JSON"));
    }

    #[test]
    fn validate_wraps_includes_stderr_for_debugging() {
        let stderr = "warning: deprecated flag";
        let out = validate_cosh_output("plain text", stderr).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["stderr"], json!(stderr));
    }

    #[test]
    fn validate_empty_stdout_uses_stderr_as_error() {
        let err = validate_cosh_output("", "backend not found").unwrap_err();
        assert!(err.contains("backend not found"));
    }

    #[test]
    fn validate_empty_stdout_and_stderr_returns_generic_error() {
        let err = validate_cosh_output("   \n", "").unwrap_err();
        assert!(err.contains("no output"));
    }

    #[test]
    fn truncate_lossy_short_input_unchanged() {
        let s = truncate_lossy(b"short", 100);
        assert_eq!(s, "short");
    }

    #[test]
    fn truncate_lossy_long_input_marks_truncation() {
        let big = vec![b'x'; 200];
        let s = truncate_lossy(&big, 32);
        assert!(s.starts_with("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"));
        assert!(s.contains("truncated 168 bytes"));
    }

    #[test]
    fn validate_envelope_round_trips_to_real_cosh_response_shape() {
        // The wrapped envelope must satisfy `{ok:bool, error:{code, message}}`
        // — the same minimal shape downstream consumers (the LLM, future
        // CoshResponse decoders) expect.
        let out = validate_cosh_output("not json", "").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["ok"].as_bool().is_some());
        assert!(v["error"]["code"].as_str().is_some());
        assert!(v["error"]["message"].as_str().is_some());
    }

    // ---- new checkpoint tools ------------------------------------------

    #[test]
    fn test_checkpoint_init_is_safe() {
        let t = CheckpointInit;
        assert!(t.is_safe(&json!({})));
        assert!(t.preview(&json!({})).starts_with("cosh checkpoint init"));
    }

    #[test]
    fn test_checkpoint_delete_is_unsafe() {
        let t = CheckpointDelete;
        assert!(!t.is_safe(&json!({"snapshot": "snap-1"})));
    }

    #[test]
    fn test_checkpoint_delete_requires_snapshot() {
        let t = CheckpointDelete;
        assert!(t.execute(&json!({})).is_err());
    }

    #[test]
    fn test_checkpoint_delete_preview() {
        let t = CheckpointDelete;
        let p = t.preview(&json!({"snapshot": "s1", "workspace": "/w", "force": true}));
        assert!(p.contains("-s s1"));
        assert!(p.contains("--workspace /w"));
        assert!(p.contains("--force"));
    }

    #[test]
    fn test_checkpoint_diff_is_safe() {
        let t = CheckpointDiff;
        assert!(t.is_safe(&json!({})));
    }

    #[test]
    fn test_checkpoint_diff_requires_all_args() {
        let t = CheckpointDiff;
        assert!(t.execute(&json!({"workspace": "/w"})).is_err());
        assert!(t.execute(&json!({"workspace": "/w", "from": "a"})).is_err());
    }

    #[test]
    fn test_checkpoint_diff_preview() {
        let t = CheckpointDiff;
        let p = t.preview(&json!({"workspace": "/w", "from": "a", "to": "b"}));
        assert!(p.contains("-f a"));
        assert!(p.contains("-t b"));
    }

    #[test]
    fn test_checkpoint_cleanup_is_unsafe() {
        let t = CheckpointCleanup;
        assert!(!t.is_safe(&json!({})));
    }

    #[test]
    fn test_checkpoint_cleanup_preview_with_keep() {
        let t = CheckpointCleanup;
        let p = t.preview(&json!({"workspace": "/w", "keep": 5}));
        assert!(p.contains("--keep 5"));
    }

    #[test]
    fn test_checkpoint_recover_is_safe() {
        let t = CheckpointRecover;
        assert!(t.is_safe(&json!({})));
        assert!(t.preview(&json!({})).starts_with("cosh checkpoint recover"));
    }
}
