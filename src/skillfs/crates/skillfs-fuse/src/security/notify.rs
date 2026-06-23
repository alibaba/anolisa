//! N2 + N3: Notify Change Client and Protocol Event Log.
//!
//! Sends `skill_ledger.skillfs_notify_change` notifications to an external
//! daemon over a Unix domain socket. The notification tells the daemon that
//! a skill's source workspace may have changed; the daemon owns scan,
//! reconcile, and activation refresh.
//!
//! N3 optionally injects a [`super::protocol_events::ProtocolEventWriter`]
//! into the controller. When present, each debounced notification also
//! writes an append-only JSONL protocol event before the socket send, so
//! the local log is written even when the daemon is unreachable.
//!
//! Failure semantics: notify failure and protocol event write failure are
//! both diagnostic only. Neither changes the in-memory
//! [`super::ActiveSkillResolver`] mapping. The existing trusted view stays
//! in place until the daemon writes a new `activation.json` / xattr.
//!
//! Wire format follows `SKILL_LEDGER_SKILLFS_ACTIVATION_CN.md` §变更通知接口.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read as IoRead, Write as IoWrite};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::runtime::{Builder, Handle};
use tokio::sync::{Notify as TokioNotify, mpsc};
use tracing::{debug, info, warn};

use super::activation_reload::ActivationReloadController;
use super::activation_watcher::WatcherRegistrar;
use super::lifecycle::is_reserved_lifecycle_name;
use super::path::is_skill_meta_path;
use super::protocol_events::{NoopProtocolEventWriter, ProtocolEvent, ProtocolEventWriter};
use super::refresh::MutationKind;

pub const NOTIFY_METHOD: &str = "skill_ledger.skillfs_notify_change";
pub const NOTIFY_SCHEMA_VERSION: u64 = 1;
pub const DEFAULT_NOTIFY_TIMEOUT_MS: u64 = 5000;
pub const DEFAULT_NOTIFY_DEBOUNCE_MS: u64 = 300;
/// Maximum number of relative paths per notification. Exceeding this sends
/// `paths: []` to signal "whole skill may have changed".
pub const MAX_NOTIFY_PATHS: usize = 64;

/// Maximum response body size (bytes) accepted from the daemon.
/// Responses exceeding this limit are rejected as `InvalidResponse`
/// to prevent unbounded memory allocation on a malicious/buggy peer.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("skillfs-{seq}")
}

// ---------------------------------------------------------------------------
// NotifyEventKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotifyEventKind {
    Mkdir,
    Create,
    Write,
    Rename,
    Unlink,
    Rmdir,
    Truncate,
    Reconcile,
    Unknown,
}

