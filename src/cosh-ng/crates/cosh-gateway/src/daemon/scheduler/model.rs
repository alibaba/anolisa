pub(super) const RUNTIME_START_SCHEMA_VERSION: u16 = 2;
const DEFAULT_LEASE_DURATION_MS: u64 = 180_000;
const DEFAULT_LEASE_RENEWAL_MARGIN_MS: u64 = 60_000;
const DEFAULT_RUNTIME_OPERATION_TIMEOUT_MS: u64 = 70_000;
const DEFAULT_RUNTIME_INPUT_TIMEOUT_MS: u64 = 15 * 60 * 1_000;

pub(super) fn runtime_start_delivery_kind() -> BoundedName {
    BoundedName::new("runtime_start").unwrap_or_else(|_| unreachable!())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeStartIntent {
    pub(super) schema_version: u16,
    pub(super) actor: ActorRef,
    pub(super) task_id: TaskId,
    pub(super) run_id: RunId,
    pub(super) runtime: RuntimeSelector,
    pub(super) intent: BoundedText,
    pub(super) target: TargetRef,
    pub(super) workspace: WorkspaceRef,
}

/// Immutable work description passed to an injected Runtime factory.
#[derive(Debug, Clone)]
pub struct ScheduledRun {
    /// Authenticated Task owner.
    pub actor: ActorRef,
    /// Task selected by the durable queue.
    pub task_id: TaskId,
    /// Fenced Run selected by the durable queue.
    pub run_id: RunId,
    /// Runtime and optional installed profile selected at ingress.
    pub runtime: RuntimeSelector,
    /// Original bounded Task intent retained in the private Outbox.
    pub intent: BoundedText,
    /// Governed execution target.
    pub target: TargetRef,
    /// Trusted public projection of the canonical workspace.
    pub workspace: WorkspaceRef,
    /// Current Run-lease generation.
    pub lease_generation: u64,
}

/// Non-blocking result from one injected Runtime handle poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePoll {
    /// No public event is ready yet.
    Pending,
    /// One validated Runtime event advanced only the durable binding sequence.
    Observed {
        /// Monotonic Runtime event sequence.
        sequence: u64,
    },
    /// One bounded progress update is ready for durable recording.
    Update {
        /// Monotonic Runtime event sequence.
        sequence: u64,
        /// Safe Task progress projection.
        update: RuntimeUpdate,
    },
    /// Provider-native execution is paused on a durable approval decision.
    PermissionRequested {
        /// Exact callback identity fenced to the active Runtime generation.
        permission: RuntimePermissionRef,
        /// Trusted normalized capability request.
        request: Box<CapabilityRequest>,
        /// Provider-facing operation description sanitized for actor review.
        summary: ToolSummary,
    },
    /// A COSH-owned typed operation is paused before durable takeover.
    BrokeredExecutionRequested {
        /// Exact callback identity fenced to the active Runtime generation.
        brokered: BrokeredExecutionRef,
        /// Trusted normalized capability request.
        request: Box<CapabilityRequest>,
        /// Closed operation whose side effect remains outside the Runtime.
        operation: BrokeredOperation,
        /// Provider-facing operation description sanitized for actor review.
        summary: ToolSummary,
    },
    /// A side-effect-free Runtime question is paused on durable actor input.
    InputRequested {
        /// Monotonic Runtime event sequence committed with the request.
        sequence: u64,
        /// Exact bounded request presentation and correlation identity.
        request: RuntimeInputRequest,
    },
    /// The Runtime completed successfully.
    Succeeded,
    /// The Runtime completed with a safe bounded failure.
    Failed(ContractError),
    /// The Runtime acknowledged an earlier cancellation request.
    Cancelled,
}

/// Active provider-neutral Runtime owned by the scheduler.
pub trait RuntimeHandle: Send {
    /// Starts the prompt only after the Runtime binding is durable.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when the prompt cannot be admitted.
    fn begin(&mut self) -> Result<(), ContractError>;

    /// Polls at most one event without blocking.
    fn poll(&mut self) -> RuntimePoll;

    /// Requests cancellation and returns only after the Runtime acknowledges it.
    ///
    /// # Errors
    ///
    /// Returns a safe Runtime failure when cancellation cannot be acknowledged.
    fn shutdown(&mut self, reason: CancelReason) -> Result<(), ContractError>;

    /// Dispatches one previously persisted provider-native response.
    fn resolve_provider_permission(
        &mut self,
        permission: &RuntimePermissionRef,
        decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError>;

    /// Dispatches durable ownership of one exact brokered callback.
    fn acknowledge_brokered_request(
        &mut self,
        _brokered: &BrokeredExecutionRef,
        _acknowledgement: BrokeredRequestAcknowledgement,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("brokered acknowledgement"))
    }

    /// Dispatches the terminal result of one exact brokered callback.
    fn deliver_brokered_result(
        &mut self,
        _brokered: &BrokeredExecutionRef,
        _delivery: BrokeredExecutionDelivery,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("brokered result delivery"))
    }

    /// Dispatches one exact durable input response to the waiting Runtime.
    fn resolve_input(
        &mut self,
        _request: &RuntimeInputRequest,
        _response: RuntimeInputResponse,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("input resolution"))
    }
}

