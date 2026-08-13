//! Authenticated local control plane for durable Gateway Tasks.
//!
//! The Unix transport derives authority from kernel peer credentials and
//! delegates every mutation to the single-writer [`TaskCoordinator`].
//!
//! Owner note: Stage 6 keeps protocol, coordinator, and Unix transport in one
//! review unit while the private API freezes. Split them into sibling modules
//! before adding scheduling, approvals, or another transport.

use std::fs::{self, FileType, Metadata};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cosh_gateway_contracts::common::{
    ContractHeader, ContractSchema, Correlation, Digest, IdempotencyKey, RuntimeSelector, TargetRef,
};
use cosh_gateway_contracts::ids::{ActorId, InstallationId, MessageId, RequestId, RunId, TaskId};
use cosh_gateway_contracts::task::{
    CancelReason, CancellationStage, TaskEvent, TaskEventEnvelope, TaskState,
};
use nix::sys::socket::getsockopt;
use nix::unistd::Uid;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::storage::{CommitOutcome, SqliteTaskStore, StoreError, TaskCommit};
use crate::task::TaskAggregate;

/// Local Gateway API version, independent from ACP wire versions.
pub const GATEWAY_API_VERSION: &str = "cosh.gateway.v1";
/// Maximum bytes in one length-prefixed request or response.
pub const MAX_GATEWAY_FRAME_BYTES: usize = 1024 * 1024;
const CONNECTION_DEADLINE: Duration = Duration::from_secs(5);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Configuration for one per-user local Gateway daemon.
#[derive(Debug, Clone)]
pub struct GatewayDaemonConfig {
    /// Absolute Unix socket path inside a private directory.
    pub socket_path: PathBuf,
    /// Absolute SQLite state path.
    pub database_path: PathBuf,
    /// Durable identity shared by events in this database.
    pub installation_id: Option<InstallationId>,
}

/// Validated fields used to create and queue one Task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitTask {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Bounded user intent; storage retains only its digest.
    pub intent: cosh_gateway_contracts::common::BoundedText,
    /// Governed environment selected for the Task.
    pub target: TargetRef,
    /// Runtime selected for the first queued Run.
    pub runtime: RuntimeSelector,
}

/// Validated fields used to request Task cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelTask {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Task owning the active Run.
    pub task_id: TaskId,
    /// Active Run whose cancellation is requested.
    pub run_id: RunId,
    /// Optional optimistic Task revision.
    pub expected_revision: Option<u64>,
}

/// Safe Task projection returned to an authorized local client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// Durable Task identity.
    pub task_id: TaskId,
    /// Latest event revision.
    pub revision: u64,
    /// Current durable lifecycle state.
    pub state: TaskState,
    /// Current Run when one has been allocated.
    pub active_run_id: Option<RunId>,
    /// Immutable governed target.
    pub target: TargetRef,
}

impl From<&TaskAggregate> for TaskView {
    fn from(task: &TaskAggregate) -> Self {
        Self {
            task_id: task.task_id().clone(),
            revision: task.revision(),
            state: task.state(),
            active_run_id: task.active_run_id().cloned(),
            target: task.target().clone(),
        }
    }
}

/// Bounded page of immutable Task events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEventPage {
    /// Task owning the stream.
    pub task_id: TaskId,
    /// Events ordered by increasing revision.
    pub events: Vec<TaskEventEnvelope>,
    /// Last revision in this page, or the supplied cursor for an empty page.
    pub next_revision: u64,
    /// Whether a later revision exists in the current projection.
    pub has_more: bool,
}

/// Successful local Gateway response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
pub enum GatewayResult {
    /// Daemon accepted an authenticated ping.
    Pong,
    /// Current authorized Task projection.
    Task(TaskView),
    /// Bounded immutable event page.
    Events(TaskEventPage),
    /// Projection after a cancellation commit or replay.
    Cancelled(TaskView),
}