impl NotifyEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mkdir => "mkdir",
            Self::Create => "create",
            Self::Write => "write",
            Self::Rename => "rename",
            Self::Unlink => "unlink",
            Self::Rmdir => "rmdir",
            Self::Truncate => "truncate",
            Self::Reconcile => "reconcile",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_mutation_kind(kind: MutationKind) -> Self {
        match kind {
            MutationKind::Mkdir => Self::Mkdir,
            MutationKind::Create => Self::Create,
            MutationKind::Write => Self::Write,
            MutationKind::Rename => Self::Rename,
            MutationKind::Unlink => Self::Unlink,
            MutationKind::Rmdir => Self::Rmdir,
            MutationKind::SetattrTruncate => Self::Truncate,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct NotifyChangeEvent {
    pub id: String,
    pub method: &'static str,
    pub params: NotifyParams,
    pub trace_context: serde_json::Value,
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyParams {
    pub schema_version: u64,
    pub skill_dir: String,
    pub skill_name: String,
    pub event_kind: String,
    pub paths: Vec<String>,
}

impl NotifyChangeEvent {
    pub fn new(
        skill_dir: impl Into<String>,
        skill_name: impl Into<String>,
        event_kind: NotifyEventKind,
        paths: Vec<String>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            id: next_request_id(),
            method: NOTIFY_METHOD,
            params: NotifyParams {
                schema_version: NOTIFY_SCHEMA_VERSION,
                skill_dir: skill_dir.into(),
                skill_name: skill_name.into(),
                event_kind: event_kind.as_str().to_string(),
                paths,
            },
            trace_context: serde_json::json!({}),
            timeout_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum NotifyError {
    Connect(std::io::Error),
    Write(std::io::Error),
    Read(std::io::Error),
    Timeout,
    InvalidResponse { body: String },
    Rejected { body: String },
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "notify: connect failed: {e}"),
            Self::Write(e) => write!(f, "notify: write failed: {e}"),
            Self::Read(e) => write!(f, "notify: read failed: {e}"),
            Self::Timeout => write!(f, "notify: timeout"),
            Self::InvalidResponse { body } => {
                write!(f, "notify: invalid response: {body}")
            }
            Self::Rejected { body } => {
                write!(f, "notify: rejected: {body}")
            }
        }
    }
}

impl std::error::Error for NotifyError {}

// ---------------------------------------------------------------------------
// Client trait + implementations
// ---------------------------------------------------------------------------

pub trait NotifyClient: Send + Sync {
    fn send(&self, event: &NotifyChangeEvent) -> Result<(), NotifyError>;
}

/// Production client that sends one NDJSON request frame per Unix socket
/// connection. Each call opens a new connection (matching the
/// single-connection-per-request protocol).
pub struct UnixSocketNotifyClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl UnixSocketNotifyClient {
    pub fn new(socket_path: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            socket_path: socket_path.into(),
            timeout,
        }
    }
}

impl NotifyClient for UnixSocketNotifyClient {
    fn send(&self, event: &NotifyChangeEvent) -> Result<(), NotifyError> {
        let stream = UnixStream::connect(&self.socket_path).map_err(NotifyError::Connect)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(NotifyError::Write)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(NotifyError::Read)?;

        let mut writer = std::io::BufWriter::new(&stream);
        serde_json::to_writer(&mut writer, event)
            .map_err(|e| NotifyError::Write(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        writer.write_all(b"\n").map_err(NotifyError::Write)?;
        writer.flush().map_err(NotifyError::Write)?;

        let reader = BufReader::new(&stream);
        let mut limited = reader.take(MAX_RESPONSE_BYTES + 1);
        let mut line = String::new();
        match limited.read_line(&mut line) {
            Ok(0) => {
                return Err(NotifyError::InvalidResponse {
                    body: "empty response".to_string(),
                });
            }
            Ok(n) if n as u64 > MAX_RESPONSE_BYTES => {
                return Err(NotifyError::InvalidResponse {
                    body: format!("response exceeds {MAX_RESPONSE_BYTES} byte limit"),
                });
            }
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                return Err(NotifyError::Timeout);
            }
            Err(e) => return Err(NotifyError::Read(e)),
        }

        validate_response(&line)
    }
}

fn validate_response(body: &str) -> Result<(), NotifyError> {
    let parsed: serde_json::Value =
        serde_json::from_str(body.trim()).map_err(|_| NotifyError::InvalidResponse {
            body: body.trim().to_string(),
        })?;

    let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        return Err(NotifyError::Rejected {
            body: body.trim().to_string(),
        });
    }

    let accepted = parsed
        .get("data")
        .and_then(|d| d.get("accepted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !accepted {
        return Err(NotifyError::Rejected {
            body: body.trim().to_string(),
        });
    }

    Ok(())
}

/// No-op client for tests and for when notify is disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopNotifyClient;

impl NotifyClient for NoopNotifyClient {
    fn send(&self, _event: &NotifyChangeEvent) -> Result<(), NotifyError> {
        Ok(())
    }
}

/// In-memory client that records events for tests.
#[derive(Debug, Default)]
pub struct InMemoryNotifyClient {
    events: Mutex<Vec<CapturedNotify>>,
}

#[derive(Debug, Clone)]
pub struct CapturedNotify {
    pub skill_name: String,
    pub event_kind: String,
    pub paths: Vec<String>,
    pub skill_dir: String,
}

impl InMemoryNotifyClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<CapturedNotify> {
        self.events.lock().clone()
    }

    pub fn len(&self) -> usize {
        self.events.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.lock().is_empty()
    }
}

impl NotifyClient for InMemoryNotifyClient {
    fn send(&self, event: &NotifyChangeEvent) -> Result<(), NotifyError> {
        self.events.lock().push(CapturedNotify {
            skill_name: event.params.skill_name.clone(),
            event_kind: event.params.event_kind.clone(),
            paths: event.params.paths.clone(),
            skill_dir: event.params.skill_dir.clone(),
        });
        Ok(())
    }
}

/// Client that sleeps for a configured duration before returning.
/// Used to verify that FUSE callbacks are not blocked by slow notify.
pub struct SlowNotifyClient {
    delay: Duration,
}

impl SlowNotifyClient {
    pub fn new(delay: Duration) -> Self {
        Self { delay }
    }
}

impl NotifyClient for SlowNotifyClient {
    fn send(&self, _event: &NotifyChangeEvent) -> Result<(), NotifyError> {
        std::thread::sleep(self.delay);
        Ok(())
    }
}

/// Client that always fails, for testing failure resilience.
#[derive(Debug, Default)]
pub struct FailingNotifyClient;

impl NotifyClient for FailingNotifyClient {
    fn send(&self, _event: &NotifyChangeEvent) -> Result<(), NotifyError> {
        Err(NotifyError::Connect(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "test: daemon unavailable",
        )))
    }
}

// ---------------------------------------------------------------------------
// NotifyController (debounce + dispatch)
// ---------------------------------------------------------------------------

pub struct NotifyController {
    inner: Arc<NotifyInner>,
}

struct NotifyInner {
    client: Arc<dyn NotifyClient>,
    protocol_event_writer: Arc<dyn ProtocolEventWriter>,
    reload_controller: Option<Arc<ActivationReloadController>>,
    /// A5: watcher registrar for auto-tracking skills observed through
    /// notify. Set post-construction via `set_watcher_registrar`.
    watcher_registrar: Mutex<Option<Arc<dyn WatcherRegistrar>>>,
    source_root: PathBuf,
    debounce: Duration,
    timeout_ms: u64,
    pending: Mutex<HashMap<String, NotifyPendingState>>,
    notify: TokioNotify,
    sender: mpsc::UnboundedSender<NotifyCommand>,
}

#[derive(Debug, Clone)]
struct NotifyPendingState {
    skill_name: String,
    event_kind: NotifyEventKind,
    paths: HashSet<String>,
    fire_at: Instant,
}

#[derive(Debug)]
enum NotifyCommand {
    Wakeup,
    Shutdown,
}

impl NotifyController {
    pub fn new(
        client: Arc<dyn NotifyClient>,
        source_root: impl Into<PathBuf>,
        debounce: Duration,
        timeout_ms: u64,
    ) -> Arc<Self> {
        Self::new_with_protocol_writer(
            client,
            source_root,
            debounce,
            timeout_ms,
            Arc::new(NoopProtocolEventWriter),
        )
    }

    pub fn new_with_protocol_writer(
        client: Arc<dyn NotifyClient>,
        source_root: impl Into<PathBuf>,
        debounce: Duration,
        timeout_ms: u64,
        protocol_event_writer: Arc<dyn ProtocolEventWriter>,
    ) -> Arc<Self> {
        Self::new_full(
            client,
            source_root,
            debounce,
            timeout_ms,
            protocol_event_writer,
            None,
        )
    }

    pub fn new_with_reload(
        client: Arc<dyn NotifyClient>,
        source_root: impl Into<PathBuf>,
        debounce: Duration,
        timeout_ms: u64,
        protocol_event_writer: Arc<dyn ProtocolEventWriter>,
        reload_controller: Arc<ActivationReloadController>,
    ) -> Arc<Self> {
        Self::new_full(
            client,
            source_root,
            debounce,
            timeout_ms,
            protocol_event_writer,
            Some(reload_controller),
        )
    }