fn runtime_handle_unsupported(operation: &'static str) -> ContractError {
    ContractError::new(
        "runtime_operation_unsupported",
        ErrorCategory::InvalidRequest,
        false,
        format!("The Runtime does not support {operation}"),
    )
    .unwrap_or_else(|_| unreachable!("static Runtime operation failure must remain bounded"))
}

/// Open Runtime session returned before its prompt is dispatched.
pub struct StartedRuntime {
    /// Fenced binding emitted by the opened Runtime session.
    pub binding: RuntimeBindingRef,
    /// Runtime handle whose prompt has not started yet.
    pub handle: Box<dyn RuntimeHandle>,
}

/// Injection boundary that opens one provider-neutral Runtime.
pub trait RuntimeFactory: Send {
    /// Opens a Runtime for an already-fenced durable Run without prompting it.
    ///
    /// # Errors
    ///
    /// Returns a safe Runtime failure when no handle can be started.
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError>;
}

impl<T: RuntimeFactory + ?Sized> RuntimeFactory for Box<T> {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        (**self).open(run)
    }
}

/// Lease and blocking-operation bounds for one scheduler worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskSchedulerConfig {
    /// Lifetime of each acquired or renewed Run lease.
    pub lease_duration: Duration,
    /// Lease is renewed once its remaining lifetime reaches this margin.
    pub lease_renewal_margin: Duration,
    /// Maximum configured duration of one blocking Runtime operation.
    pub runtime_operation_timeout: Duration,
}

impl Default for TaskSchedulerConfig {
    fn default() -> Self {
        Self {
            lease_duration: Duration::from_millis(DEFAULT_LEASE_DURATION_MS),
            lease_renewal_margin: Duration::from_millis(DEFAULT_LEASE_RENEWAL_MARGIN_MS),
            runtime_operation_timeout: Duration::from_millis(DEFAULT_RUNTIME_OPERATION_TIMEOUT_MS),
        }
    }
}

impl TaskSchedulerConfig {
    fn validate(self) -> Result<ValidatedSchedulerConfig, GatewayDaemonError> {
        let lease_duration_ms = duration_ms(self.lease_duration, "scheduler lease duration")?;
        let lease_renewal_margin_ms =
            duration_ms(self.lease_renewal_margin, "scheduler lease renewal margin")?;
        let runtime_operation_timeout_ms = duration_ms(
            self.runtime_operation_timeout,
            "scheduler Runtime operation timeout",
        )?;
        if lease_renewal_margin_ms == 0
            || runtime_operation_timeout_ms == 0
            || lease_duration_ms
                <= lease_renewal_margin_ms.saturating_add(runtime_operation_timeout_ms)
        {
            return Err(GatewayDaemonError::Protocol(
                "scheduler lease duration must exceed the Runtime operation timeout plus renewal margin"
                    .to_owned(),
            ));
        }
        Ok(ValidatedSchedulerConfig {
            lease_duration_ms,
            lease_renewal_margin_ms,
            runtime_operation_timeout_ms,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ValidatedSchedulerConfig {
    lease_duration_ms: u64,
    lease_renewal_margin_ms: u64,
    runtime_operation_timeout_ms: u64,
}

/// Observable result of one bounded scheduler iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerTick {
    /// No queued or active work changed durable state.
    Idle,
    /// A queued Run was fenced and its Runtime started.
    Started(TaskView),
    /// Runtime progress was committed.
    Progressed(TaskView),
    /// A Run and its Task reached a terminal state.
    Settled(TaskView),
}

struct ActiveRun {
    scheduled: ScheduledRun,
    lease: LeaseClaim,
    lease_expires_at_ms: u64,
    next_event_sequence: u64,
    abort_error: Option<ContractError>,
    binding: RuntimeBindingRef,
    terminal: Option<TerminalOutcome>,
    binding_closed: bool,
    task_settled: bool,
    pending_permission: Option<PendingPermission>,
    pending_brokered: Option<PendingBrokered>,
    pending_input: Option<RuntimeInputRequestRecord>,
    handle: Box<dyn RuntimeHandle>,
}

#[derive(Debug, Clone)]
struct PendingPermission {
    permission: RuntimePermissionRef,
    approval: ApprovalRequest,
}

#[derive(Debug, Clone)]
enum TerminalOutcome {
    Succeeded,
    Failed(ContractError),
    Cancelled,
}

enum RuntimeStartClaim {
    Empty,
    Claimed {
        outbox: OutboxClaim,
        intent: Box<RuntimeStartIntent>,
        lease: LeaseClaim,
    },
    Recovered(TaskView),
}

/// Minimal durable scheduler for one active Runtime per worker instance.
pub struct TaskScheduler<F> {
    coordinator: TaskCoordinator,
    worker_id: BoundedOpaque,
    config: ValidatedSchedulerConfig,
    factory: F,
    brokered_driver: Box<dyn BrokeredExecutionDriver>,
    active: Option<ActiveRun>,
    shutting_down: bool,
    #[cfg(test)]
    fail_next_brokered_result_completion: bool,
    #[cfg(test)]
    fail_next_terminal_lease_release: bool,
    #[cfg(test)]
    fail_next_input_dispatch_completion: bool,
    #[cfg(test)]
    fail_next_input_request_install: bool,
    #[cfg(test)]
    fail_next_input_unknown_cleanup: bool,
}