/// Local daemon or client failure.
#[derive(Debug, Error)]
pub enum GatewayDaemonError {
    /// A configured socket or state path is unsafe.
    #[error("unsafe Gateway path {path}: {message}")]
    UnsafePath {
        /// Rejected path.
        path: PathBuf,
        /// Bounded reason.
        message: String,
    },
    /// Another daemon owns the configured socket.
    #[error("a Gateway daemon is already listening at {0}")]
    AlreadyRunning(PathBuf),
    /// Kernel peer credentials do not authorize this local client.
    #[error("local Gateway peer is not authorized")]
    Unauthorized,
    /// The local framing or API contract is invalid.
    #[error("invalid Gateway protocol: {0}")]
    Protocol(String),
    /// A remote daemon returned a stable domain failure.
    #[error("Gateway request failed [{code}]: {message}")]
    Remote {
        /// Stable machine-readable error code.
        code: String,
        /// Bounded diagnostic safe for the local client.
        message: String,
        /// Whether refreshing state and retrying may succeed.
        recoverable: bool,
    },
    /// Local I/O failed.
    #[error("Gateway I/O failed: {0}")]
    Io(#[from] io::Error),
    /// Durable Task storage failed.
    #[error("Gateway storage failed: {0}")]
    Store(#[from] StoreError),
    /// JSON encoding or decoding failed.
    #[error("Gateway serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum GatewayRequest {
    Ping {
        api_version: String,
        request_id: RequestId,
    },
    Submit {
        api_version: String,
        #[serde(flatten)]
        request: SubmitTask,
    },
    Get {
        api_version: String,
        request_id: RequestId,
        task_id: TaskId,
    },
    Events {
        api_version: String,
        request_id: RequestId,
        task_id: TaskId,
        after_revision: Option<u64>,
        limit: u16,
    },
    Cancel {
        api_version: String,
        #[serde(flatten)]
        request: CancelTask,
    },
}

impl GatewayRequest {
    fn request_id(&self) -> &RequestId {
        match self {
            Self::Ping { request_id, .. }
            | Self::Get { request_id, .. }
            | Self::Events { request_id, .. } => request_id,
            Self::Submit { request, .. } => &request.request_id,
            Self::Cancel { request, .. } => &request.request_id,
        }
    }

    fn api_version(&self) -> &str {
        match self {
            Self::Ping { api_version, .. }
            | Self::Submit { api_version, .. }
            | Self::Get { api_version, .. }
            | Self::Events { api_version, .. }
            | Self::Cancel { api_version, .. } => api_version,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct GatewayResponse {
    api_version: String,
    request_id: Option<RequestId>,
    #[serde(flatten)]
    outcome: GatewayResponseOutcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum GatewayResponseOutcome {
    Ok { result: GatewayResult },
    Error { error: GatewayErrorBody },
}

#[derive(Debug, Serialize, Deserialize)]
struct GatewayErrorBody {
    code: String,
    message: String,
    recoverable: bool,
}

/// Single-writer Task lifecycle boundary used by the transport handler.
pub struct TaskCoordinator {
    store: SqliteTaskStore,
    installation_id: InstallationId,
}

impl TaskCoordinator {
    /// Opens durable state for one Gateway installation.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed storage error for unsafe or corrupt state.
    pub fn open(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
    ) -> Result<Self, GatewayDaemonError> {
        let mut store = SqliteTaskStore::open(database_path)?;
        let installation_id = store.bind_installation_id(requested_installation_id.as_ref())?;
        Ok(Self {
            store,
            installation_id,
        })
    }

    fn submit(
        &mut self,
        actor_id: &ActorId,
        request: SubmitTask,
    ) -> Result<TaskView, GatewayDaemonError> {
        let task_id = TaskId::new();
        let run_id = RunId::new();
        let committed_at_ms = now_ms()?;
        let intent_digest = sha256_digest(request.intent.as_str().as_bytes());
        let command_digest =
            digest_json(&("submit", &request.intent, &request.target, &request.runtime))?;
        let submitted = self.event(
            actor_id,
            &task_id,
            None,
            1,
            committed_at_ms,
            TaskEvent::TaskSubmitted {
                intent_digest,
                target: request.target,
            },
        );
        let queued = self.event(
            actor_id,
            &task_id,
            Some(&run_id),
            2,
            committed_at_ms,
            TaskEvent::TaskQueued {
                run_id: run_id.clone(),
                runtime: request.runtime,
            },
        );
        let outcome = self.store.commit_task(&TaskCommit {
            actor_id: actor_id.clone(),
            idempotency_key: request.idempotency_key,
            command_digest,
            expected_revision: Some(0),
            events: vec![submitted, queued],
            outbox: Vec::new(),
            committed_at_ms,
        })?;
        let task_id = receipt_task_id(&outcome);
        let task = self.store.load_task(task_id)?;
        authorize(&task, actor_id)?;
        Ok(TaskView::from(&task))
    }

    fn get(&self, actor_id: &ActorId, task_id: &TaskId) -> Result<TaskView, GatewayDaemonError> {
        let task = self.store.load_task(task_id)?;
        authorize(&task, actor_id)?;
        Ok(TaskView::from(&task))
    }

    fn events(
        &self,
        actor_id: &ActorId,
        task_id: &TaskId,
        after_revision: Option<u64>,
        limit: u16,
    ) -> Result<TaskEventPage, GatewayDaemonError> {
        let (events, task_revision) =
            self.store
                .load_task_events_for_owner(task_id, actor_id, after_revision, limit)?;
        let next_revision = events
            .last()
            .map_or(after_revision.unwrap_or(0), |event| event.revision);
        let page = TaskEventPage {
            task_id: task_id.clone(),
            has_more: next_revision < task_revision,
            events,
            next_revision,
        };
        if serde_json::to_vec(&page)?.len() > MAX_GATEWAY_FRAME_BYTES.saturating_sub(4096) {
            return Err(GatewayDaemonError::Protocol(
                "Task event page exceeds the response byte budget".to_owned(),
            ));
        }
        Ok(page)
    }

    fn cancel(
        &mut self,
        actor_id: &ActorId,
        request: CancelTask,
    ) -> Result<TaskView, GatewayDaemonError> {
        let command_digest = digest_json(&("cancel", &request.task_id, &request.run_id))?;
        if let Some(receipt) =
            self.store
                .load_command_receipt(actor_id, &request.idempotency_key, &command_digest)?
        {
            let task = self.store.load_task(&receipt.task_id)?;
            authorize(&task, actor_id)?;
            return Ok(TaskView::from(&task));
        }
        let current = self.store.load_task(&request.task_id)?;
        authorize(&current, actor_id)?;
        if current.active_run_id() != Some(&request.run_id) {
            return Err(GatewayDaemonError::Protocol(
                "cancel Run does not match the active Task Run".to_owned(),
            ));
        }
        if current.state() != TaskState::Queued {
            return Err(GatewayDaemonError::Protocol(
                "this daemon slice cancels only queued, not yet started Runs".to_owned(),
            ));
        }
        let stage = CancellationStage::BeforeRuntime;
        let committed_at_ms = now_ms()?;
        let first_revision = current.revision().saturating_add(1);
        let requested = self.event(
            actor_id,
            &request.task_id,
            Some(&request.run_id),
            first_revision,
            committed_at_ms,
            TaskEvent::CancellationRequested {
                run_id: request.run_id.clone(),
                cause: CancelReason::UserRequested,
            },
        );
        let run_cancelled = self.event(
            actor_id,
            &request.task_id,
            Some(&request.run_id),
            first_revision.saturating_add(1),
            committed_at_ms,
            TaskEvent::RunCancelled {
                run_id: request.run_id.clone(),
                stage,
            },
        );
        let task_cancelled = self.event(
            actor_id,
            &request.task_id,
            Some(&request.run_id),
            first_revision.saturating_add(2),
            committed_at_ms,
            TaskEvent::TaskCancelled,
        );
        let outcome = self.store.commit_task(&TaskCommit {
            actor_id: actor_id.clone(),
            idempotency_key: request.idempotency_key,
            command_digest,
            expected_revision: request.expected_revision.or(Some(current.revision())),
            events: vec![requested, run_cancelled, task_cancelled],
            outbox: Vec::new(),
            committed_at_ms,
        })?;
        let task = self.store.load_task(receipt_task_id(&outcome))?;
        Ok(TaskView::from(&task))
    }

    fn event(
        &self,
        actor_id: &ActorId,
        task_id: &TaskId,
        run_id: Option<&RunId>,
        revision: u64,
        occurred_at_ms: u64,
        event: TaskEvent,
    ) -> TaskEventEnvelope {
        let mut correlation = Correlation::new(self.installation_id.clone());
        correlation.actor_id = Some(actor_id.clone());
        correlation.task_id = Some(task_id.clone());
        correlation.run_id = run_id.cloned();
        TaskEventEnvelope {
            header: ContractHeader::new(
                ContractSchema::TaskEvent,
                MessageId::new(),
                occurred_at_ms,
                correlation,
            ),
            task_id: task_id.clone(),
            revision,
            event,
        }
    }
}

/// Bound per-user local Gateway server.
pub struct GatewayDaemon {
    listener: UnixListener,
    coordinator: TaskCoordinator,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
    owner_uid: u32,
}

impl GatewayDaemon {
    /// Validates private paths, opens state, and binds the local socket.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed path, storage, socket, or already-running error.
    pub fn bind(config: GatewayDaemonConfig) -> Result<Self, GatewayDaemonError> {
        let owner_uid = Uid::effective().as_raw();
        prepare_socket_path(&config.socket_path, owner_uid)?;
        let coordinator = TaskCoordinator::open(&config.database_path, config.installation_id)?;
        let listener = UnixListener::bind(&config.socket_path)?;
        fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let metadata = fs::symlink_metadata(&config.socket_path)?;
        Ok(Self {
            listener,
            coordinator,
            socket_path: config.socket_path,
            socket_identity: (metadata.dev(), metadata.ino()),
            owner_uid,
        })
    }

    /// Serves one request per connection until the shutdown flag is set.
    ///
    /// # Errors
    ///
    /// Returns listener failures. Per-connection protocol and authorization
    /// errors are returned to that client without stopping admission.
    pub fn serve_until(&mut self, shutdown: &AtomicBool) -> Result<(), GatewayDaemonError> {
        while !shutdown.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = self.handle_connection(stream);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL_INTERVAL);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn handle_connection(&mut self, mut stream: UnixStream) -> Result<(), GatewayDaemonError> {
        stream.set_read_timeout(Some(CONNECTION_DEADLINE))?;
        stream.set_write_timeout(Some(CONNECTION_DEADLINE))?;
        let peer_uid = peer_uid(&stream)?;
        if peer_uid != self.owner_uid {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let actor_id = actor_id_for_uid(&self.coordinator.installation_id, peer_uid)?;
        let request = match read_frame::<GatewayRequest>(&mut stream) {
            Ok(request) => request,
            Err(error) => {
                let response = error_response(None, &error);
                let _ = write_frame(&mut stream, &response);
                return Err(error);
            }
        };
        let request_id = request.request_id().clone();
        let result = self.dispatch(&actor_id, request);
        let response = match result {
            Ok(result) => GatewayResponse {
                api_version: GATEWAY_API_VERSION.to_owned(),
                request_id: Some(request_id),
                outcome: GatewayResponseOutcome::Ok { result },
            },
            Err(error) => error_response(Some(request_id), &error),
        };
        write_frame(&mut stream, &response)
    }

    fn dispatch(
        &mut self,
        actor_id: &ActorId,
        request: GatewayRequest,
    ) -> Result<GatewayResult, GatewayDaemonError> {
        if request.api_version() != GATEWAY_API_VERSION {
            return Err(GatewayDaemonError::Protocol(
                "unsupported Gateway API version".to_owned(),
            ));
        }
        match request {
            GatewayRequest::Ping { .. } => Ok(GatewayResult::Pong),
            GatewayRequest::Submit { request, .. } => self
                .coordinator
                .submit(actor_id, request)
                .map(GatewayResult::Task),
            GatewayRequest::Get { task_id, .. } => self
                .coordinator
                .get(actor_id, &task_id)
                .map(GatewayResult::Task),
            GatewayRequest::Events {
                task_id,
                after_revision,
                limit,
                ..
            } => self
                .coordinator
                .events(actor_id, &task_id, after_revision, limit)
                .map(GatewayResult::Events),
            GatewayRequest::Cancel { request, .. } => self
                .coordinator
                .cancel(actor_id, request)
                .map(GatewayResult::Cancelled),
        }
    }
}

impl Drop for GatewayDaemon {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path) {
            if (metadata.dev(), metadata.ino()) == self.socket_identity
                && metadata.file_type().is_socket()
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }
}

/// Thin local client that carries no identity or execution authority.
#[derive(Debug, Clone)]
pub struct LocalGatewayClient {
    socket_path: PathBuf,
}

impl LocalGatewayClient {
    /// Creates a client for one absolute local socket path.
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Verifies the daemon transport and authentication path.
    pub fn ping(&self, request_id: RequestId) -> Result<GatewayResult, GatewayDaemonError> {
        self.request(GatewayRequest::Ping {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request_id,
        })
    }

    /// Creates and queues one durable Task.
    pub fn submit(&self, request: SubmitTask) -> Result<GatewayResult, GatewayDaemonError> {
        self.request(GatewayRequest::Submit {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request,
        })
    }

    /// Reads one authorized Task projection.
    pub fn get(
        &self,
        request_id: RequestId,
        task_id: TaskId,
    ) -> Result<GatewayResult, GatewayDaemonError> {
        self.request(GatewayRequest::Get {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request_id,
            task_id,
        })
    }

    /// Reads a bounded authorized event page.
    pub fn events(
        &self,
        request_id: RequestId,
        task_id: TaskId,
        after_revision: Option<u64>,
        limit: u16,
    ) -> Result<GatewayResult, GatewayDaemonError> {
        self.request(GatewayRequest::Events {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request_id,
            task_id,
            after_revision,
            limit,
        })
    }

    /// Persists cancellation of one active Task Run.
    pub fn cancel(&self, request: CancelTask) -> Result<GatewayResult, GatewayDaemonError> {
        self.request(GatewayRequest::Cancel {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request,
        })
    }

    fn request(&self, request: GatewayRequest) -> Result<GatewayResult, GatewayDaemonError> {
        if !self.socket_path.is_absolute() {
            return Err(unsafe_path(
                &self.socket_path,
                "socket path must be absolute",
            ));
        }
        let expected_request_id = request.request_id().clone();
        let mut stream = UnixStream::connect(&self.socket_path)?;
        if peer_uid(&stream)? != Uid::effective().as_raw() {
            return Err(GatewayDaemonError::Unauthorized);
        }
        stream.set_read_timeout(Some(CONNECTION_DEADLINE))?;
        stream.set_write_timeout(Some(CONNECTION_DEADLINE))?;
        write_frame(&mut stream, &request)?;
        let response = read_frame::<GatewayResponse>(&mut stream)?;
        if response.api_version != GATEWAY_API_VERSION
            || response.request_id.as_ref() != Some(&expected_request_id)
        {
            return Err(GatewayDaemonError::Protocol(
                "response correlation or API version mismatch".to_owned(),
            ));
        }
        match response.outcome {
            GatewayResponseOutcome::Ok { result } => Ok(result),
            GatewayResponseOutcome::Error { error } => Err(GatewayDaemonError::Remote {
                code: error.code,
                message: error.message,
                recoverable: error.recoverable,
            }),
        }
    }
}

fn authorize(task: &TaskAggregate, actor_id: &ActorId) -> Result<(), GatewayDaemonError> {
    if task.owner_actor_id() == actor_id {
        Ok(())
    } else {
        // Hide foreign Task existence from the local actor namespace.
        Err(StoreError::TaskNotFound.into())
    }
}

fn receipt_task_id(outcome: &CommitOutcome) -> &TaskId {
    match outcome {
        CommitOutcome::Applied(receipt) | CommitOutcome::Replayed(receipt) => &receipt.task_id,
    }
}

fn digest_json(value: &impl Serialize) -> Result<Digest, GatewayDaemonError> {
    Ok(sha256_digest(&serde_json::to_vec(value)?))
}

fn sha256_digest(bytes: &[u8]) -> Digest {
    let digest = Sha256::digest(bytes);
    // SHA-256 lower-hex output always satisfies the contract.
    Digest::parse(format!("{digest:x}")).unwrap_or_else(|_| unreachable!())
}

fn now_ms() -> Result<u64, GatewayDaemonError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GatewayDaemonError::Protocol("system clock precedes Unix epoch".to_owned()))?
        .as_millis()
        .try_into()
        .map_err(|_| GatewayDaemonError::Protocol("system clock is out of range".to_owned()))
}

fn actor_id_for_uid(
    installation_id: &InstallationId,
    uid: u32,
) -> Result<ActorId, GatewayDaemonError> {
    let mut bytes = Sha256::digest(
        [
            b"cosh.gateway.local.actor.v1".as_slice(),
            installation_id.as_str().as_bytes(),
            &uid.to_be_bytes(),
        ]
        .concat(),
    );
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    );
    ActorId::parse(format!("act_{uuid}"))
        .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Result<u32, GatewayDaemonError> {
    use nix::sys::socket::sockopt::PeerCredentials;

    Ok(getsockopt(stream, PeerCredentials)
        .map_err(nix_to_io)?
        .uid())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn peer_uid(stream: &UnixStream) -> Result<u32, GatewayDaemonError> {
    use nix::sys::socket::sockopt::LocalPeerCred;

    Ok(getsockopt(stream, LocalPeerCred).map_err(nix_to_io)?.uid())
}

fn nix_to_io(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

fn prepare_socket_path(path: &Path, owner_uid: u32) -> Result<(), GatewayDaemonError> {
    if !path.is_absolute() {
        return Err(unsafe_path(path, "socket path must be absolute"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| unsafe_path(path, "socket path has no parent"))?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) => {
            validate_socket_ancestor_chain(parent, owner_uid)?;
            validate_private_directory(parent, &metadata, owner_uid)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let grandparent = parent
                .parent()
                .ok_or_else(|| unsafe_path(parent, "socket directory has no parent"))?;
            validate_socket_ancestor_chain(grandparent, owner_uid)?;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700).create(parent)?;
            let metadata = fs::symlink_metadata(parent)?;
            validate_private_directory(parent, &metadata, owner_uid)?;
        }
        Err(error) => return Err(error.into()),
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != owner_uid {
                return Err(unsafe_path(
                    path,
                    "existing path is not an owned Unix socket",
                ));
            }
            let stale_identity = (metadata.dev(), metadata.ino());
            if UnixStream::connect(path).is_ok() {
                return Err(GatewayDaemonError::AlreadyRunning(path.to_path_buf()));
            }
            let current = fs::symlink_metadata(path)?;
            if !current.file_type().is_socket()
                || current.uid() != owner_uid
                || (current.dev(), current.ino()) != stale_identity
            {
                return Err(unsafe_path(
                    path,
                    "socket path changed during stale-socket validation",
                ));
            }
            fs::remove_file(path)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_socket_ancestor_chain(
    directory: &Path,
    owner_uid: u32,
) -> Result<(), GatewayDaemonError> {
    for ancestor in directory.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(unsafe_path(
                ancestor,
                "socket ancestor is not a real directory",
            ));
        }
        let mode = metadata.permissions().mode();
        let root_sticky = metadata.uid() == 0 && mode & 0o1000 != 0;
        if metadata.uid() != owner_uid && metadata.uid() != 0 {
            return Err(unsafe_path(
                ancestor,
                "socket ancestor has an untrusted owner",
            ));
        }
        if mode & 0o022 != 0 && !root_sticky {
            return Err(unsafe_path(
                ancestor,
                "socket ancestor is writable by another principal",
            ));
        }
    }
    Ok(())
}

fn validate_private_directory(
    path: &Path,
    metadata: &Metadata,
    owner_uid: u32,
) -> Result<(), GatewayDaemonError> {
    let file_type: FileType = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_dir() {
        return Err(unsafe_path(path, "socket parent is not a real directory"));
    }
    if metadata.uid() != owner_uid || metadata.permissions().mode() & 0o077 != 0 {
        return Err(unsafe_path(
            path,
            "socket parent must be owned by the effective UID with mode 0700",
        ));
    }
    Ok(())
}

fn unsafe_path(path: &Path, message: &str) -> GatewayDaemonError {
    GatewayDaemonError::UnsafePath {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

fn read_frame<T: for<'de> Deserialize<'de>>(
    reader: &mut impl Read,
) -> Result<T, GatewayDaemonError> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header)?;
    let length = usize::try_from(u32::from_be_bytes(header))
        .map_err(|_| GatewayDaemonError::Protocol("frame length is out of range".to_owned()))?;
    if length == 0 || length > MAX_GATEWAY_FRAME_BYTES {
        return Err(GatewayDaemonError::Protocol(format!(
            "frame length must be between 1 and {MAX_GATEWAY_FRAME_BYTES} bytes"
        )));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(GatewayDaemonError::from)
}

fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), GatewayDaemonError> {
    let payload = serde_json::to_vec(value)?;
    let length = u32::try_from(payload.len()).map_err(|_| {
        GatewayDaemonError::Protocol("serialized frame length is out of range".to_owned())
    })?;
    if payload.is_empty() || payload.len() > MAX_GATEWAY_FRAME_BYTES {
        return Err(GatewayDaemonError::Protocol(format!(
            "serialized frame exceeds {MAX_GATEWAY_FRAME_BYTES} bytes"
        )));
    }
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

fn error_response(request_id: Option<RequestId>, error: &GatewayDaemonError) -> GatewayResponse {
    let (code, message, recoverable) = match error {
        GatewayDaemonError::Unauthorized => {
            ("unauthenticated", "local peer authentication failed", false)
        }
        GatewayDaemonError::Protocol(_) | GatewayDaemonError::Serialization(_) => (
            "invalid_request",
            "request violates the Gateway contract",
            false,
        ),
        GatewayDaemonError::Store(StoreError::TaskNotFound) => {
            ("not_found", "Task was not found", false)
        }
        GatewayDaemonError::Store(StoreError::IdempotencyConflict) => (
            "idempotency_conflict",
            "idempotency key was used for another command",
            false,
        ),
        GatewayDaemonError::Store(StoreError::RevisionConflict { .. }) => (
            "task_version_conflict",
            "Task changed before the command committed",
            true,
        ),
        GatewayDaemonError::Store(error) => (
            "store_unavailable",
            "durable Task storage is unavailable",
            error.recoverable(),
        ),
        GatewayDaemonError::Io(_) => ("internal", "local transport failed", true),
        GatewayDaemonError::UnsafePath { .. }
        | GatewayDaemonError::AlreadyRunning(_)
        | GatewayDaemonError::Remote { .. } => {
            ("internal", "Gateway cannot complete the request", false)
        }
    };
    GatewayResponse {
        api_version: GATEWAY_API_VERSION.to_owned(),
        request_id,
        outcome: GatewayResponseOutcome::Error {
            error: GatewayErrorBody {
                code: code.to_owned(),
                message: message.to_owned(),
                recoverable,
            },
        },
    }
}

#[cfg(test)]
mod tests;
