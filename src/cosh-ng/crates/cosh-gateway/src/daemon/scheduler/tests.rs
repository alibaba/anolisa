use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{fs, os::unix::fs::PermissionsExt};

use cosh_gateway_contracts::capability::{CapabilityRequest, CapabilityScope, OperationDescriptor};
use cosh_gateway_contracts::common::{
    BoundedName, BoundedOpaque, BoundedText, Digest, IdempotencyKey, RuntimeBindingRef,
    RuntimeSelector,
};
use cosh_gateway_contracts::external::{ExternalRef, ExternalRefKind};
use cosh_gateway_contracts::ids::{
    AgentSessionId, ApprovalId, InstallationId, RequestId, RuntimeBindingId, RuntimeInstanceId,
    TurnId,
};
use cosh_gateway_contracts::profile::GatewayCapabilityProfile;
use cosh_gateway_contracts::runtime::ToolSummary;
use tempfile::TempDir;

use super::*;
use crate::daemon::{actor_id_for_uid, now_ms, CancelTask, SubmitTask};

fn submission(key: &str) -> SubmitTask {
    SubmitTask {
        request_id: RequestId::new(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        intent: BoundedText::new("inspect service").unwrap(),
        target: GatewayCapabilityProfile::task_only_v1().governed_target(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("core").unwrap(),
            profile: Some(BoundedName::new("gateway-brokered-v1").unwrap()),
        },
    }
}

struct NeverStartFactory(Arc<AtomicUsize>);

impl RuntimeFactory for NeverStartFactory {
    fn open(&mut self, _run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err(runtime_lost_error("unexpected_start", "Runtime must not be started twice").unwrap())
    }
}

struct UpdateFactory;

impl RuntimeFactory for UpdateFactory {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        Ok(StartedRuntime {
            binding: runtime_binding(run),
            handle: Box::new(UpdateHandle),
        })
    }
}

struct UpdateHandle;

impl RuntimeHandle for UpdateHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        RuntimePoll::Update {
            sequence: 2,
            update: RuntimeUpdate::Progress {
                summary: BoundedText::new("progress").unwrap(),
            },
        }
    }

    fn shutdown(&mut self, _reason: CancelReason) -> Result<(), ContractError> {
        Ok(())
    }

    fn resolve_provider_permission(
        &mut self,
        _permission: &RuntimePermissionRef,
        _decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError> {
        Ok(())
    }
}

struct ShutdownProbeFactory(Arc<AtomicUsize>);

impl RuntimeFactory for ShutdownProbeFactory {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        Ok(StartedRuntime {
            binding: runtime_binding(run),
            handle: Box::new(ShutdownProbeHandle(Arc::clone(&self.0))),
        })
    }
}

struct ShutdownProbeHandle(Arc<AtomicUsize>);

impl RuntimeHandle for ShutdownProbeHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        RuntimePoll::Pending
    }

    fn shutdown(&mut self, _reason: CancelReason) -> Result<(), ContractError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn resolve_provider_permission(
        &mut self,
        _permission: &RuntimePermissionRef,
        _decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError> {
        Ok(())
    }
}

fn runtime_binding(run: &ScheduledRun) -> RuntimeBindingRef {
    RuntimeBindingRef {
        binding_id: RuntimeBindingId::new(),
        task_id: run.task_id.clone(),
        run_id: run.run_id.clone(),
        agent_session_id: AgentSessionId::new(),
        runtime_instance_id: RuntimeInstanceId::new(),
        runtime_generation: run.lease_generation,
        external_session: ExternalRef {
            kind: ExternalRefKind::AcpSession,
            authority: BoundedName::new("scheduler-test").unwrap(),
            scope_digest: Digest::parse("a".repeat(64)).unwrap(),
            value: BoundedOpaque::new("session-hash").unwrap(),
        },
    }
}

struct PermissionFactory {
    decisions: Arc<Mutex<Vec<RuntimePermissionDecision>>>,
    expires_at_ms: u64,
}