    fn new_full(
        client: Arc<dyn NotifyClient>,
        source_root: impl Into<PathBuf>,
        debounce: Duration,
        timeout_ms: u64,
        protocol_event_writer: Arc<dyn ProtocolEventWriter>,
        reload_controller: Option<Arc<ActivationReloadController>>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(NotifyInner {
            client,
            protocol_event_writer,
            reload_controller,
            watcher_registrar: Mutex::new(None),
            source_root: source_root.into(),
            debounce,
            timeout_ms,
            pending: Mutex::new(HashMap::new()),
            notify: TokioNotify::new(),
            sender: tx,
        });
        let worker_inner = inner.clone();
        match Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move { notify_worker_loop(worker_inner, rx).await });
            }
            Err(_) => {
                let spawn_result = std::thread::Builder::new()
                    .name("skillfs-notify".to_string())
                    .spawn(move || {
                        let rt = match Builder::new_current_thread()
                            .enable_time()
                            .enable_io()
                            .build()
                        {
                            Ok(rt) => rt,
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    "failed to build Tokio runtime for notify worker; \
                                     notifications will be dropped"
                                );
                                return;
                            }
                        };
                        rt.block_on(notify_worker_loop(worker_inner, rx));
                    });
                if spawn_result.is_err() {
                    warn!(
                        "failed to spawn skillfs-notify worker thread; \
                         notifications will be dropped"
                    );
                }
            }
        }
        Arc::new(Self { inner })
    }

    /// Record a FUSE mutation observation. Returns `true` when accepted,
    /// `false` when filtered (skill-discover, `.skill-meta/**`, lifecycle).
    pub fn observe(
        &self,
        skill_name: &str,
        relative_path: Option<&Path>,
        kind: MutationKind,
    ) -> bool {
        if !is_notify_eligible(skill_name) {
            return false;
        }
        if let Some(rel) = relative_path {
            if is_skill_meta_path(rel) {
                return false;
            }
        }

        let event_kind = NotifyEventKind::from_mutation_kind(kind);
        let now = Instant::now();
        let fire_at = now + self.inner.debounce;
        {
            let mut pending = self.inner.pending.lock();
            let entry =
                pending
                    .entry(skill_name.to_string())
                    .or_insert_with(|| NotifyPendingState {
                        skill_name: skill_name.to_string(),
                        event_kind,
                        paths: HashSet::new(),
                        fire_at,
                    });
            entry.fire_at = fire_at;
            entry.event_kind = event_kind;
            if let Some(rel) = relative_path {
                let path_str = rel.to_string_lossy().to_string();
                if !path_str.is_empty() {
                    entry.paths.insert(path_str);
                }
            }
        }
        let _ = self.inner.sender.send(NotifyCommand::Wakeup);
        self.inner.notify.notify_one();
        true
    }

    /// Convenience wrapper matching `RefreshController::observe_mutation`.
    pub fn observe_mutation(
        &self,
        skill_name: &str,
        relative_path: Option<&Path>,
        kind: MutationKind,
    ) -> bool {
        self.observe(skill_name, relative_path, kind)
    }

    /// Drain and send all pending notifications synchronously. Test helper.
    pub fn flush_for_testing(&self) -> usize {
        let drained = self
            .inner
            .drain_due(Instant::now() + self.inner.debounce * 2);
        let mut count = 0;
        for state in drained {
            self.inner.send_one(state);
            count += 1;
        }
        count
    }

    /// Emit startup reconcile events for all given skills. Bypasses
    /// debounce and sends immediately. Each eligible skill gets one
    /// `eventKind="reconcile"` protocol event and one notify. Filtered
    /// skills (skill-discover, lifecycle roots) are skipped.
    ///
    /// **Blocking**: each `client.send()` may wait up to `timeout_ms`
    /// per skill. Production callers should use
    /// [`Self::spawn_startup_reconcile`] to avoid blocking the startup
    /// path.
    ///
    /// Returns the number of reconcile events emitted.
    pub fn emit_startup_reconcile(&self, skill_names: &[String]) -> usize {
        let mut count = 0;
        for name in skill_names {
            if !is_notify_eligible(name) {
                continue;
            }
            let skill_dir = self.inner.source_root.join(name);
            let skill_dir_str = skill_dir.to_string_lossy().to_string();

            let protocol_event =
                ProtocolEvent::new(&skill_dir_str, name.as_str(), "reconcile", Vec::new());
            self.inner.protocol_event_writer.emit(&protocol_event);

            let event = NotifyChangeEvent::new(
                &skill_dir_str,
                name.as_str(),
                NotifyEventKind::Reconcile,
                Vec::new(),
                self.inner.timeout_ms,
            );

            if let Err(e) = self.inner.client.send(&event) {
                warn!(
                    skill = %name,
                    error = %e,
                    "reconcile: failed to send reconcile notification"
                );
            } else {
                debug!(
                    skill = %name,
                    "reconcile: startup reconcile notification sent"
                );
            }

            count += 1;
        }
        info!(count, "reconcile: startup reconcile complete");
        count
    }

    /// Non-blocking variant of [`Self::emit_startup_reconcile`].
    ///
    /// Spawns a background thread that sends reconcile events for each
    /// eligible skill. The caller is not blocked by daemon socket
    /// latency. The thread is detached — its lifetime is bounded by the
    /// `Arc<NotifyController>` it holds.
    pub fn spawn_startup_reconcile(self: &Arc<Self>, skill_names: Vec<String>) {
        let ctrl = self.clone();
        let spawn_result = std::thread::Builder::new()
            .name("skillfs-reconcile".to_string())
            .spawn(move || {
                ctrl.emit_startup_reconcile(&skill_names);
            });
        if let Err(e) = spawn_result {
            warn!(
                error = %e,
                "reconcile: failed to spawn startup reconcile thread"
            );
        }
    }

    /// A5: inject an activation watcher registrar so that skills observed
    /// through notify are automatically tracked for late-activation
    /// convergence. Called post-construction because the watcher is
    /// built after the notify controller.
    pub fn set_watcher_registrar(&self, registrar: Arc<dyn WatcherRegistrar>) {
        *self.inner.watcher_registrar.lock() = Some(registrar);
    }

    /// Enqueue a notification for immediate dispatch by the background
    /// worker. Bypasses the debounce window (fire_at = now) but does NOT
    /// block the calling thread on socket send or activation reload poll.
    /// The worker picks it up on its next iteration.
    pub fn enqueue_immediate(&self, skill_name: &str, kind: MutationKind, paths: Vec<String>) {
        let event_kind = NotifyEventKind::from_mutation_kind(kind);
        let state = NotifyPendingState {
            skill_name: skill_name.to_string(),
            event_kind,
            paths: paths.into_iter().collect(),
            fire_at: Instant::now(),
        };
        self.inner
            .pending
            .lock()
            .insert(skill_name.to_string(), state);
        let _ = self.inner.sender.send(NotifyCommand::Wakeup);
        self.inner.notify.notify_one();
    }

    pub fn shutdown(&self) {
        let _ = self.inner.sender.send(NotifyCommand::Shutdown);
        self.inner.notify.notify_waiters();
    }

    pub fn debounce(&self) -> Duration {
        self.inner.debounce
    }
}

