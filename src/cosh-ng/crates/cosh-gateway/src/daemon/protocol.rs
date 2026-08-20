/// Configuration for one per-user local Gateway daemon.
#[derive(Debug, Clone)]
pub struct GatewayDaemonConfig {
    /// Absolute Unix socket path inside a private directory.
    pub socket_path: PathBuf,
    /// Absolute SQLite state path.
    pub database_path: PathBuf,
    /// Durable identity shared by events in this database.
    pub installation_id: Option<InstallationId>,
    /// Closed capability profile selected by trusted daemon configuration.
    pub capability_profile: GatewayCapabilityProfile,
    /// Canonical workspace projection resolved from trusted daemon config.
    pub workspace: WorkspaceRef,
    /// Exact installed Runtime kind and profile admitted by this daemon instance.
    pub runtime: RuntimeSelector,
}

/// Validated fields used to create and queue one Task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitTask {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Bounded user intent; Task history retains its digest while the private
    /// runtime-start Outbox retains the delivery payload.
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

/// Validated fields used to queue a replacement for one suspended Run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryTask {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Task owning the suspended Run.
    pub task_id: TaskId,
    /// Exact active attempt from which immutable start intent is recovered.
    pub previous_run_id: RunId,
    /// Optional optimistic Task revision.
    pub expected_revision: Option<u64>,
}

/// Validated fields used to append one exact pending Runtime input response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendTaskInput {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Task owning the pending Runtime question.
    pub task_id: TaskId,
    /// Exact durable Runtime input request being resolved.
    pub input_request_id: InputRequestId,
    /// Typed bounded response stored only in the private dispatch ledger.
    pub response: RuntimeInputResponse,
    /// Optional optimistic Task revision.
    pub expected_revision: Option<u64>,
}

/// Validated fields used to resolve a provider-native approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveApproval {
    /// Correlates one transport request and response.
    pub request_id: RequestId,
    /// Caller-stable replay key within the authenticated actor namespace.
    pub idempotency_key: IdempotencyKey,
    /// Durable approval awaiting this decision.
    pub approval_id: ApprovalId,
    /// Human decision dispatched once to the bound provider callback.
    pub decision: ApprovalDecision,
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
    /// Projection after a provider-native approval resolution.
    ApprovalResolved(TaskView),
    /// Projection after a retry was queued or replayed.
    Retried(TaskView),
    /// Projection after an input response was durably appended and dispatched.
    InputAppended(TaskView),
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
    ResolveApproval {
        api_version: String,
        #[serde(flatten)]
        request: ResolveApproval,
    },
    Retry {
        api_version: String,
        #[serde(flatten)]
        request: RetryTask,
    },
    AppendInput {
        api_version: String,
        #[serde(flatten)]
        request: AppendTaskInput,
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
            Self::Retry { request, .. } => &request.request_id,
            Self::ResolveApproval { request, .. } => &request.request_id,
            Self::AppendInput { request, .. } => &request.request_id,
        }
    }

    fn api_version(&self) -> &str {
        match self {
            Self::Ping { api_version, .. }
            | Self::Submit { api_version, .. }
            | Self::Get { api_version, .. }
            | Self::Events { api_version, .. }
            | Self::Cancel { api_version, .. }
            | Self::Retry { api_version, .. }
            | Self::AppendInput { api_version, .. }
            | Self::ResolveApproval { api_version, .. } => api_version,
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
