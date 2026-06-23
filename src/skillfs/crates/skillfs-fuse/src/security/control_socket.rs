//! Trusted peer control socket.
//!
//! Provides a Unix domain socket control plane authenticated via
//! `SO_PEERCRED` + executable identity. External daemons connect to
//! this socket; SkillFS verifies the peer's pid/uid/gid and resolves
//! the peer's `/proc/<pid>/exe` to match a pinned `(dev, ino)`.
//!
//! ## Protocol
//!
//! JSONL over the Unix socket: one JSON object per line.
//!
//! Request:
//! ```json
//! {"schemaVersion":"1","method":"ping"}
//! {"schemaVersion":"1","method":"status"}
//! {"schemaVersion":"1","method":"meta.writeActivation","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
//! {"schemaVersion":"1","method":"meta.setActivationXattr","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
//! ```
//!
//! Response:
//! ```json
//! {"schemaVersion":"1","ok":true,"result":{"pong":true}}
//! {"schemaVersion":"1","ok":true,"result":{"status":"ready"}}
//! {"schemaVersion":"1","ok":true,"result":{"outcome":"updated"}}
//! {"schemaVersion":"1","ok":false,"error":{"code":"permission_denied","message":"..."}}
//! ```
//!
//! ## Security
//!
//! - Socket file permissions: `0o600` (owner-only).
//! - Peer credentials obtained via `SO_PEERCRED` (`getsockopt`).
//! - Peer executable resolved via `/proc/<pid>/exe` readlink + stat.
//! - Both credential and executable identity must match configuration.
//! - Failed verification returns an error and closes the connection.
//! - Linux-only; non-Linux targets fail at startup when configured.