impl RuntimeFactory for PermissionFactory {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        let binding = runtime_binding(run);
        let request = CapabilityRequest {
            request_id: RequestId::new(),
            task_id: run.task_id.clone(),
            run_id: run.run_id.clone(),
            actor: run.actor.clone(),
            target: run.target.clone(),
            operation: OperationDescriptor {
                namespace: BoundedName::new("process").unwrap(),
                name: BoundedName::new("spawn").unwrap(),
                arguments_digest: test_digest(),
            },
            operation_digest: test_digest(),
            requested_scope: CapabilityScope {
                resource: BoundedName::new("process").unwrap(),
                access: BoundedName::new("execute").unwrap(),
            },
            input_digest: test_digest(),
            expires_at_ms: self.expires_at_ms,
        };
        let permission = RuntimePermissionRef {
            binding_id: binding.binding_id.clone(),
            runtime_generation: binding.runtime_generation,
            event_sequence: 2,
            run_id: run.run_id.clone(),
            turn_id: TurnId::new(),
            tool_use_id: None,
            request_id: request.request_id.clone(),
        };
        Ok(StartedRuntime {
            binding,
            handle: Box::new(PermissionHandle {
                permission,
                request,
                emitted: false,
                decisions: Arc::clone(&self.decisions),
            }),
        })
    }
}

struct PermissionHandle {
    permission: RuntimePermissionRef,
    request: CapabilityRequest,
    emitted: bool,
    decisions: Arc<Mutex<Vec<RuntimePermissionDecision>>>,
}

impl RuntimeHandle for PermissionHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        if self.emitted {
            RuntimePoll::Pending
        } else {
            self.emitted = true;
            RuntimePoll::PermissionRequested {
                permission: self.permission.clone(),
                request: Box::new(self.request.clone()),
                summary: ToolSummary {
                    name: BoundedName::new("shell").unwrap(),
                    summary: BoundedText::new("Run the inspected shell command").unwrap(),
                },
            }
        }
    }

    fn shutdown(&mut self, _reason: CancelReason) -> Result<(), ContractError> {
        Ok(())
    }

    fn resolve_provider_permission(
        &mut self,
        permission: &RuntimePermissionRef,
        decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError> {
        assert_eq!(permission, &self.permission);
        self.decisions.lock().unwrap().push(decision);
        Ok(())
    }
}

fn test_digest() -> Digest {
    Digest::parse("a".repeat(64)).unwrap()
}

#[test]
fn invalid_start_intents_are_rejected_before_outbox_claim() {
    for case in ["profile-drift", "v2-target", "v2-runtime", "future-schema"] {
        let root = TempDir::new().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let database_path = root.path().join("gateway.db");
        let installation = InstallationId::new();
        let mut coordinator =
            TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
        let actor = actor_id_for_uid(&installation, 1000).unwrap();
        coordinator.submit(&actor, submission(case)).unwrap();
        let now = now_ms().unwrap().saturating_add(1);
        let candidate = coordinator
            .store
            .peek_ready_outbox(&runtime_start_delivery_kind(), now)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.attempt, 0, "case {case}");
        let mut payload = candidate.payload;
        match case {
            "profile-drift" => {
                payload["capability_profile"]["manifest_digest"] =
                    serde_json::json!("b".repeat(64));
            }
            "v2-target" => {
                payload["schema_version"] = serde_json::json!(2);
                payload
                    .as_object_mut()
                    .unwrap()
                    .remove("capability_profile");
                payload["target"]["identifier"] = serde_json::json!("another-target");
            }
            "v2-runtime" => {
                payload["schema_version"] = serde_json::json!(2);
                payload
                    .as_object_mut()
                    .unwrap()
                    .remove("capability_profile");
                payload["runtime"] = serde_json::json!({"runtime": "acp", "profile": "codex"});
            }
            "future-schema" => payload["schema_version"] = serde_json::json!(4),
            _ => unreachable!(),
        }
        coordinator
            .store
            .replace_outbox_payload_for_test(&candidate.delivery_id, &payload)
            .unwrap();
        drop(coordinator);

        let starts = Arc::new(AtomicUsize::new(0));
        let mut scheduler = TaskScheduler::open(
            &database_path,
            Some(installation),
            BoundedOpaque::new(format!("invalid-start-{case}")).unwrap(),
            NeverStartFactory(Arc::clone(&starts)),
        )
        .unwrap();
        assert!(
            matches!(scheduler.tick(now), Err(GatewayDaemonError::Protocol(_))),
            "case {case}"
        );
        let candidate = scheduler
            .coordinator
            .store
            .peek_ready_outbox(&runtime_start_delivery_kind(), now)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.attempt, 0, "case {case}");
        assert_eq!(starts.load(Ordering::Relaxed), 0, "case {case}");
    }
}