impl Drop for NotifyController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl NotifyInner {
    fn drain_due(&self, deadline: Instant) -> Vec<NotifyPendingState> {
        let mut due = Vec::new();
        let mut guard = self.pending.lock();
        let keys: Vec<String> = guard.keys().cloned().collect();
        for key in keys {
            if let Some(state) = guard.get(&key) {
                if state.fire_at <= deadline {
                    if let Some(removed) = guard.remove(&key) {
                        due.push(removed);
                    }
                }
            }
        }
        due
    }

    fn next_fire_at(&self) -> Option<Instant> {
        self.pending.lock().values().map(|s| s.fire_at).min()
    }

    fn send_one(&self, state: NotifyPendingState) {
        let skill_dir = self.source_root.join(&state.skill_name);
        let skill_dir_str = skill_dir.to_string_lossy().to_string();

        // A3: snapshot activation freshness BEFORE sending the notify so
        // the poll baseline predates the daemon's activation write.
        // Covers both activation.json mtime and skill dir ctime (xattr).
        let pre_notify_freshness = self
            .reload_controller
            .as_ref()
            .map(|r| r.snapshot_freshness(&state.skill_name));

        let paths: Vec<String> = if state.paths.len() > MAX_NOTIFY_PATHS {
            Vec::new()
        } else {
            let mut sorted: Vec<String> = state.paths.into_iter().collect();
            sorted.sort();
            sorted
        };

        // Write protocol event log regardless of notify outcome.
        let protocol_event = ProtocolEvent::new(
            &skill_dir_str,
            &state.skill_name,
            state.event_kind.as_str(),
            paths.clone(),
        );
        self.protocol_event_writer.emit(&protocol_event);

        let event = NotifyChangeEvent::new(
            skill_dir_str.clone(),
            state.skill_name.clone(),
            state.event_kind,
            paths,
            self.timeout_ms,
        );

        if let Err(e) = self.client.send(&event) {
            warn!(
                skill = %state.skill_name,
                error = %e,
                "notify: failed to send change notification; \
                 current activation mapping unchanged"
            );
            // A5: daemon unreachable — register for watcher convergence
            // so a later daemon repair can still be observed.
            self.register_with_watcher(&state.skill_name);
        } else {
            debug!(
                skill = %state.skill_name,
                event_kind = state.event_kind.as_str(),
                "notify: change notification accepted"
            );
        }

        // A3: poll-after-notify activation reload.
        if let Some(ref reload) = self.reload_controller {
            let baseline = pre_notify_freshness
                .expect("reload_controller presence implies freshness was captured");
            debug!(
                skill = %state.skill_name,
                "notify: starting activation reload poll"
            );
            let outcome = reload.poll_reload_skill(&state.skill_name, baseline);
            debug!(
                skill = %state.skill_name,
                outcome = ?outcome,
                "notify: activation reload poll completed"
            );

            // A5: on poll timeout, register the skill with the watcher
            // so late activation writes are still caught by the
            // background convergence loop.
            if matches!(outcome, super::activation_reload::ReloadOutcome::Timeout) {
                self.register_with_watcher(&state.skill_name);
            }

            // A4: emit reload outcome as a protocol event.
            let reload_event = ProtocolEvent::with_reload_outcome(
                &skill_dir_str,
                &state.skill_name,
                outcome.as_protocol_label(),
            );
            self.protocol_event_writer.emit(&reload_event);
        }
    }

    /// A5: register a skill with the activation watcher (if set).
    fn register_with_watcher(&self, skill_name: &str) {
        if let Some(ref registrar) = *self.watcher_registrar.lock() {
            registrar.register(skill_name);
            debug!(
                skill = %skill_name,
                "notify: registered skill with activation watcher for convergence"
            );
        }
    }
}

fn is_notify_eligible(skill: &str) -> bool {
    if skill.is_empty() {
        return false;
    }
    if skill == "skill-discover" {
        return false;
    }
    if is_reserved_lifecycle_name(skill) {
        return false;
    }
    true
}