use std::io::{BufRead, BufReader, Read as IoRead, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{debug, info, warn};

use super::activation::{ACTIVATION_XATTR, ActivationRecord};
use super::activation_reload::ReloadOutcome;
use super::active::{ActiveSkillResolver, ActiveTarget};
use super::ledger::validate_skill_name_component;
use super::protocol_events::{ProtocolEvent, ProtocolEventWriter};
use super::trusted_writer::FileId;

// ─────────────────────────────────────────────────────────────────────────────
// Protocol constants
// ─────────────────────────────────────────────────────────────────────────────

pub const CONTROL_SCHEMA_VERSION: &str = "1";

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ControlSocketConfig {
    pub socket_path: PathBuf,
    pub trusted_peer: TrustedPeerConfig,
}

#[derive(Debug, Clone)]
pub struct TrustedPeerConfig {
    pub exe_path: PathBuf,
    pub exe_file_id: FileId,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

/// Runtime context for methods that need filesystem access
/// (e.g. `meta.writeActivation`, `meta.setActivationXattr`).
///
/// Passed through `ControlSocketServer::new()` and threaded into
/// `handle_connection()` so write methods can access the source root,
/// active resolver, and protocol event writer. Read-only methods
/// (`ping`, `status`) ignore the context.
#[derive(Clone)]
pub struct ControlSocketContext {
    pub source_root: PathBuf,
    pub resolver: Option<Arc<ActiveSkillResolver>>,
    pub protocol_event_writer: Option<Arc<dyn ProtocolEventWriter>>,
}

impl std::fmt::Debug for ControlSocketContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlSocketContext")
            .field("source_root", &self.source_root)
            .field("resolver", &self.resolver.is_some())
            .field(
                "protocol_event_writer",
                &self.protocol_event_writer.is_some(),
            )
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Peer identity types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    pub credentials: PeerCredentials,
    pub exe_path: Option<PathBuf>,
    pub exe_file_id: Option<FileId>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Protocol types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRequest {
    pub schema_version: String,
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlResponse {
    pub schema_version: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}

impl ControlResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            schema_version: CONTROL_SCHEMA_VERSION.to_string(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            schema_version: CONTROL_SCHEMA_VERSION.to_string(),
            ok: false,
            result: None,
            error: Some(ControlError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Peer credential resolution
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub fn get_peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };

    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(PeerCredentials {
        pid: cred.pid as u32,
        uid: cred.uid,
        gid: cred.gid,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn get_peer_credentials(_stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SO_PEERCRED is only available on Linux",
    ))
}

/// Resolve peer executable identity from `/proc/<pid>/exe`.
///
/// The display path comes from `readlink` (human-readable), but the
/// file identity `(dev, ino)` is obtained by statting the proc symlink
/// itself (with follow), NOT the resolved path string. This avoids a
/// TOCTOU where the path is replaced between readlink and stat.
#[cfg(target_os = "linux")]
pub fn resolve_peer_exe(pid: u32) -> Option<(PathBuf, FileId)> {
    use std::os::unix::fs::MetadataExt;

    let exe_link = PathBuf::from(format!("/proc/{pid}/exe"));
    let exe_path = std::fs::read_link(&exe_link).ok()?;
    // stat the proc symlink (follows to the running exe inode), not
    // the resolved path string which could race with replacement.
    let meta = std::fs::metadata(&exe_link).ok()?;
    Some((
        exe_path,
        FileId {
            dev: meta.dev(),
            ino: meta.ino(),
        },
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn resolve_peer_exe(_pid: u32) -> Option<(PathBuf, FileId)> {
    None
}

/// Build a full [`PeerIdentity`] from a connected stream.
pub fn identify_peer(stream: &UnixStream) -> std::io::Result<PeerIdentity> {
    let creds = get_peer_credentials(stream)?;
    let (exe_path, exe_file_id) = match resolve_peer_exe(creds.pid) {
        Some((p, fid)) => (Some(p), Some(fid)),
        None => (None, None),
    };
    Ok(PeerIdentity {
        credentials: creds,
        exe_path,
        exe_file_id,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Peer verification
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerVerifyResult {
    Accepted,
    DeniedUidMismatch { expected: u32, actual: u32 },
    DeniedGidMismatch { expected: u32, actual: u32 },
    DeniedExeUnresolved,
    DeniedExePathMismatch { expected: PathBuf, actual: PathBuf },
    DeniedExeFileIdMismatch { expected: FileId, actual: FileId },
}

impl PeerVerifyResult {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    pub fn denial_message(&self) -> Option<String> {
        match self {
            Self::Accepted => None,
            Self::DeniedUidMismatch { expected, actual } => {
                Some(format!("uid mismatch: expected {expected}, got {actual}"))
            }
            Self::DeniedGidMismatch { expected, actual } => {
                Some(format!("gid mismatch: expected {expected}, got {actual}"))
            }
            Self::DeniedExeUnresolved => Some("peer executable could not be resolved".to_string()),
            Self::DeniedExePathMismatch { expected, actual } => Some(format!(
                "exe path mismatch: expected {}, got {}",
                expected.display(),
                actual.display()
            )),
            Self::DeniedExeFileIdMismatch { expected, actual } => Some(format!(
                "exe file id mismatch: expected {expected}, got {actual}"
            )),
        }
    }
}

pub fn verify_peer(config: &TrustedPeerConfig, identity: &PeerIdentity) -> PeerVerifyResult {
    if let Some(expected_uid) = config.uid {
        if identity.credentials.uid != expected_uid {
            return PeerVerifyResult::DeniedUidMismatch {
                expected: expected_uid,
                actual: identity.credentials.uid,
            };
        }
    }

    if let Some(expected_gid) = config.gid {
        if identity.credentials.gid != expected_gid {
            return PeerVerifyResult::DeniedGidMismatch {
                expected: expected_gid,
                actual: identity.credentials.gid,
            };
        }
    }

    let actual_path = match identity.exe_path.as_ref() {
        Some(p) => p,
        None => return PeerVerifyResult::DeniedExeUnresolved,
    };
    let actual_fid = match identity.exe_file_id {
        Some(fid) => fid,
        None => return PeerVerifyResult::DeniedExeUnresolved,
    };

    let actual_canon = std::fs::canonicalize(actual_path)
        .ok()
        .unwrap_or_else(|| actual_path.clone());

    if actual_canon != config.exe_path {
        return PeerVerifyResult::DeniedExePathMismatch {
            expected: config.exe_path.clone(),
            actual: actual_canon,
        };
    }

    if actual_fid != config.exe_file_id {
        return PeerVerifyResult::DeniedExeFileIdMismatch {
            expected: config.exe_file_id,
            actual: actual_fid,
        };
    }

    PeerVerifyResult::Accepted
}

// ─────────────────────────────────────────────────────────────────────────────
// Request dispatch
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_request(line: &str) -> Result<ControlRequest, ControlResponse> {
    let (req, _raw) = parse_request_with_raw(line)?;
    Ok(req)
}

pub fn parse_request_with_raw(
    line: &str,
) -> Result<(ControlRequest, serde_json::Value), ControlResponse> {
    let raw: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| ControlResponse::err("invalid_request", format!("JSON parse error: {e}")))?;
    let req: ControlRequest = serde_json::from_value(raw.clone())
        .map_err(|e| ControlResponse::err("invalid_request", format!("JSON parse error: {e}")))?;
    if req.schema_version != CONTROL_SCHEMA_VERSION {
        return Err(ControlResponse::err(
            "unsupported_schema_version",
            format!(
                "unsupported schemaVersion '{}'; expected '{CONTROL_SCHEMA_VERSION}'",
                req.schema_version
            ),
        ));
    }
    Ok((req, raw))
}

pub fn dispatch_request(
    req: &ControlRequest,
    raw: &serde_json::Value,
    ctx: Option<&ControlSocketContext>,
) -> ControlResponse {
    match req.method.as_str() {
        "ping" => ControlResponse::ok(serde_json::json!({ "pong": true })),
        "status" => ControlResponse::ok(serde_json::json!({ "status": "ready" })),
        "meta.writeActivation" => dispatch_meta_write_activation(raw, ctx),
        "meta.setActivationXattr" => dispatch_meta_set_activation_xattr(raw, ctx),
        other => ControlResponse::err("unknown_method", format!("unknown method '{other}'")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Meta write: shared validation
// ─────────────────────────────────────────────────────────────────────────────

fn extract_and_validate_meta_request<'a>(
    raw: &'a serde_json::Value,
    ctx: Option<&ControlSocketContext>,
) -> Result<(&'a str, String, PathBuf), ControlResponse> {
    let ctx = ctx.ok_or_else(|| {
        ControlResponse::err(
            "not_configured",
            "meta write methods require a configured source root",
        )
    })?;

    if ctx.resolver.is_none() {
        return Err(ControlResponse::err(
            "not_configured",
            "meta write methods require an active resolver (--security --activation-mode file)",
        ));
    }

    let skill_name = raw
        .get("skillName")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ControlResponse::err("invalid_request", "missing or non-string 'skillName' field")
        })?;

    validate_skill_name_component(skill_name)
        .map_err(|e| ControlResponse::err("invalid_skill_name", e.to_string()))?;

    let activation_value = raw
        .get("activation")
        .ok_or_else(|| ControlResponse::err("invalid_request", "missing 'activation' field"))?;

    let activation_json = serde_json::to_string(activation_value).map_err(|e| {
        ControlResponse::err(
            "invalid_activation",
            format!("cannot serialize activation: {e}"),
        )
    })?;

    ActivationRecord::from_json_str(&activation_json)
        .map_err(|e| ControlResponse::err("invalid_activation", e.to_string()))?;

    let skill_dir = ctx.source_root.join(skill_name);

    Ok((skill_name, activation_json, skill_dir))
}

fn reload_and_emit(
    ctx: Option<&ControlSocketContext>,
    skill_name: &str,
    skill_dir: &Path,
    write_kind: &str,
) -> serde_json::Value {
    let mut outcome_label = "no_reload";

    if let Some(ctx) = ctx {
        if let Some(ref resolver) = ctx.resolver {
            let reload_outcome = reload_skill_once_into(resolver, &ctx.source_root, skill_name);
            outcome_label = match &reload_outcome {
                ReloadOutcome::Updated(_) => "updated",
                ReloadOutcome::Unchanged => "unchanged",
                ReloadOutcome::Timeout => "timeout",
                ReloadOutcome::FailSafeHidden { .. } => "fail_safe_hidden",
            };
        }

        if let Some(ref writer) = ctx.protocol_event_writer {
            let event = ProtocolEvent::new(
                skill_dir.to_string_lossy().to_string(),
                skill_name,
                write_kind,
                Vec::new(),
            );
            writer.emit(&event);

            let reload_event = ProtocolEvent::with_reload_outcome(
                skill_dir.to_string_lossy().to_string(),
                skill_name,
                &format!("activation_{outcome_label}"),
            );
            writer.emit(&reload_event);
        }
    }

    serde_json::json!({ "outcome": outcome_label })
}

fn reload_skill_once_into(
    resolver: &ActiveSkillResolver,
    source_root: &Path,
    skill_name: &str,
) -> ReloadOutcome {
    use super::activation::{fail_safe_hidden, load_activation_prefer_xattr};

    let skill_dir = source_root.join(skill_name);
    match load_activation_prefer_xattr(&skill_dir) {
        Ok(target) => {
            let prev = resolver.get(skill_name);
            let changed = match (&prev, &target) {
                (None, _) => true,
                (Some(ActiveTarget::Hidden { .. }), ActiveTarget::Hidden { .. }) => false,
                (
                    Some(ActiveTarget::Snapshot {
                        snapshot_dir: a, ..
                    }),
                    ActiveTarget::Snapshot {
                        snapshot_dir: b, ..
                    },
                ) => a != b,
                (
                    Some(ActiveTarget::Current { source_dir: a }),
                    ActiveTarget::Current { source_dir: b },
                ) => a != b,
                _ => true,
            };
            resolver.set(skill_name.to_string(), target.clone());
            if changed {
                ReloadOutcome::Updated(target)
            } else {
                ReloadOutcome::Unchanged
            }
        }
        Err(e) => {
            let hidden = fail_safe_hidden(&e);
            resolver.set(skill_name.to_string(), hidden);
            ReloadOutcome::FailSafeHidden {
                reason: e.to_string(),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// meta.writeActivation
// ─────────────────────────────────────────────────────────────────────────────

fn dispatch_meta_write_activation(
    raw: &serde_json::Value,
    ctx: Option<&ControlSocketContext>,
) -> ControlResponse {
    let (skill_name, activation_json, skill_dir) = match extract_and_validate_meta_request(raw, ctx)
    {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let source_root = &ctx.as_ref().unwrap().source_root;
    if let Err(resp) =
        atomic_write_activation_fd(source_root, skill_name, activation_json.as_bytes())
    {
        return resp;
    }

    let result = reload_and_emit(
        ctx,
        skill_name,
        &skill_dir,
        "control_plane_write_activation",
    );
    ControlResponse::ok(result)
}

/// Fully fd-anchored atomic activation write.
///
/// Opens `source_root` → `openat(skill_name, O_NOFOLLOW|O_DIRECTORY)` →
/// `mkdirat(.skill-meta)` → `openat(.skill-meta, O_NOFOLLOW|O_DIRECTORY)` →
/// `openat(tmp, O_CREAT|O_EXCL)` → write+fsync → `renameat(tmp, activation.json)` →
/// fsync dir.
///
/// Every path segment is opened with `O_NOFOLLOW` so a symlink at any
/// level (skill dir or `.skill-meta`) causes `ELOOP` rather than
/// following the link outside the source tree.
fn atomic_write_activation_fd(
    source_root: &Path,
    skill_name: &str,
    json_bytes: &[u8],
) -> Result<(), ControlResponse> {
    use std::ffi::CString;
    use std::os::unix::io::FromRawFd;

    let (_source_guard, skill_guard) = open_skill_dir_nofollow(source_root, skill_name)?;
    let skill_fd = skill_guard.0;

    // 3. Ensure .skill-meta exists via mkdirat. EEXIST is fine.
    let c_meta = CString::new(".skill-meta").unwrap();
    let rc = unsafe { libc::mkdirat(skill_fd, c_meta.as_ptr(), 0o755) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EEXIST) {
            return Err(ControlResponse::err(
                "write_failed",
                format!("failed to create .skill-meta: {e}"),
            ));
        }
    }

    // 4. Open .skill-meta relative to skill_fd with O_NOFOLLOW.
    let meta_fd = unsafe {
        libc::openat(
            skill_fd,
            c_meta.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if meta_fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to open .skill-meta (O_NOFOLLOW): {e}"),
        ));
    }
    let meta_dir_file = unsafe { std::fs::File::from_raw_fd(meta_fd) };

    // 5. Create temp file via openat on meta_fd.
    let tmp_name = format!(
        "activation.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let c_tmp = CString::new(tmp_name.as_bytes())
        .map_err(|_| ControlResponse::err("write_failed", "temp name contains NUL"))?;
    let c_target = CString::new("activation.json").unwrap();

    let tmp_fd = unsafe {
        libc::openat(
            meta_fd,
            c_tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            0o644,
        )
    };
    if tmp_fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to create temp file: {e}"),
        ));
    }

    // 6. Write, fsync, close temp file.
    let write_result = {
        let mut f = unsafe { std::fs::File::from_raw_fd(tmp_fd) };
        f.write_all(json_bytes)
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{e}"))
    };
    if let Err(msg) = write_result {
        unsafe { libc::unlinkat(meta_fd, c_tmp.as_ptr(), 0) };
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to write/fsync temp file: {msg}"),
        ));
    }

    // 7. Atomic rename via renameat on meta_fd.
    let rc = unsafe { libc::renameat(meta_fd, c_tmp.as_ptr(), meta_fd, c_target.as_ptr()) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        unsafe { libc::unlinkat(meta_fd, c_tmp.as_ptr(), 0) };
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to rename temp to activation.json: {e}"),
        ));
    }

    // 8. Best-effort fsync the directory.
    let _ = meta_dir_file.sync_all();

    Ok(())
}

/// RAII guard that closes a raw fd on drop.
struct FdGuard(libc::c_int);

impl Drop for FdGuard {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { libc::close(self.0) };
        }
    }
}

/// Open the source root as a directory fd, then open the skill
/// directory relative to it with `O_NOFOLLOW|O_DIRECTORY`.
///
/// Returns (source_guard, skill_guard) on success. On error, returns
/// a structured `ControlResponse` distinguishing symlinks, missing
/// directories, and other failures.
fn open_skill_dir_nofollow(
    source_root: &Path,
    skill_name: &str,
) -> Result<(FdGuard, FdGuard), ControlResponse> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_source = CString::new(source_root.as_os_str().as_bytes())
        .map_err(|_| ControlResponse::err("write_failed", "source root path contains NUL"))?;
    let source_fd = unsafe {
        libc::open(
            c_source.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if source_fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to open source root: {e}"),
        ));
    }
    let source_guard = FdGuard(source_fd);

    let c_skill = CString::new(skill_name.as_bytes())
        .map_err(|_| ControlResponse::err("write_failed", "skill name contains NUL"))?;
    let skill_fd = unsafe {
        libc::openat(
            source_fd,
            c_skill.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if skill_fd < 0 {
        let e = std::io::Error::last_os_error();
        let errno = e.raw_os_error().unwrap_or(0);

        // O_NOFOLLOW on a symlink returns ELOOP on some kernels,
        // ENOTDIR on others (when combined with O_DIRECTORY). Use
        // fstatat to distinguish "is a symlink" from "truly not a
        // directory" so we return the right error code.
        if errno == libc::ELOOP {
            return Err(ControlResponse::err(
                "invalid_skill_name",
                format!("skill directory '{skill_name}' is a symlink; refusing to follow"),
            ));
        }
        if errno == libc::ENOTDIR {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::fstatat(
                    source_fd,
                    c_skill.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if rc == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFLNK {
                return Err(ControlResponse::err(
                    "invalid_skill_name",
                    format!("skill directory '{skill_name}' is a symlink; refusing to follow"),
                ));
            }
            return Err(ControlResponse::err(
                "skill_not_found",
                format!("skill directory '{skill_name}' is not a directory"),
            ));
        }
        if errno == libc::ENOENT {
            return Err(ControlResponse::err(
                "skill_not_found",
                format!("skill directory '{skill_name}' does not exist"),
            ));
        }
        return Err(ControlResponse::err(
            "write_failed",
            format!("failed to open skill directory '{skill_name}': {e}"),
        ));
    }

    Ok((source_guard, FdGuard(skill_fd)))
}

// ─────────────────────────────────────────────────────────────────────────────
// meta.setActivationXattr
// ─────────────────────────────────────────────────────────────────────────────

fn dispatch_meta_set_activation_xattr(
    raw: &serde_json::Value,
    ctx: Option<&ControlSocketContext>,
) -> ControlResponse {
    let (skill_name, activation_json, skill_dir) = match extract_and_validate_meta_request(raw, ctx)
    {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let source_root = &ctx.as_ref().unwrap().source_root;
    if let Err(resp) = set_activation_xattr_fd(source_root, skill_name, &activation_json) {
        return resp;
    }

    let result = reload_and_emit(ctx, skill_name, &skill_dir, "control_plane_write_xattr");
    ControlResponse::ok(result)
}

/// Fd-anchored xattr write: open source_root → openat(skill_name,
/// O_NOFOLLOW|O_DIRECTORY) → fsetxattr on the verified fd.
fn set_activation_xattr_fd(
    source_root: &Path,
    skill_name: &str,
    json_str: &str,
) -> Result<(), ControlResponse> {
    use std::ffi::CString;

    let (_source_guard, skill_guard) = open_skill_dir_nofollow(source_root, skill_name)?;
    let skill_fd = skill_guard.0;

    let c_name = CString::new(ACTIVATION_XATTR)
        .map_err(|_| ControlResponse::err("write_failed", "xattr name contains NUL"))?;

    let rc = unsafe {
        libc::fsetxattr(
            skill_fd,
            c_name.as_ptr(),
            json_str.as_ptr() as *const libc::c_void,
            json_str.len(),
            0,
        )
    };

    if rc != 0 {
        let err = std::io::Error::last_os_error();
        let errno = err.raw_os_error().unwrap_or(0);
        if errno == libc::ENOTSUP || errno == libc::EOPNOTSUPP {
            return Err(ControlResponse::err(
                "xattr_not_supported",
                format!("filesystem does not support user xattrs on '{skill_name}'"),
            ));
        }
        return Err(ControlResponse::err(
            "write_failed",
            format!("fsetxattr failed: {err}"),
        ));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Socket path preflight
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SocketPreflightError {
    ParentDoesNotExist(PathBuf),
    ExistingPathNotSocket(PathBuf),
    UnlinkFailed(PathBuf, std::io::Error),
}

impl std::fmt::Display for SocketPreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentDoesNotExist(p) => {
                write!(f, "socket parent directory does not exist: {}", p.display())
            }
            Self::ExistingPathNotSocket(p) => write!(
                f,
                "path '{}' exists but is not a socket; refusing to overwrite",
                p.display()
            ),
            Self::UnlinkFailed(p, e) => {
                write!(f, "failed to unlink existing socket '{}': {e}", p.display())
            }
        }
    }
}

impl std::error::Error for SocketPreflightError {}

pub fn preflight_socket_path(path: &Path) -> Result<(), SocketPreflightError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(SocketPreflightError::ParentDoesNotExist(
                parent.to_path_buf(),
            ));
        }
    }

    if path.exists() {
        let meta = std::fs::symlink_metadata(path)
            .map_err(|e| SocketPreflightError::UnlinkFailed(path.to_path_buf(), e))?;
        let file_type = meta.file_type();
        if !file_type.is_socket() {
            return Err(SocketPreflightError::ExistingPathNotSocket(
                path.to_path_buf(),
            ));
        }
        std::fs::remove_file(path)
            .map_err(|e| SocketPreflightError::UnlinkFailed(path.to_path_buf(), e))?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Server
// ─────────────────────────────────────────────────────────────────────────────

pub struct ControlSocketServer {
    config: ControlSocketConfig,
    context: Option<Arc<ControlSocketContext>>,
    shutdown: Arc<AtomicBool>,
}

/// Handle returned to the caller for shutdown coordination.
pub struct ControlSocketHandle {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ControlSocketHandle {
    pub fn shutdown(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Connect to the socket to unblock the accept() call.
        let _ = UnixStream::connect(&self.socket_path);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        // Clean up socket file.
        let _ = std::fs::remove_file(&self.socket_path);
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ControlSocketHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl ControlSocketServer {
    pub fn new(config: ControlSocketConfig) -> Self {
        Self {
            config,
            context: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_context(mut self, context: ControlSocketContext) -> Self {
        self.context = Some(Arc::new(context));
        self
    }

    /// Start the server on a dedicated thread. Returns a handle for
    /// shutdown coordination.
    pub fn start(self) -> Result<ControlSocketHandle, Box<dyn std::error::Error>> {
        preflight_socket_path(&self.config.socket_path)?;

        let listener = UnixListener::bind(&self.config.socket_path)?;

        // Set socket file permissions to 0o600.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.config.socket_path, perms)?;
        }

        let shutdown = self.shutdown.clone();
        let config = self.config.clone();
        let context = self.context.clone();
        let socket_path = self.config.socket_path.clone();

        info!(
            socket = %socket_path.display(),
            trusted_peer_exe = %config.trusted_peer.exe_path.display(),
            trusted_peer_file_id = %config.trusted_peer.exe_file_id,
            "control socket server starting"
        );

        let shutdown_for_thread = shutdown.clone();
        let thread = std::thread::Builder::new()
            .name("skillfs-control-socket".to_string())
            .spawn(move || {
                run_server_loop(&listener, &config, context.as_deref(), &shutdown_for_thread);
            })?;

        Ok(ControlSocketHandle {
            socket_path,
            shutdown,
            thread: Some(thread),
        })
    }
}

fn run_server_loop(
    listener: &UnixListener,
    config: &ControlSocketConfig,
    ctx: Option<&ControlSocketContext>,
    shutdown: &AtomicBool,
) {
    listener
        .set_nonblocking(false)
        .unwrap_or_else(|e| warn!("failed to set listener blocking: {e}"));

    for stream_result in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                warn!("control socket accept error: {e}");
                continue;
            }
        };

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        handle_connection(stream, config, ctx);
    }

    debug!("control socket server loop exited");
}

/// Per-connection read timeout. The server processes exactly one
/// request per connection, so this bounds the total hold time.
const CONNECTION_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Maximum request body size (bytes) accepted from a peer.
const MAX_CONTROL_REQUEST_BYTES: u64 = 64 * 1024;

fn handle_connection(
    stream: UnixStream,
    config: &ControlSocketConfig,
    ctx: Option<&ControlSocketContext>,
) {
    let _ = stream.set_read_timeout(Some(CONNECTION_READ_TIMEOUT));

    let peer_identity = match identify_peer(&stream) {
        Ok(id) => id,
        Err(e) => {
            warn!("failed to identify peer: {e}");
            let resp = ControlResponse::err("peer_identification_failed", e.to_string());
            let _ = send_response(&stream, &resp);
            return;
        }
    };

    debug!(
        pid = peer_identity.credentials.pid,
        uid = peer_identity.credentials.uid,
        gid = peer_identity.credentials.gid,
        exe = ?peer_identity.exe_path,
        "control socket peer connected"
    );

    let verify = verify_peer(&config.trusted_peer, &peer_identity);
    if !verify.is_accepted() {
        let msg = verify
            .denial_message()
            .unwrap_or_else(|| "peer verification failed".to_string());
        warn!(
            pid = peer_identity.credentials.pid,
            reason = %msg,
            "control socket peer rejected"
        );
        let resp = ControlResponse::err("permission_denied", msg);
        let _ = send_response(&stream, &resp);
        return;
    }

    debug!(
        pid = peer_identity.credentials.pid,
        "control socket peer accepted"
    );

    let reader = BufReader::new(&stream);
    let mut limited = reader.take(MAX_CONTROL_REQUEST_BYTES + 1);
    let mut line = String::new();
    match limited.read_line(&mut line) {
        Ok(0) => return,
        Ok(n) if n as u64 > MAX_CONTROL_REQUEST_BYTES => {
            warn!(
                pid = peer_identity.credentials.pid,
                "control socket request exceeds {MAX_CONTROL_REQUEST_BYTES} byte limit"
            );
            let resp = ControlResponse::err(
                "invalid_request",
                format!("request exceeds {MAX_CONTROL_REQUEST_BYTES} byte limit"),
            );
            let _ = send_response(&stream, &resp);
            return;
        }
        Ok(_) => {}
        Err(e) => {
            debug!("control socket read error: {e}");
            return;
        }
    }

    if line.trim().is_empty() {
        return;
    }

    let resp = match parse_request_with_raw(&line) {
        Ok((req, raw)) => dispatch_request(&req, &raw, ctx),
        Err(err_resp) => err_resp,
    };

    let _ = send_response(&stream, &resp);
}

fn send_response(stream: &UnixStream, resp: &ControlResponse) -> std::io::Result<()> {
    let mut writer = stream;
    let json = serde_json::to_string(resp)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writer.write_all(json.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Protocol parse / serialize ───────────────────────────────────────

    #[test]
    fn parse_ping_request() {
        let line = r#"{"schemaVersion":"1","method":"ping"}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "ping");
        assert_eq!(req.schema_version, "1");
    }

    #[test]
    fn parse_status_request() {
        let line = r#"{"schemaVersion":"1","method":"status"}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "status");
    }

    #[test]
    fn parse_request_missing_schema_version_is_error() {
        let line = r#"{"method":"ping"}"#;
        let result = parse_request(line);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.ok);
        assert_eq!(err.error.as_ref().unwrap().code, "invalid_request");
    }

    #[test]
    fn parse_request_wrong_schema_version_is_error() {
        let line = r#"{"schemaVersion":"99","method":"ping"}"#;
        let result = parse_request(line);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.error.as_ref().unwrap().code,
            "unsupported_schema_version"
        );
    }

    #[test]
    fn parse_request_invalid_json_is_error() {
        let line = "not json at all";
        let result = parse_request(line);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error.as_ref().unwrap().code, "invalid_request");
    }

    #[test]
    fn dispatch_ping_returns_pong() {
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "ping".to_string(),
        };
        let raw = serde_json::json!({"schemaVersion": "1", "method": "ping"});
        let resp = dispatch_request(&req, &raw, None);
        assert!(resp.ok);
        let result = resp.result.unwrap();
        assert_eq!(result["pong"], true);
    }

    #[test]
    fn dispatch_status_returns_ready() {
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "status".to_string(),
        };
        let raw = serde_json::json!({"schemaVersion": "1", "method": "status"});
        let resp = dispatch_request(&req, &raw, None);
        assert!(resp.ok);
        let result = resp.result.unwrap();
        assert_eq!(result["status"], "ready");
    }

    #[test]
    fn dispatch_unknown_method_returns_error() {
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "write_meta".to_string(),
        };
        let raw = serde_json::json!({"schemaVersion": "1", "method": "write_meta"});
        let resp = dispatch_request(&req, &raw, None);
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "unknown_method");
        assert!(resp.error.as_ref().unwrap().message.contains("write_meta"));
    }

    // ── Meta write dispatch (unit) ─────────────────────────────────────

    fn test_ctx(source_root: &Path) -> ControlSocketContext {
        ControlSocketContext {
            source_root: source_root.to_path_buf(),
            resolver: Some(Arc::new(ActiveSkillResolver::new(source_root))),
            protocol_event_writer: None,
        }
    }

    #[test]
    fn meta_write_activation_missing_skill_name() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_request");
    }

    #[test]
    fn meta_write_activation_missing_activation() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha"
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_request");
    }

    #[test]
    fn meta_write_activation_invalid_skill_name_dot() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "..",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_write_activation_invalid_skill_name_slash() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "a/b",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_write_activation_invalid_skill_name_nul() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "a\0b",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_write_activation_invalid_skill_name_empty() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_write_activation_skill_not_found() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "nonexistent",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "skill_not_found");
    }

    #[test]
    fn meta_write_activation_symlink_skill_dir_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real-skill");
        std::fs::create_dir(&real_dir).unwrap();
        std::os::unix::fs::symlink(&real_dir, dir.path().join("link-skill")).unwrap();

        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "link-skill",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
        assert!(
            resp.error.as_ref().unwrap().message.contains("symlink"),
            "error should mention symlink: {}",
            resp.error.as_ref().unwrap().message
        );
    }

    #[test]
    fn meta_set_xattr_symlink_skill_dir_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real-skill");
        std::fs::create_dir(&real_dir).unwrap();
        std::os::unix::fs::symlink(&real_dir, dir.path().join("link-skill")).unwrap();

        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "skillName": "link-skill",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_write_activation_no_resolver_returns_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = ControlSocketContext {
            source_root: dir.path().to_path_buf(),
            resolver: None,
            protocol_event_writer: None,
        };
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "not_configured");
        // Verify no file was written to disk.
        assert!(
            !dir.path()
                .join("alpha/.skill-meta/activation.json")
                .exists(),
            "no-resolver request must not write to disk"
        );
    }

    #[test]
    fn meta_set_xattr_no_resolver_returns_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = ControlSocketContext {
            source_root: dir.path().to_path_buf(),
            resolver: None,
            protocol_event_writer: None,
        };
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "not_configured");
    }

    #[test]
    fn meta_write_activation_invalid_activation_bad_schema() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 99, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_activation");
    }

    #[test]
    fn meta_write_activation_invalid_activation_bad_target() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": "/etc/passwd"}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_activation");
    }

    #[test]
    fn meta_write_activation_no_context_returns_not_configured() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let resp = dispatch_request(&req, &raw, None);
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "not_configured");
    }

    #[test]
    fn meta_write_activation_success_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("alpha");
        std::fs::create_dir(&skill_dir).unwrap();

        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(resp.ok, "expected ok, got: {resp:?}");

        let written =
            std::fs::read_to_string(skill_dir.join(".skill-meta/activation.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["schemaVersion"], 1);
        assert!(parsed["target"].is_null());
    }

    #[test]
    fn meta_write_activation_success_updates_resolver() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("alpha");
        std::fs::create_dir(&skill_dir).unwrap();

        let resolver = Arc::new(ActiveSkillResolver::new(dir.path()));
        let ctx = ControlSocketContext {
            source_root: dir.path().to_path_buf(),
            resolver: Some(resolver.clone()),
            protocol_event_writer: None,
        };

        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(resp.ok);

        assert!(
            matches!(resolver.get("alpha"), Some(ActiveTarget::Hidden { .. })),
            "resolver should have hidden target after null activation write"
        );
    }

    #[test]
    fn meta_write_activation_snapshot_target_updates_resolver() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("alpha");
        std::fs::create_dir_all(skill_dir.join(".skill-meta/versions/v000001.snapshot")).unwrap();

        let resolver = Arc::new(ActiveSkillResolver::new(dir.path()));
        let ctx = ControlSocketContext {
            source_root: dir.path().to_path_buf(),
            resolver: Some(resolver.clone()),
            protocol_event_writer: None,
        };

        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.writeActivation",
            "skillName": "alpha",
            "activation": {"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.writeActivation".to_string(),
        };
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(resp.ok, "expected ok, got: {resp:?}");

        assert!(
            matches!(resolver.get("alpha"), Some(ActiveTarget::Snapshot { .. })),
            "resolver should have snapshot target"
        );
    }

    #[test]
    fn meta_set_xattr_missing_skill_name() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_request");
    }

    #[test]
    fn meta_set_xattr_invalid_skill_name() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "skillName": "../escape",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");
    }

    #[test]
    fn meta_set_xattr_skill_not_found() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "skillName": "missing",
            "activation": {"schemaVersion": 1, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "skill_not_found");
    }

    #[test]
    fn meta_set_xattr_invalid_activation() {
        let raw = serde_json::json!({
            "schemaVersion": "1",
            "method": "meta.setActivationXattr",
            "skillName": "alpha",
            "activation": {"schemaVersion": 99, "target": null}
        });
        let req = ControlRequest {
            schema_version: "1".to_string(),
            method: "meta.setActivationXattr".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let ctx = test_ctx(dir.path());
        let resp = dispatch_request(&req, &raw, Some(&ctx));
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "invalid_activation");
    }

    #[test]
    fn parse_request_with_raw_preserves_extra_fields() {
        let line = r#"{"schemaVersion":"1","method":"meta.writeActivation","skillName":"demo","activation":{"schemaVersion":1,"target":null}}"#;
        let (req, raw) = parse_request_with_raw(line).unwrap();
        assert_eq!(req.method, "meta.writeActivation");
        assert_eq!(raw["skillName"], "demo");
        assert!(raw.get("activation").is_some());
    }

    #[test]
    fn response_ok_serializes() {
        let resp = ControlResponse::ok(serde_json::json!({"pong": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"pong\":true"));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn response_err_serializes() {
        let resp = ControlResponse::err("test_code", "test message");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"test_code\""));
        assert!(json.contains("test message"));
        assert!(!json.contains("\"result\""));
    }

    // ── Socket path preflight ────────────────────────────────────────────

    #[test]
    fn preflight_nonexistent_parent_fails() {
        let path = PathBuf::from("/nonexistent/parent/dir/socket.sock");
        let result = preflight_socket_path(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            SocketPreflightError::ParentDoesNotExist(_) => {}
            other => panic!("expected ParentDoesNotExist, got {other:?}"),
        }
    }

    #[test]
    fn preflight_existing_regular_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-socket");
        std::fs::write(&path, "data").unwrap();
        let result = preflight_socket_path(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            SocketPreflightError::ExistingPathNotSocket(_) => {}
            other => panic!("expected ExistingPathNotSocket, got {other:?}"),
        }
    }

    #[test]
    fn preflight_existing_directory_fails() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let result = preflight_socket_path(&sub);
        assert!(result.is_err());
        match result.unwrap_err() {
            SocketPreflightError::ExistingPathNotSocket(_) => {}
            other => panic!("expected ExistingPathNotSocket, got {other:?}"),
        }
    }

    #[test]
    fn preflight_existing_socket_is_unlinked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");
        // Create a real socket to simulate stale leftover.
        let _listener = UnixListener::bind(&path).unwrap();
        drop(_listener);
        assert!(path.exists());
        let result = preflight_socket_path(&path);
        assert!(result.is_ok());
        assert!(!path.exists(), "stale socket should have been unlinked");
    }

    #[test]
    fn preflight_nonexistent_path_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sock");
        let result = preflight_socket_path(&path);
        assert!(result.is_ok());
    }

    // ── Peer verification ────────────────────────────────────────────────

    fn test_peer_config() -> TrustedPeerConfig {
        TrustedPeerConfig {
            exe_path: PathBuf::from("/usr/local/bin/agent-sec-cli"),
            exe_file_id: FileId { dev: 10, ino: 20 },
            uid: None,
            gid: None,
        }
    }

    fn test_peer_identity() -> PeerIdentity {
        PeerIdentity {
            credentials: PeerCredentials {
                pid: 1234,
                uid: 1000,
                gid: 1000,
            },
            exe_path: Some(PathBuf::from("/usr/local/bin/agent-sec-cli")),
            exe_file_id: Some(FileId { dev: 10, ino: 20 }),
        }
    }

    #[test]
    fn verify_matching_peer_accepted() {
        let config = test_peer_config();
        let identity = test_peer_identity();
        let result = verify_peer(&config, &identity);
        assert!(result.is_accepted());
    }

    #[test]
    fn verify_uid_mismatch_denied() {
        let config = TrustedPeerConfig {
            uid: Some(0),
            ..test_peer_config()
        };
        let identity = test_peer_identity();
        let result = verify_peer(&config, &identity);
        assert!(!result.is_accepted());
        match result {
            PeerVerifyResult::DeniedUidMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 1000);
            }
            other => panic!("expected DeniedUidMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_gid_mismatch_denied() {
        let config = TrustedPeerConfig {
            gid: Some(0),
            ..test_peer_config()
        };
        let identity = test_peer_identity();
        let result = verify_peer(&config, &identity);
        assert!(!result.is_accepted());
        match result {
            PeerVerifyResult::DeniedGidMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 1000);
            }
            other => panic!("expected DeniedGidMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_uid_match_accepted() {
        let config = TrustedPeerConfig {
            uid: Some(1000),
            ..test_peer_config()
        };
        let identity = test_peer_identity();
        let result = verify_peer(&config, &identity);
        assert!(result.is_accepted());
    }

    #[test]
    fn verify_gid_match_accepted() {
        let config = TrustedPeerConfig {
            gid: Some(1000),
            ..test_peer_config()
        };
        let identity = test_peer_identity();
        let result = verify_peer(&config, &identity);
        assert!(result.is_accepted());
    }

    #[test]
    fn verify_exe_unresolved_denied() {
        let config = test_peer_config();
        let identity = PeerIdentity {
            credentials: PeerCredentials {
                pid: 1234,
                uid: 1000,
                gid: 1000,
            },
            exe_path: None,
            exe_file_id: None,
        };
        let result = verify_peer(&config, &identity);
        assert!(!result.is_accepted());
        assert!(matches!(result, PeerVerifyResult::DeniedExeUnresolved));
    }

    #[test]
    fn verify_exe_path_mismatch_denied() {
        let config = test_peer_config();
        let identity = PeerIdentity {
            credentials: PeerCredentials {
                pid: 1234,
                uid: 1000,
                gid: 1000,
            },
            exe_path: Some(PathBuf::from("/usr/bin/imposter")),
            exe_file_id: Some(FileId { dev: 99, ino: 99 }),
        };
        let result = verify_peer(&config, &identity);
        assert!(!result.is_accepted());
        match result {
            PeerVerifyResult::DeniedExePathMismatch { .. } => {}
            other => panic!("expected DeniedExePathMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_exe_file_id_mismatch_denied() {
        let config = test_peer_config();
        let identity = PeerIdentity {
            credentials: PeerCredentials {
                pid: 1234,
                uid: 1000,
                gid: 1000,
            },
            exe_path: Some(PathBuf::from("/usr/local/bin/agent-sec-cli")),
            exe_file_id: Some(FileId { dev: 10, ino: 999 }),
        };
        let result = verify_peer(&config, &identity);
        assert!(!result.is_accepted());
        match result {
            PeerVerifyResult::DeniedExeFileIdMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, FileId { dev: 10, ino: 20 });
                assert_eq!(actual, FileId { dev: 10, ino: 999 });
            }
            other => panic!("expected DeniedExeFileIdMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_uid_checked_before_exe() {
        let config = TrustedPeerConfig {
            uid: Some(0),
            ..test_peer_config()
        };
        let identity = PeerIdentity {
            credentials: PeerCredentials {
                pid: 1234,
                uid: 1000,
                gid: 1000,
            },
            exe_path: None,
            exe_file_id: None,
        };
        let result = verify_peer(&config, &identity);
        assert!(
            matches!(result, PeerVerifyResult::DeniedUidMismatch { .. }),
            "uid check should fire before exe check"
        );
    }

    #[test]
    fn denial_message_variants() {
        assert!(PeerVerifyResult::Accepted.denial_message().is_none());
        assert!(
            PeerVerifyResult::DeniedUidMismatch {
                expected: 0,
                actual: 1000
            }
            .denial_message()
            .unwrap()
            .contains("uid")
        );
        assert!(
            PeerVerifyResult::DeniedGidMismatch {
                expected: 0,
                actual: 1000
            }
            .denial_message()
            .unwrap()
            .contains("gid")
        );
        assert!(
            PeerVerifyResult::DeniedExeUnresolved
                .denial_message()
                .unwrap()
                .contains("resolved")
        );
        assert!(
            PeerVerifyResult::DeniedExePathMismatch {
                expected: PathBuf::from("/a"),
                actual: PathBuf::from("/b"),
            }
            .denial_message()
            .unwrap()
            .contains("path")
        );
        assert!(
            PeerVerifyResult::DeniedExeFileIdMismatch {
                expected: FileId { dev: 1, ino: 2 },
                actual: FileId { dev: 3, ino: 4 },
            }
            .denial_message()
            .unwrap()
            .contains("file id")
        );
    }

    // ── Server integration (Linux only) ──────────────────────────────────

    #[cfg(target_os = "linux")]
    mod integration {
        use super::*;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::fs::MetadataExt;

        fn self_exe_config() -> TrustedPeerConfig {
            let exe = std::env::current_exe().unwrap();
            let canon = std::fs::canonicalize(&exe).unwrap();
            let meta = std::fs::metadata(&canon).unwrap();
            TrustedPeerConfig {
                exe_path: canon,
                exe_file_id: FileId {
                    dev: meta.dev(),
                    ino: meta.ino(),
                },
                uid: None,
                gid: None,
            }
        }

        fn connect_and_send(socket_path: &Path, request: &str) -> String {
            let mut stream = UnixStream::connect(socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            writeln!(stream, "{request}").unwrap();
            stream.flush().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();
            response
        }

        #[test]
        fn server_ping_returns_pong() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let resp_str =
                connect_and_send(&socket_path, r#"{"schemaVersion":"1","method":"ping"}"#);
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok);
            assert_eq!(resp.result.unwrap()["pong"], true);

            handle.shutdown();
            assert!(
                !socket_path.exists(),
                "socket file should be cleaned up after shutdown"
            );
        }

        #[test]
        fn server_status_returns_ready() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let resp_str =
                connect_and_send(&socket_path, r#"{"schemaVersion":"1","method":"status"}"#);
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok);
            assert_eq!(resp.result.unwrap()["status"], "ready");

            handle.shutdown();
        }

        #[test]
        fn server_unknown_method_returns_error() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let resp_str = connect_and_send(
                &socket_path,
                r#"{"schemaVersion":"1","method":"write_meta"}"#,
            );
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "unknown_method");

            handle.shutdown();
        }

        #[test]
        fn server_invalid_schema_returns_error() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let resp_str =
                connect_and_send(&socket_path, r#"{"schemaVersion":"99","method":"ping"}"#);
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(
                resp.error.as_ref().unwrap().code,
                "unsupported_schema_version"
            );

            handle.shutdown();
        }

        #[test]
        fn server_invalid_json_returns_error() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let resp_str = connect_and_send(&socket_path, "not json");
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "invalid_request");

            handle.shutdown();
        }

        #[test]
        fn server_untrusted_peer_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: TrustedPeerConfig {
                    exe_path: PathBuf::from("/nonexistent/binary"),
                    exe_file_id: FileId { dev: 0, ino: 0 },
                    uid: None,
                    gid: None,
                },
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let mut stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            writeln!(stream, r#"{{"schemaVersion":"1","method":"ping"}}"#).unwrap();
            stream.flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();

            let resp: ControlResponse = serde_json::from_str(&response).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "permission_denied");

            handle.shutdown();
        }

        #[test]
        fn server_untrusted_uid_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            // Use the real exe but require uid=99999, which won't match.
            let mut peer_config = self_exe_config();
            peer_config.uid = Some(99999);
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: peer_config,
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let mut stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            writeln!(stream, r#"{{"schemaVersion":"1","method":"ping"}}"#).unwrap();
            stream.flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();

            let resp: ControlResponse = serde_json::from_str(&response).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "permission_denied");

            handle.shutdown();
        }

        #[test]
        fn server_handles_sequential_connections() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            // Each request uses its own connection (one-request-per-connection).
            let resp_str =
                connect_and_send(&socket_path, r#"{"schemaVersion":"1","method":"ping"}"#);
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok);
            assert_eq!(resp.result.unwrap()["pong"], true);

            let resp_str =
                connect_and_send(&socket_path, r#"{"schemaVersion":"1","method":"status"}"#);
            let resp2: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp2.ok);
            assert_eq!(resp2.result.unwrap()["status"], "ready");

            handle.shutdown();
        }

        #[test]
        fn shutdown_removes_socket_file() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();
            assert!(socket_path.exists(), "socket file must exist while running");
            handle.shutdown();
            assert!(
                !socket_path.exists(),
                "socket file must be removed after shutdown"
            );
        }

        #[test]
        fn drop_removes_socket_file() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();
            assert!(socket_path.exists());
            drop(handle);
            assert!(
                !socket_path.exists(),
                "socket file must be removed after drop"
            );
        }

        #[test]
        fn socket_permissions_are_0600() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config);
            let handle = server.start().unwrap();

            let meta = std::fs::metadata(&socket_path).unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "socket file permissions must be 0600, got {mode:o}"
            );

            handle.shutdown();
        }

        // ── Request size limit ────────────────────────────────────────────

        #[test]
        fn normal_request_accepted() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let handle = ControlSocketServer::new(config).start().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));

            let stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let req = r#"{"schemaVersion":"1","method":"ping"}"#;
            writeln!(&stream, "{req}").unwrap();
            (&stream).flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();
            assert!(
                response.contains("\"ok\":true"),
                "normal request must be accepted: {response}"
            );

            handle.shutdown();
        }

        #[test]
        fn oversized_request_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("test.sock");
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let handle = ControlSocketServer::new(config).start().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));

            let stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            // Write >64KB without a newline — should be rejected.
            let payload = vec![b'A'; (MAX_CONTROL_REQUEST_BYTES as usize) + 100];
            (&stream).write_all(&payload).unwrap();
            (&stream).write_all(b"\n").unwrap();
            (&stream).flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();
            assert!(
                response.contains("request exceeds") || response.contains("invalid_request"),
                "oversized request must be rejected: {response}"
            );

            handle.shutdown();
        }

        // ── Meta write integration (through socket) ─────────────────────

        fn start_server_with_context(
            dir: &Path,
            source_root: &Path,
        ) -> (PathBuf, ControlSocketHandle) {
            let socket_path = dir.join("test.sock");
            let resolver = Arc::new(ActiveSkillResolver::new(source_root));
            let writer =
                Arc::new(super::super::super::protocol_events::InMemoryProtocolEventWriter::new());
            let ctx = ControlSocketContext {
                source_root: source_root.to_path_buf(),
                resolver: Some(resolver),
                protocol_event_writer: Some(writer),
            };
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: self_exe_config(),
            };
            let server = ControlSocketServer::new(config).with_context(ctx);
            let handle = server.start().unwrap();
            (socket_path, handle)
        }

        #[test]
        fn server_meta_write_activation_writes_file() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("demo-weather");
            std::fs::create_dir(&skill_dir).unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "demo-weather",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok, "expected ok, got: {resp:?}");

            let written = std::fs::read_to_string(skill_dir.join(".skill-meta/activation.json"))
                .expect("activation.json should exist");
            let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert_eq!(parsed["schemaVersion"], 1);
            assert!(parsed["target"].is_null());

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_snapshot_target() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("demo-weather");
            std::fs::create_dir_all(skill_dir.join(".skill-meta/versions/v000001.snapshot"))
                .unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "demo-weather",
                "activation": {
                    "schemaVersion": 1,
                    "target": ".skill-meta/versions/v000001.snapshot"
                }
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok, "expected ok, got: {resp:?}");

            let result = resp.result.unwrap();
            let outcome = result["outcome"].as_str().unwrap();
            assert!(
                outcome == "updated" || outcome == "unchanged",
                "expected updated or unchanged, got {outcome}"
            );

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_untrusted_peer_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("demo-weather");
            std::fs::create_dir(&skill_dir).unwrap();

            let socket_path = dir.path().join("test.sock");
            let ctx = ControlSocketContext {
                source_root: source.path().to_path_buf(),
                resolver: None,
                protocol_event_writer: None,
            };
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: TrustedPeerConfig {
                    exe_path: PathBuf::from("/nonexistent/binary"),
                    exe_file_id: FileId { dev: 0, ino: 0 },
                    uid: None,
                    gid: None,
                },
            };
            let server = ControlSocketServer::new(config).with_context(ctx);
            let handle = server.start().unwrap();

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "demo-weather",
                "activation": {"schemaVersion": 1, "target": null}
            });

            let mut stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            writeln!(stream, "{req}").unwrap();
            stream.flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();

            let resp: ControlResponse = serde_json::from_str(&response).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "permission_denied");

            // Verify no file was written.
            assert!(
                !skill_dir.join(".skill-meta/activation.json").exists(),
                "rejected peer must not write activation.json"
            );

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_invalid_skill_name_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "../escape",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "invalid_skill_name");

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_nonexistent_skill_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "nonexistent",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "skill_not_found");

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_malformed_activation_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("alpha");
            std::fs::create_dir(&skill_dir).unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "alpha",
                "activation": {"schemaVersion": 99, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "invalid_activation");

            // Verify no file was written.
            assert!(
                !skill_dir.join(".skill-meta/activation.json").exists(),
                "rejected activation must not write to disk"
            );

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_activation_no_partial_json() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("alpha");
            std::fs::create_dir(&skill_dir).unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "alpha",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok);

            // Read the file and verify it's valid JSON.
            let content =
                std::fs::read_to_string(skill_dir.join(".skill-meta/activation.json")).unwrap();
            let parsed: serde_json::Value =
                serde_json::from_str(&content).expect("activation.json must be valid JSON");
            assert_eq!(parsed["schemaVersion"], 1);

            // No temp files should remain.
            let meta_dir = skill_dir.join(".skill-meta");
            for entry in std::fs::read_dir(&meta_dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                assert!(
                    !name_str.starts_with("activation.tmp."),
                    "temp file should have been cleaned up: {name_str}"
                );
            }

            handle.shutdown();
        }

        #[test]
        fn server_meta_write_without_trusted_writer_exe_works() {
            let dir = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let skill_dir = source.path().join("alpha");
            std::fs::create_dir(&skill_dir).unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.writeActivation",
                "skillName": "alpha",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(
                resp.ok,
                "control socket write should work without --trusted-writer-exe"
            );

            handle.shutdown();
        }

        #[test]
        fn server_meta_set_xattr_writes_xattr() {
            let dir = tempfile::tempdir().unwrap();

            // Find an xattr-capable tempdir for the source.
            let source = match xattr_capable_tempdir_for_meta() {
                Some(d) => d,
                None => {
                    eprintln!("SKIP: no xattr-capable filesystem for meta.setActivationXattr test");
                    return;
                }
            };
            let skill_dir = source.path().join("alpha");
            std::fs::create_dir(&skill_dir).unwrap();

            let (socket_path, handle) = start_server_with_context(dir.path(), source.path());

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.setActivationXattr",
                "skillName": "alpha",
                "activation": {"schemaVersion": 1, "target": null}
            });
            let resp_str = connect_and_send(&socket_path, &req.to_string());
            let resp: ControlResponse = serde_json::from_str(&resp_str).unwrap();
            assert!(resp.ok, "expected ok, got: {resp:?}");

            // Verify the xattr was set.
            let xattr_outcome = super::super::super::activation::read_activation_xattr(&skill_dir);
            match xattr_outcome {
                super::super::super::activation::XattrReadOutcome::Present(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
                    assert_eq!(parsed["schemaVersion"], 1);
                    assert!(parsed["target"].is_null());
                }
                other => panic!("expected xattr Present, got {other:?}"),
            }

            handle.shutdown();
        }

        #[test]
        fn server_meta_set_xattr_untrusted_peer_no_disk_change() {
            let dir = tempfile::tempdir().unwrap();
            let source = match xattr_capable_tempdir_for_meta() {
                Some(d) => d,
                None => {
                    eprintln!("SKIP: no xattr-capable filesystem for untrusted xattr test");
                    return;
                }
            };
            let skill_dir = source.path().join("alpha");
            std::fs::create_dir(&skill_dir).unwrap();

            let socket_path = dir.path().join("test.sock");
            let ctx = ControlSocketContext {
                source_root: source.path().to_path_buf(),
                resolver: None,
                protocol_event_writer: None,
            };
            let config = ControlSocketConfig {
                socket_path: socket_path.clone(),
                trusted_peer: TrustedPeerConfig {
                    exe_path: PathBuf::from("/nonexistent/binary"),
                    exe_file_id: FileId { dev: 0, ino: 0 },
                    uid: None,
                    gid: None,
                },
            };
            let server = ControlSocketServer::new(config).with_context(ctx);
            let handle = server.start().unwrap();

            let req = serde_json::json!({
                "schemaVersion": "1",
                "method": "meta.setActivationXattr",
                "skillName": "alpha",
                "activation": {"schemaVersion": 1, "target": null}
            });

            let mut stream = UnixStream::connect(&socket_path).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            writeln!(stream, "{req}").unwrap();
            stream.flush().unwrap();

            let mut reader = BufReader::new(&stream);
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();

            let resp: ControlResponse = serde_json::from_str(&response).unwrap();
            assert!(!resp.ok);
            assert_eq!(resp.error.as_ref().unwrap().code, "permission_denied");

            // Verify xattr was not set.
            let xattr_outcome = super::super::super::activation::read_activation_xattr(&skill_dir);
            assert!(
                !matches!(
                    xattr_outcome,
                    super::super::super::activation::XattrReadOutcome::Present(_)
                ),
                "rejected peer must not write xattr"
            );

            handle.shutdown();
        }

        fn xattr_capable_tempdir_for_meta() -> Option<tempfile::TempDir> {
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Ok(env_path) = std::env::var("SKILLFS_XATTR_TEST_ROOT") {
                if !env_path.is_empty() {
                    candidates.push(PathBuf::from(env_path));
                }
            }
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            for ancestor in manifest_dir.ancestors() {
                if ancestor.join("Cargo.lock").exists() {
                    candidates.push(ancestor.join("target").join("xattr-tests"));
                    break;
                }
            }
            if let Some(home) = std::env::var_os("HOME") {
                let mut path = PathBuf::from(home);
                path.push(".cache");
                path.push("skillfs-xattr-tests");
                candidates.push(path);
            }

            for cand in candidates {
                if std::fs::create_dir_all(&cand).is_err() {
                    continue;
                }
                let td = match tempfile::Builder::new()
                    .prefix("c1-meta-")
                    .tempdir_in(&cand)
                {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if user_xattr_supported_meta(td.path()) {
                    return Some(td);
                }
            }
            None
        }

        fn user_xattr_supported_meta(dir: &Path) -> bool {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let c_path = match CString::new(dir.as_os_str().as_bytes()) {
                Ok(c) => c,
                Err(_) => return false,
            };
            let c_name = match CString::new("user.skillfs.probe") {
                Ok(c) => c,
                Err(_) => return false,
            };
            let rc = unsafe {
                libc::lsetxattr(
                    c_path.as_ptr(),
                    c_name.as_ptr(),
                    b"1".as_ptr() as *const libc::c_void,
                    1,
                    0,
                )
            };
            if rc != 0 {
                return false;
            }
            unsafe {
                libc::lremovexattr(c_path.as_ptr(), c_name.as_ptr());
            }
            true
        }
    }
}