#[test]
fn exact_task_only_v2_intent_maps_to_current_profile() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("compatible-v2-start"))
        .unwrap();
    let now = now_ms().unwrap().saturating_add(1);
    let candidate = coordinator
        .store
        .peek_ready_outbox(&runtime_start_delivery_kind(), now)
        .unwrap()
        .unwrap();
    let mut payload = candidate.payload;
    payload["schema_version"] = serde_json::json!(2);
    payload
        .as_object_mut()
        .unwrap()
        .remove("capability_profile");
    coordinator
        .store
        .replace_outbox_payload_for_test(&candidate.delivery_id, &payload)
        .unwrap();
    drop(coordinator);

    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("compatible-v2-worker").unwrap(),
        UpdateFactory,
    )
    .unwrap();
    assert!(matches!(
        scheduler.tick(now).unwrap(),
        SchedulerTick::Started(_)
    ));
    assert_eq!(
        scheduler
            .active
            .as_ref()
            .unwrap()
            .scheduled
            .capability_profile,
        GatewayCapabilityProfile::task_only_v1().identity()
    );
}

#[test]
fn stale_scheduler_generation_cannot_settle_taken_over_run() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut first = TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    first.submit(&actor, submission("lease-fence")).unwrap();
    let claimed_at = now_ms().unwrap().saturating_add(1);
    let stale = match first
        .claim_runtime_start(
            &BoundedOpaque::new("worker-a").unwrap(),
            claimed_at,
            claimed_at + 10,
        )
        .unwrap()
    {
        RuntimeStartClaim::Claimed { lease, .. } => lease,
        RuntimeStartClaim::Empty | RuntimeStartClaim::Recovered(_) => {
            panic!("first scheduler must claim the queued Run")
        }
    };

    let starts = Arc::new(AtomicUsize::new(0));
    let mut second = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-b").unwrap(),
        NeverStartFactory(Arc::clone(&starts)),
    )
    .unwrap();
    assert!(matches!(
        second.tick(claimed_at + 10).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));
    assert_eq!(starts.load(Ordering::Relaxed), 0);

    assert!(matches!(
        first.settle_succeeded(&stale, claimed_at + 11),
        Err(GatewayDaemonError::Store(
            StoreError::GenerationFenced { .. }
        ))
    ));
}

#[test]
fn runtime_event_sequence_overflow_fails_before_commit() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    let task = coordinator
        .submit(&actor, submission("sequence-overflow"))
        .unwrap();
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-sequence").unwrap(),
        UpdateFactory,
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    scheduler.tick(started_at).unwrap();
    scheduler.active.as_mut().unwrap().next_event_sequence = u64::MAX;

    assert!(matches!(
        scheduler.tick(started_at + 1),
        Err(GatewayDaemonError::Protocol(message))
            if message.contains("sequence exceeds")
    ));
    assert_eq!(coordinator.get(&actor, &task.task_id).unwrap().revision, 4);
}

#[test]
fn scheduler_rejects_a_lease_that_cannot_cover_one_runtime_operation() {
    let error = TaskSchedulerConfig {
        lease_duration: Duration::from_secs(100),
        lease_renewal_margin: Duration::from_secs(30),
        runtime_operation_timeout: Duration::from_secs(70),
    }
    .validate()
    .unwrap_err();

    assert!(matches!(
        error,
        GatewayDaemonError::Protocol(message) if message.contains("must exceed")
    ));
}