async fn notify_worker_loop(
    inner: Arc<NotifyInner>,
    mut rx: mpsc::UnboundedReceiver<NotifyCommand>,
) {
    debug!("notify worker starting");
    loop {
        let sleep_for = match inner.next_fire_at() {
            Some(t) => t.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(60),
        };
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(NotifyCommand::Wakeup) => {}
                    Some(NotifyCommand::Shutdown) | None => {
                        debug!("notify worker shutting down");
                        return;
                    }
                }
            }
            _ = tokio::time::sleep(sleep_for) => {}
        }

        let due = inner.drain_due(Instant::now());
        if due.is_empty() {
            continue;
        }
        for state in due {
            let inner_clone = inner.clone();
            let join = tokio::task::spawn_blocking(move || inner_clone.send_one(state)).await;
            if let Err(e) = join {
                warn!(error = %e, "notify: blocking task join failed");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_change_event_json_shape() {
        let event = NotifyChangeEvent {
            id: "skillfs-42".to_string(),
            method: NOTIFY_METHOD,
            params: NotifyParams {
                schema_version: NOTIFY_SCHEMA_VERSION,
                skill_dir: "/srv/skills/tianqi-weather".to_string(),
                skill_name: "tianqi-weather".to_string(),
                event_kind: "write".to_string(),
                paths: vec!["SKILL.md".to_string()],
            },
            trace_context: serde_json::json!({}),
            timeout_ms: 5000,
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["id"], "skillfs-42");
        assert_eq!(json["method"], "skill_ledger.skillfs_notify_change");
        assert_eq!(json["params"]["schemaVersion"], 1);
        assert_eq!(json["params"]["skillDir"], "/srv/skills/tianqi-weather");
        assert_eq!(json["params"]["skillName"], "tianqi-weather");
        assert_eq!(json["params"]["eventKind"], "write");
        assert_eq!(json["params"]["paths"], serde_json::json!(["SKILL.md"]));
        assert_eq!(json["trace_context"], serde_json::json!({}));
        assert_eq!(json["timeout_ms"], 5000);
    }

    #[test]
    fn notify_event_kind_from_mutation_kind_mapping() {
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Mkdir),
            NotifyEventKind::Mkdir
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Create),
            NotifyEventKind::Create
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Write),
            NotifyEventKind::Write
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Rename),
            NotifyEventKind::Rename
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Unlink),
            NotifyEventKind::Unlink
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::Rmdir),
            NotifyEventKind::Rmdir
        );
        assert_eq!(
            NotifyEventKind::from_mutation_kind(MutationKind::SetattrTruncate),
            NotifyEventKind::Truncate
        );
    }

    #[test]
    fn notify_event_kind_labels() {
        assert_eq!(NotifyEventKind::Mkdir.as_str(), "mkdir");
        assert_eq!(NotifyEventKind::Create.as_str(), "create");
        assert_eq!(NotifyEventKind::Write.as_str(), "write");
        assert_eq!(NotifyEventKind::Rename.as_str(), "rename");
        assert_eq!(NotifyEventKind::Unlink.as_str(), "unlink");
        assert_eq!(NotifyEventKind::Rmdir.as_str(), "rmdir");
        assert_eq!(NotifyEventKind::Truncate.as_str(), "truncate");
        assert_eq!(NotifyEventKind::Unknown.as_str(), "unknown");
    }

    #[test]
    fn noop_client_succeeds() {
        let client = NoopNotifyClient;
        let event = NotifyChangeEvent::new(
            "/srv/skills/alpha",
            "alpha",
            NotifyEventKind::Write,
            vec!["SKILL.md".to_string()],
            5000,
        );
        assert!(client.send(&event).is_ok());
    }

    #[test]
    fn in_memory_client_records() {
        let client = InMemoryNotifyClient::new();
        assert!(client.is_empty());
        let event = NotifyChangeEvent::new(
            "/srv/skills/alpha",
            "alpha",
            NotifyEventKind::Write,
            vec!["SKILL.md".to_string()],
            5000,
        );
        client.send(&event).unwrap();
        assert_eq!(client.len(), 1);
        let events = client.events();
        assert_eq!(events[0].skill_name, "alpha");
        assert_eq!(events[0].event_kind, "write");
        assert_eq!(events[0].paths, vec!["SKILL.md"]);
    }

    #[test]
    fn validate_response_accepts_ok_accepted() {
        let body = r#"{"ok":true,"data":{"schemaVersion":1,"accepted":true}}"#;
        assert!(validate_response(body).is_ok());
    }

    #[test]
    fn validate_response_rejects_ok_false() {
        let body = r#"{"ok":false,"error":{"code":"not_found"}}"#;
        assert!(matches!(
            validate_response(body),
            Err(NotifyError::Rejected { .. })
        ));
    }

    #[test]
    fn validate_response_rejects_accepted_false() {
        let body = r#"{"ok":true,"data":{"accepted":false}}"#;
        assert!(matches!(
            validate_response(body),
            Err(NotifyError::Rejected { .. })
        ));
    }

    #[test]
    fn validate_response_rejects_invalid_json() {
        let body = "not json at all";
        assert!(matches!(
            validate_response(body),
            Err(NotifyError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn validate_response_rejects_missing_data() {
        let body = r#"{"ok":true}"#;
        assert!(matches!(
            validate_response(body),
            Err(NotifyError::Rejected { .. })
        ));
    }

    #[test]
    fn controller_filters_skill_discover() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        let accepted = ctrl.observe(
            "skill-discover",
            Some(Path::new("SKILL.md")),
            MutationKind::Write,
        );
        assert!(!accepted);
        ctrl.shutdown();
    }

    #[test]
    fn controller_filters_skill_meta_paths() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        let accepted = ctrl.observe(
            "alpha",
            Some(Path::new(".skill-meta/manifest.json")),
            MutationKind::Write,
        );
        assert!(!accepted);
        ctrl.shutdown();
    }

    #[test]
    fn controller_filters_lifecycle_reserved() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        for name in &[".staging", ".certified", ".quarantine", ".archive"] {
            let accepted = ctrl.observe(name, None, MutationKind::Mkdir);
            assert!(!accepted, "{name} must be filtered");
        }
        ctrl.shutdown();
    }

    #[test]
    fn controller_debounce_collapses() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        for _ in 0..5 {
            ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        }
        let processed = ctrl.flush_for_testing();
        assert_eq!(processed, 1, "five observations must collapse to one");
        let events = client.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].skill_name, "alpha");
        ctrl.shutdown();
    }

    #[test]
    fn controller_collects_and_deduplicates_paths() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.observe(
            "alpha",
            Some(Path::new("scripts/run.sh")),
            MutationKind::Create,
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        let processed = ctrl.flush_for_testing();
        assert_eq!(processed, 1);
        let events = client.events();
        assert_eq!(events.len(), 1);
        let mut paths = events[0].paths.clone();
        paths.sort();
        assert_eq!(paths, vec!["SKILL.md", "scripts/run.sh"]);
        ctrl.shutdown();
    }

    #[test]
    fn controller_caps_paths_at_limit() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        for i in 0..MAX_NOTIFY_PATHS + 10 {
            ctrl.observe(
                "alpha",
                Some(Path::new(&format!("file_{i}.txt"))),
                MutationKind::Write,
            );
        }
        ctrl.flush_for_testing();
        let events = client.events();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].paths.is_empty(),
            "exceeding MAX_NOTIFY_PATHS must send empty paths"
        );
        ctrl.shutdown();
    }

    #[test]
    fn controller_notify_failure_is_silent() {
        let client = Arc::new(FailingNotifyClient);
        let ctrl = NotifyController::new(client, "/srv/skills", Duration::from_millis(50), 5000);
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        // Must not panic or propagate error
        let processed = ctrl.flush_for_testing();
        assert_eq!(processed, 1);
        ctrl.shutdown();
    }

    #[test]
    fn controller_source_root_appears_in_skill_dir() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/home/user/skills",
            Duration::from_millis(50),
            5000,
        );
        ctrl.observe("weather", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.flush_for_testing();
        let events = client.events();
        assert_eq!(events[0].skill_dir, "/home/user/skills/weather");
        ctrl.shutdown();
    }

    #[test]
    fn eligible_rejects_empty_name() {
        assert!(!is_notify_eligible(""));
    }

    #[test]
    fn eligible_accepts_normal_skill() {
        assert!(is_notify_eligible("alpha"));
        assert!(is_notify_eligible("my-weather-skill"));
    }

    #[test]
    fn request_ids_are_sequential() {
        let id1 = next_request_id();
        let id2 = next_request_id();
        assert!(id1.starts_with("skillfs-"));
        assert!(id2.starts_with("skillfs-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn no_ambient_runtime_controller_tears_down_on_drop() {
        let start = std::time::Instant::now();
        for _ in 0..8 {
            let client = Arc::new(InMemoryNotifyClient::new());
            let ctrl = NotifyController::new(
                client.clone(),
                "/srv/skills",
                Duration::from_millis(20),
                5000,
            );
            ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
            assert_eq!(ctrl.flush_for_testing(), 1);
            let events = client.events();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].skill_name, "alpha");
            // Drop the Arc — Drop sends Shutdown, the worker returns,
            // the private runtime thread exits cleanly.
            drop(ctrl);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "8 controller create/drop cycles took {elapsed:?}; \
             a leaked worker thread would have blocked here"
        );
    }

    // -------------------------------------------------------------------
    // N3 Protocol Event integration tests
    // -------------------------------------------------------------------

    use super::super::protocol_events::InMemoryProtocolEventWriter;

    #[test]
    fn protocol_event_written_on_flush() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.flush_for_testing();
        assert_eq!(writer.len(), 1);
        let events = writer.events();
        assert_eq!(events[0].schema_version, 1);
        assert_eq!(events[0].skill_name, "alpha");
        assert_eq!(events[0].event_kind, "write");
        assert_eq!(events[0].paths, vec!["SKILL.md"]);
        assert_eq!(events[0].skill_dir, "/srv/skills/alpha");
        assert!(events[0].time.ends_with('Z'), "time must be RFC3339 UTC");
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_written_even_when_notify_fails() {
        let client = Arc::new(FailingNotifyClient);
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.flush_for_testing();
        assert_eq!(
            writer.len(),
            1,
            "protocol event must be written even when notify client fails"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_debounce_collapses() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        for _ in 0..5 {
            ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        }
        ctrl.flush_for_testing();
        assert_eq!(
            writer.len(),
            1,
            "five observations must collapse to one protocol event"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_collects_and_deduplicates_paths() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.observe(
            "alpha",
            Some(Path::new("scripts/run.sh")),
            MutationKind::Create,
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.flush_for_testing();
        assert_eq!(writer.len(), 1);
        let events = writer.events();
        let mut paths = events[0].paths.clone();
        paths.sort();
        assert_eq!(paths, vec!["SKILL.md", "scripts/run.sh"]);
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_caps_paths_at_limit() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        for i in 0..MAX_NOTIFY_PATHS + 10 {
            ctrl.observe(
                "alpha",
                Some(Path::new(&format!("file_{i}.txt"))),
                MutationKind::Write,
            );
        }
        ctrl.flush_for_testing();
        assert_eq!(writer.len(), 1);
        assert!(
            writer.events()[0].paths.is_empty(),
            "exceeding MAX_NOTIFY_PATHS must send empty paths in protocol event"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_empty_paths_for_mkdir() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        ctrl.observe("alpha", None, MutationKind::Mkdir);
        ctrl.flush_for_testing();
        assert_eq!(writer.len(), 1);
        assert!(writer.events()[0].paths.is_empty());
        assert_eq!(writer.events()[0].event_kind, "mkdir");
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_not_written_for_skill_meta() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        let accepted = ctrl.observe(
            "alpha",
            Some(Path::new(".skill-meta/manifest.json")),
            MutationKind::Write,
        );
        assert!(!accepted);
        ctrl.flush_for_testing();
        assert!(
            writer.is_empty(),
            ".skill-meta/** must not produce protocol events"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_not_written_for_skill_discover() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        let accepted = ctrl.observe(
            "skill-discover",
            Some(Path::new("SKILL.md")),
            MutationKind::Write,
        );
        assert!(!accepted);
        ctrl.flush_for_testing();
        assert!(
            writer.is_empty(),
            "skill-discover must not produce protocol events"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_not_written_for_lifecycle_reserved() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        for name in &[".staging", ".certified", ".quarantine", ".archive"] {
            let accepted = ctrl.observe(name, None, MutationKind::Mkdir);
            assert!(!accepted, "{name} must be filtered");
        }
        ctrl.flush_for_testing();
        assert!(
            writer.is_empty(),
            "lifecycle reserved roots must not produce protocol events"
        );
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_all_mutation_kinds() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );
        let kinds = [
            (MutationKind::Write, "write"),
            (MutationKind::Create, "create"),
            (MutationKind::Rename, "rename"),
            (MutationKind::Unlink, "unlink"),
            (MutationKind::Rmdir, "rmdir"),
            (MutationKind::SetattrTruncate, "truncate"),
        ];
        for (i, (kind, _label)) in kinds.iter().enumerate() {
            let skill = format!("skill-{i}");
            ctrl.observe(&skill, Some(Path::new("file.txt")), *kind);
        }
        ctrl.flush_for_testing();
        assert_eq!(writer.len(), kinds.len());
        let events = writer.events();
        // HashMap drain order is not deterministic, so check by skill name.
        let mut by_skill: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for e in &events {
            by_skill.insert(e.skill_name.clone(), e.event_kind.clone());
        }
        for (i, (_, label)) in kinds.iter().enumerate() {
            let skill = format!("skill-{i}");
            assert_eq!(
                by_skill.get(&skill).map(|s| s.as_str()),
                Some(*label),
                "{skill} event_kind"
            );
        }
        ctrl.shutdown();
    }

    #[test]
    fn protocol_event_jsonl_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("protocol-events.jsonl");
        let jsonl_writer = Arc::new(
            super::super::protocol_events::JsonlProtocolEventWriter::new(&path, 0)
                .expect("open writer"),
        );
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            jsonl_writer.clone(),
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.observe("beta", Some(Path::new("lib.rs")), MutationKind::Create);
        ctrl.flush_for_testing();
        // Give the writer thread time to flush; use generous margin for
        // contended CI environments.
        std::thread::sleep(Duration::from_millis(500));
        ctrl.shutdown();

        let body = std::fs::read_to_string(&path).expect("read protocol events file");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines, got {body:?}");
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
            assert_eq!(parsed["schemaVersion"], 1);
            assert!(parsed["time"].as_str().unwrap().ends_with('Z'));
            assert!(parsed.get("skillDir").is_some());
            assert!(parsed.get("skillName").is_some());
            assert!(parsed.get("eventKind").is_some());
            assert!(parsed.get("paths").is_some());
        }
        // HashMap drain order is non-deterministic; check by skill name.
        let mut skill_names: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["skillName"].as_str().unwrap().to_string()
            })
            .collect();
        skill_names.sort();
        assert_eq!(skill_names, vec!["alpha", "beta"]);
    }

    #[test]
    fn noop_protocol_writer_does_not_block_controller() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );
        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);
        ctrl.flush_for_testing();
        // Verify notify still works with default noop writer.
        let events = client.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].skill_name, "alpha");
        ctrl.shutdown();
    }

    // -------------------------------------------------------------------
    // A4 Reconcile tests
    // -------------------------------------------------------------------

    #[test]
    fn reconcile_event_kind_label() {
        assert_eq!(NotifyEventKind::Reconcile.as_str(), "reconcile");
    }

    #[test]
    fn emit_startup_reconcile_sends_notify_and_protocol_event() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );

        let names = vec!["alpha".to_string(), "beta".to_string()];
        let count = ctrl.emit_startup_reconcile(&names);

        assert_eq!(count, 2);

        // Verify notify events.
        let notify_events = client.events();
        assert_eq!(notify_events.len(), 2);
        let mut notify_names: Vec<String> =
            notify_events.iter().map(|e| e.skill_name.clone()).collect();
        notify_names.sort();
        assert_eq!(notify_names, vec!["alpha", "beta"]);
        for e in &notify_events {
            assert_eq!(e.event_kind, "reconcile");
            assert!(e.paths.is_empty());
        }

        // Verify protocol events.
        let proto_events = writer.events();
        assert_eq!(proto_events.len(), 2);
        let mut proto_names: Vec<String> =
            proto_events.iter().map(|e| e.skill_name.clone()).collect();
        proto_names.sort();
        assert_eq!(proto_names, vec!["alpha", "beta"]);
        for e in &proto_events {
            assert_eq!(e.event_kind, "reconcile");
            assert!(e.paths.is_empty());
            assert!(e.reload_outcome.is_none());
        }

        ctrl.shutdown();
    }

    #[test]
    fn emit_startup_reconcile_filters_skill_discover() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );

        let names = vec![
            "alpha".to_string(),
            "skill-discover".to_string(),
            "beta".to_string(),
        ];
        let count = ctrl.emit_startup_reconcile(&names);

        assert_eq!(count, 2, "skill-discover must be filtered");
        let notify_events = client.events();
        assert_eq!(notify_events.len(), 2);
        assert!(
            notify_events
                .iter()
                .all(|e| e.skill_name != "skill-discover"),
            "skill-discover must not appear in notify events"
        );

        ctrl.shutdown();
    }

    #[test]
    fn emit_startup_reconcile_filters_lifecycle_roots() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );

        let names = vec![
            "alpha".to_string(),
            ".staging".to_string(),
            ".certified".to_string(),
            ".quarantine".to_string(),
            ".archive".to_string(),
        ];
        let count = ctrl.emit_startup_reconcile(&names);

        assert_eq!(count, 1, "lifecycle roots must be filtered");
        let notify_events = client.events();
        assert_eq!(notify_events.len(), 1);
        assert_eq!(notify_events[0].skill_name, "alpha");

        ctrl.shutdown();
    }

    #[test]
    fn emit_startup_reconcile_notify_failure_is_silent() {
        let client = Arc::new(FailingNotifyClient);
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client,
            "/srv/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );

        let names = vec!["alpha".to_string()];
        let count = ctrl.emit_startup_reconcile(&names);

        assert_eq!(count, 1);
        // Protocol event must still be written even when notify fails.
        assert_eq!(writer.len(), 1);
        assert_eq!(writer.events()[0].event_kind, "reconcile");

        ctrl.shutdown();
    }

    #[test]
    fn emit_startup_reconcile_empty_list() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let ctrl = NotifyController::new(
            client.clone(),
            "/srv/skills",
            Duration::from_millis(50),
            5000,
        );

        let count = ctrl.emit_startup_reconcile(&[]);
        assert_eq!(count, 0);
        assert!(client.is_empty());

        ctrl.shutdown();
    }

    #[test]
    fn emit_startup_reconcile_skill_dir_path() {
        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_protocol_writer(
            client.clone(),
            "/home/user/skills",
            Duration::from_millis(50),
            5000,
            writer.clone(),
        );

        ctrl.emit_startup_reconcile(&["weather".to_string()]);

        let notify_events = client.events();
        assert_eq!(notify_events[0].skill_dir, "/home/user/skills/weather");

        let proto_events = writer.events();
        assert_eq!(proto_events[0].skill_dir, "/home/user/skills/weather");

        ctrl.shutdown();
    }

    // -------------------------------------------------------------------
    // A4 Reload outcome protocol event tests
    // -------------------------------------------------------------------

    // -------------------------------------------------------------------
    // Response size limit tests
    // -------------------------------------------------------------------

    #[test]
    fn unix_socket_client_accepts_normal_response() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let client = UnixSocketNotifyClient::new(&sock_path, Duration::from_secs(5));
        let event = NotifyChangeEvent::new(
            "/srv/skills/alpha",
            "alpha",
            NotifyEventKind::Write,
            vec!["SKILL.md".to_string()],
            5000,
        );

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut _req = String::new();
            reader.read_line(&mut _req).unwrap();
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(&stream);
            writer
                .write_all(br#"{"ok":true,"data":{"schemaVersion":1,"accepted":true}}"#)
                .unwrap();
            writer.write_all(b"\n").unwrap();
            writer.flush().unwrap();
        });

        let result = client.send(&event);
        handle.join().unwrap();
        assert!(
            result.is_ok(),
            "normal response must be accepted: {result:?}"
        );
    }

    #[test]
    fn unix_socket_client_rejects_oversized_response() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let client = UnixSocketNotifyClient::new(&sock_path, Duration::from_secs(5));
        let event = NotifyChangeEvent::new(
            "/srv/skills/alpha",
            "alpha",
            NotifyEventKind::Write,
            vec![],
            5000,
        );

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut _req = String::new();
            reader.read_line(&mut _req).unwrap();
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(&stream);
            // Write >64KB without a newline — should be rejected.
            let payload = vec![b'A'; (MAX_RESPONSE_BYTES as usize) + 100];
            writer.write_all(&payload).unwrap();
            writer.write_all(b"\n").unwrap();
            writer.flush().unwrap();
        });

        let result = client.send(&event);
        handle.join().unwrap();
        assert!(
            matches!(result, Err(NotifyError::InvalidResponse { .. })),
            "oversized response must be rejected: {result:?}"
        );
    }

    #[test]
    fn reload_outcome_emitted_as_protocol_event_after_flush() {
        use super::super::activation_reload::ActivationReloadController;

        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("alpha");
        let meta = skill_dir.join(".skill-meta");
        std::fs::create_dir_all(&meta).unwrap();
        // No activation.json → reload will timeout since nothing is fresh.
        // But reload_skill_once will fail-safe hidden.
        // Actually, for send_one path we need the poll. Let's create a valid
        // activation that is already fresh to get an outcome quickly.
        let snap = skill_dir.join(".skill-meta/versions/v000001.snapshot");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(
            meta.join("activation.json"),
            r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
        )
        .unwrap();

        let resolver = Arc::new(super::super::active::ActiveSkillResolver::new(dir.path()));
        let reload_ctrl = Arc::new(ActivationReloadController::new(
            dir.path(),
            resolver.clone(),
            Duration::from_millis(30),
            Duration::from_millis(500),
        ));

        let client = Arc::new(InMemoryNotifyClient::new());
        let writer = Arc::new(InMemoryProtocolEventWriter::new());
        let ctrl = NotifyController::new_with_reload(
            client.clone(),
            dir.path().to_path_buf(),
            Duration::from_millis(50),
            5000,
            writer.clone(),
            reload_ctrl,
        );

        ctrl.observe("alpha", Some(Path::new("SKILL.md")), MutationKind::Write);

        // Wait briefly to allow mtime to advance, then re-write activation.
        std::thread::sleep(Duration::from_millis(15));
        std::fs::write(
            meta.join("activation.json"),
            r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
        )
        .unwrap();

        ctrl.flush_for_testing();

        // Should have at least 2 protocol events: the mutation event and
        // the reload outcome event.
        let events = writer.events();
        assert!(
            events.len() >= 2,
            "expected at least 2 protocol events (mutation + reload), got {}",
            events.len()
        );

        let reload_events: Vec<_> = events.iter().filter(|e| e.event_kind == "reload").collect();
        assert!(
            !reload_events.is_empty(),
            "expected at least one reload protocol event"
        );
        let reload_event = &reload_events[0];
        assert!(
            reload_event.reload_outcome.is_some(),
            "reload event must have reload_outcome"
        );
        let outcome = reload_event.reload_outcome.as_ref().unwrap();
        assert!(
            outcome == "activation_updated"
                || outcome == "activation_unchanged"
                || outcome == "activation_timeout",
            "unexpected reload outcome: {outcome}"
        );

        ctrl.shutdown();
    }
}