#[test]
fn shutdown_preserves_an_already_observed_terminal_outcome() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("shutdown-terminal"))
        .unwrap();
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-shutdown-terminal").unwrap(),
        ShutdownProbeFactory(Arc::clone(&shutdowns)),
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    scheduler.tick(started_at).unwrap();
    scheduler.active.as_mut().unwrap().terminal = Some(TerminalOutcome::Succeeded);

    assert!(matches!(
        scheduler
            .shutdown(now_ms().unwrap().saturating_add(1))
            .unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Succeeded,
            ..
        })
    ));
    assert_eq!(shutdowns.load(Ordering::Relaxed), 0);
}

#[test]
fn shutdown_preserves_an_earlier_abort_failure() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("shutdown-abort"))
        .unwrap();
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-shutdown-abort").unwrap(),
        ShutdownProbeFactory(Arc::clone(&shutdowns)),
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    scheduler.tick(started_at).unwrap();
    scheduler.active.as_mut().unwrap().abort_error =
        Some(runtime_lost_error("earlier_failure", "The Runtime had already failed").unwrap());

    assert!(matches!(
        scheduler
            .shutdown(now_ms().unwrap().saturating_add(1))
            .unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));
    assert_eq!(shutdowns.load(Ordering::Relaxed), 1);
}

#[test]
fn provider_approval_is_dispatched_once_and_delivered_replay_is_read_only() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("provider-approval"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-approval").unwrap(),
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms: i64::MAX as u64,
        },
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    assert!(matches!(
        scheduler.tick(started_at).unwrap(),
        SchedulerTick::Started(_)
    ));
    assert!(matches!(
        scheduler.tick(started_at + 1).unwrap(),
        SchedulerTick::Progressed(TaskView {
            state: TaskState::WaitingApproval,
            ..
        })
    ));
    let approval_id = scheduler
        .active
        .as_ref()
        .unwrap()
        .pending_permission
        .as_ref()
        .unwrap()
        .approval
        .approval_id
        .clone();
    let decision_at = scheduler
        .coordinator
        .store
        .load_approval_record(&approval_id)
        .unwrap()
        .updated_at_ms
        .saturating_add(1);

    assert!(matches!(
        scheduler.resolve_approval(
            &ActorId::new(),
            IdempotencyKey::new("wrong-actor").unwrap(),
            &approval_id,
            ApprovalDecision::Approve,
            started_at + 2,
        ),
        Err(GatewayDaemonError::Unauthorized)
    ));
    assert!(scheduler
        .resolve_approval(
            &actor,
            IdempotencyKey::new("wrong-approval").unwrap(),
            &ApprovalId::new(),
            ApprovalDecision::Approve,
            started_at + 2,
        )
        .is_err());
    assert!(decisions.lock().unwrap().is_empty());

    let replay_key = IdempotencyKey::new("resolve-provider-once").unwrap();
    assert!(matches!(
        scheduler
            .resolve_approval(
                &actor,
                replay_key.clone(),
                &approval_id,
                ApprovalDecision::Approve,
                decision_at,
            )
            .unwrap(),
        SchedulerTick::Progressed(TaskView {
            state: TaskState::Running,
            ..
        })
    ));
    assert_eq!(decisions.lock().unwrap().len(), 1);
    assert!(matches!(
        decisions.lock().unwrap().as_slice(),
        [RuntimePermissionDecision::ProviderNativeAllowOnce]
    ));
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_provider_permission_dispatch_record(&approval_id)
            .unwrap()
            .state,
        ProviderPermissionDispatchState::Delivered
    );

    let approved_task_id = scheduler
        .coordinator
        .store
        .load_approval_record(&approval_id)
        .unwrap()
        .task_id;
    scheduler.active.take();
    coordinator
        .submit(&actor, submission("another-active-run"))
        .unwrap();
    scheduler.tick(now_ms().unwrap().saturating_add(1)).unwrap();
    let replayed = scheduler
        .resolve_approval(
            &actor,
            replay_key,
            &approval_id,
            ApprovalDecision::Approve,
            now_ms().unwrap().saturating_add(1),
        )
        .unwrap();
    assert!(matches!(
        replayed,
        SchedulerTick::Progressed(TaskView {
            task_id,
            state: TaskState::Running,
            ..
        }) if task_id == approved_task_id
    ));
    assert_eq!(decisions.lock().unwrap().len(), 1);
    assert!(scheduler
        .resolve_approval(
            &actor,
            IdempotencyKey::new("change-delivered-decision").unwrap(),
            &approval_id,
            ApprovalDecision::Deny,
            now_ms().unwrap().saturating_add(1),
        )
        .is_err());
    assert_eq!(decisions.lock().unwrap().len(), 1);
}

#[test]
fn expired_provider_approval_fails_closed_instead_of_renewing_forever() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("approval-expiry"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let started_at = now_ms().unwrap().saturating_add(1);
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-expiry").unwrap(),
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms: started_at + 10_000,
        },
    )
    .unwrap();
    scheduler.tick(started_at).unwrap();
    scheduler.tick(started_at + 1).unwrap();
    let approval_id = scheduler
        .active
        .as_ref()
        .unwrap()
        .pending_permission
        .as_ref()
        .unwrap()
        .approval
        .approval_id
        .clone();

    assert!(matches!(
        scheduler.tick(started_at + 10_000).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));
    assert!(decisions.lock().unwrap().is_empty());
    assert!(scheduler.active.is_none());
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_approval_record(&approval_id)
            .unwrap()
            .state,
        ApprovalState::Expired
    );
}

#[test]
fn resolving_at_provider_approval_deadline_expires_without_dispatch() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submission("approval-resolve-expiry"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let started_at = now_ms().unwrap().saturating_add(1);
    let expires_at_ms = started_at + 10_000;
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-resolve-expiry").unwrap(),
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms,
        },
    )
    .unwrap();
    scheduler.tick(started_at).unwrap();
    scheduler.tick(started_at + 1).unwrap();
    let approval_id = scheduler
        .active
        .as_ref()
        .unwrap()
        .pending_permission
        .as_ref()
        .unwrap()
        .approval
        .approval_id
        .clone();

    assert!(matches!(
        scheduler.resolve_approval(
            &actor,
            IdempotencyKey::new("late-provider-approval").unwrap(),
            &approval_id,
            ApprovalDecision::Approve,
            expires_at_ms,
        ),
        Err(GatewayDaemonError::Protocol(message))
            if message == "approval is no longer resolvable"
    ));
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_approval_record(&approval_id)
            .unwrap()
            .state,
        ApprovalState::Expired
    );
    assert!(scheduler
        .coordinator
        .store
        .load_provider_permission_dispatch_record(&approval_id)
        .is_err());
    assert!(matches!(
        scheduler.resolve_approval(
            &actor,
            IdempotencyKey::new("late-provider-approval").unwrap(),
            &approval_id,
            ApprovalDecision::Approve,
            expires_at_ms + 1,
        ),
        Err(GatewayDaemonError::Protocol(message))
            if message == "approval is no longer resolvable"
    ));
    assert!(decisions.lock().unwrap().is_empty());
    let task = scheduler
        .coordinator
        .store
        .load_task(
            &scheduler
                .coordinator
                .store
                .load_approval_record(&approval_id)
                .unwrap()
                .task_id,
        )
        .unwrap();
    assert_eq!(task.state(), TaskState::Failed);
}

#[test]
fn durable_cancellation_takes_priority_over_pending_approval() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    let task = coordinator
        .submit(&actor, submission("approval-cancel"))
        .unwrap();
    let run_id = task.active_run_id.clone().unwrap();
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-cancel").unwrap(),
        PermissionFactory {
            decisions: Arc::new(Mutex::new(Vec::new())),
            expires_at_ms: i64::MAX as u64,
        },
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    scheduler.tick(started_at).unwrap();
    scheduler.tick(started_at + 1).unwrap();
    coordinator
        .cancel(
            &actor,
            CancelTask {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new("cancel-pending-approval").unwrap(),
                task_id: task.task_id,
                run_id,
                expected_revision: None,
            },
        )
        .unwrap();

    assert!(matches!(
        scheduler.tick(started_at + 2).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Cancelled,
            ..
        })
    ));
}
