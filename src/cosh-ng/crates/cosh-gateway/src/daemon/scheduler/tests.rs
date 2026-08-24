use std::collections::BTreeMap;
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
use crate::runtime::{
    AcpRuntimeProfileId, InstalledAcpRuntimePortFactory, LocalOsActorResolver,
    ScheduledAgentRuntimeFactory, TrustedWorkspaceResolver,
};

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

fn delegated_submission(key: &str) -> SubmitTask {
    SubmitTask {
        request_id: RequestId::new(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        intent: BoundedText::new("update dependencies and run tests").unwrap(),
        target: GatewayCapabilityProfile::delegated_acp_v1().governed_target(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("acp").unwrap(),
            profile: Some(BoundedName::new("codex").unwrap()),
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

struct RejectingShutdownFactory {
    opens: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
}

impl RuntimeFactory for RejectingShutdownFactory {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(StartedRuntime {
            binding: runtime_binding(run),
            handle: Box::new(RejectingShutdownHandle(Arc::clone(&self.shutdowns))),
        })
    }
}

struct RejectingShutdownHandle(Arc<AtomicUsize>);

impl RuntimeHandle for RejectingShutdownHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        RuntimePoll::Pending
    }

    fn shutdown(&mut self, _reason: CancelReason) -> Result<(), ContractError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err(runtime_lost_error(
            "runtime_shutdown_unacknowledged",
            "Runtime shutdown was not acknowledged",
        )
        .unwrap())
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
    abandon_before_decision: bool,
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
            callback: Some(
                cosh_gateway_contracts::runtime::ProviderPermissionCallbackV2 {
                    provider_session_digest: test_digest(),
                    provider_request_id_digest: test_digest(),
                    provider_tool_call_id_digest: test_digest(),
                    ordered_option_set_digest: test_digest(),
                    callback_payload_digest: test_digest(),
                    normalized_operation_digest: request.operation_digest.clone(),
                },
            ),
        };
        Ok(StartedRuntime {
            binding,
            handle: Box::new(PermissionHandle {
                permission,
                request,
                emitted: false,
                abandon_before_decision: self.abandon_before_decision,
                abandonment_emitted: false,
                terminal_emitted: false,
                decisions: Arc::clone(&self.decisions),
            }),
        })
    }
}

struct PermissionHandle {
    permission: RuntimePermissionRef,
    request: CapabilityRequest,
    emitted: bool,
    abandon_before_decision: bool,
    abandonment_emitted: bool,
    terminal_emitted: bool,
    decisions: Arc<Mutex<Vec<RuntimePermissionDecision>>>,
}

impl RuntimeHandle for PermissionHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        if self.emitted && self.abandon_before_decision && !self.abandonment_emitted {
            self.abandonment_emitted = true;
            RuntimePoll::PermissionAbandoned {
                sequence: self.permission.event_sequence + 1,
                permission: self.permission.clone(),
            }
        } else if self.abandonment_emitted && !self.terminal_emitted {
            self.terminal_emitted = true;
            RuntimePoll::Cancelled {
                cause: RuntimeCancellationCause::ProviderPermissionAbandoned {
                    permission: self.permission.clone(),
                },
            }
        } else if self.emitted
            && !self.terminal_emitted
            && self
                .decisions
                .lock()
                .unwrap()
                .last()
                .is_some_and(|decision| matches!(decision, RuntimePermissionDecision::Deny { .. }))
        {
            self.terminal_emitted = true;
            RuntimePoll::Cancelled {
                cause: RuntimeCancellationCause::ProviderPermissionDenied {
                    permission: self.permission.clone(),
                },
            }
        } else if self.emitted {
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
fn exact_checkpoint_intent_starts_only_under_checkpoint_profile() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
    let mut coordinator = TaskCoordinator::open_for_capability_profile(
        &database_path,
        Some(installation.clone()),
        profile,
    )
    .unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(
            &actor,
            SubmitTask {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new("checkpoint-start").unwrap(),
                intent: BoundedText::new("checkpoint workspace").unwrap(),
                target: profile.governed_target(),
                runtime: RuntimeSelector {
                    runtime: BoundedName::new("core").unwrap(),
                    profile: Some(BoundedName::new("gateway-checkpoint-v1").unwrap()),
                },
            },
        )
        .unwrap();
    drop(coordinator);

    let mut scheduler = TaskScheduler::open_for_capability_profile(
        &database_path,
        Some(installation),
        BoundedOpaque::new("checkpoint-worker").unwrap(),
        profile,
        UpdateFactory,
    )
    .unwrap();
    assert!(matches!(
        scheduler.tick(now_ms().unwrap().saturating_add(1)).unwrap(),
        SchedulerTick::Started(_)
    ));
    assert_eq!(
        scheduler
            .active
            .as_ref()
            .unwrap()
            .scheduled
            .capability_profile,
        profile.identity()
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
fn unacknowledged_shutdown_fails_durably_and_restart_does_not_replay() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    let task = coordinator
        .submit(&actor, submission("shutdown-unacknowledged"))
        .unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation.clone()),
        BoundedOpaque::new("worker-shutdown-unacknowledged").unwrap(),
        RejectingShutdownFactory {
            opens: Arc::clone(&opens),
            shutdowns: Arc::clone(&shutdowns),
        },
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
    scheduler.tick(started_at).unwrap();

    assert!(matches!(
        scheduler.shutdown(started_at + 1).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    assert_eq!(shutdowns.load(Ordering::Relaxed), 1);
    drop(scheduler);

    let restart_opens = Arc::new(AtomicUsize::new(0));
    let mut restarted = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-after-shutdown-failure").unwrap(),
        NeverStartFactory(Arc::clone(&restart_opens)),
    )
    .unwrap();
    assert_eq!(restarted.tick(started_at + 2).unwrap(), SchedulerTick::Idle);
    assert_eq!(restart_opens.load(Ordering::Relaxed), 0);
    assert_eq!(
        restarted
            .coordinator
            .get(&actor, &task.task_id)
            .unwrap()
            .state,
        TaskState::Failed
    );
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
            abandon_before_decision: false,
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
        scheduler.resolve_approval_for_task(
            &actor,
            IdempotencyKey::new("wrong-task").unwrap(),
            &TaskId::new(),
            &approval_id,
            ApprovalDecision::Approve,
            decision_at,
        ),
        Err(GatewayDaemonError::Unauthorized)
    ));
    assert!(decisions.lock().unwrap().is_empty());

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
        ProviderPermissionDispatchState::Written
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
fn denied_provider_permission_accepts_only_its_matching_cancelled_terminal() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    let task = coordinator
        .submit(&actor, submission("provider-denial-cancel"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-denial-cancel").unwrap(),
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms: i64::MAX as u64,
            abandon_before_decision: false,
        },
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
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
    let decision_at = scheduler
        .coordinator
        .store
        .load_approval_record(&approval_id)
        .unwrap()
        .updated_at_ms
        .saturating_add(1);

    assert!(matches!(
        scheduler
            .resolve_approval(
                &actor,
                IdempotencyKey::new("deny-provider-once").unwrap(),
                &approval_id,
                ApprovalDecision::Deny,
                decision_at,
            )
            .unwrap(),
        SchedulerTick::Progressed(TaskView {
            state: TaskState::Running,
            ..
        })
    ));
    assert!(matches!(
        scheduler.tick(decision_at.saturating_add(1)).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));
    assert!(matches!(
        decisions.lock().unwrap().as_slice(),
        [RuntimePermissionDecision::Deny { .. }]
    ));
    let (events, _) = scheduler
        .coordinator
        .store
        .load_task_events_for_owner(&task.task_id, &actor, None, 64)
        .unwrap();
    let failure = events.into_iter().find_map(|event| match event.event {
        TaskEvent::TaskFailed { error } => Some(error),
        _ => None,
    });
    assert_eq!(
        failure.expect("denial must settle durably").code.as_str(),
        "provider_permission_denied"
    );
}

#[test]
fn provider_cancel_atomically_abandons_pending_approval_before_terminal() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    let task = coordinator
        .submit(&actor, submission("provider-abandons-permission"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = TaskScheduler::open(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-provider-abandon").unwrap(),
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms: i64::MAX as u64,
            abandon_before_decision: true,
        },
    )
    .unwrap();
    let started_at = now_ms().unwrap().saturating_add(1);
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
        scheduler.tick(now_ms().unwrap().saturating_add(1)).unwrap(),
        SchedulerTick::Progressed(TaskView {
            state: TaskState::Running,
            ..
        })
    ));
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_approval_record(&approval_id)
            .unwrap()
            .state,
        ApprovalState::Cancelled
    );
    assert!(decisions.lock().unwrap().is_empty());
    assert!(scheduler
        .coordinator
        .store
        .load_provider_permission_dispatch_record(&approval_id)
        .is_err());
    assert!(matches!(
        scheduler.tick(now_ms().unwrap().saturating_add(1)).unwrap(),
        SchedulerTick::Settled(TaskView {
            state: TaskState::Failed,
            ..
        })
    ));

    let (events, _) = scheduler
        .coordinator
        .store
        .load_task_events_for_owner(&task.task_id, &actor, None, 64)
        .unwrap();
    let requested = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::ApprovalRequested { .. }))
        .unwrap();
    let abandoned = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::ApprovalAbandoned { .. }))
        .unwrap();
    let failed = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::TaskFailed { .. }))
        .unwrap();
    assert!(requested < abandoned && abandoned < failed);
}

#[test]
fn delegated_acp_profile_durably_allows_provider_callbacks_once() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let profile = GatewayCapabilityProfile::delegated_acp_v1();
    let mut coordinator = TaskCoordinator::open_for_capability_profile(
        &database_path,
        Some(installation.clone()),
        profile,
    )
    .unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, delegated_submission("delegated-provider-approval"))
        .unwrap();
    let decisions = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = TaskScheduler::open_for_capability_profile(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-delegated-approval").unwrap(),
        profile,
        PermissionFactory {
            decisions: Arc::clone(&decisions),
            expires_at_ms: i64::MAX as u64,
            abandon_before_decision: false,
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
            state: TaskState::Running,
            ..
        })
    ));
    assert!(scheduler
        .active
        .as_ref()
        .unwrap()
        .pending_permission
        .is_none());
    assert_eq!(
        decisions.lock().unwrap().as_slice(),
        [RuntimePermissionDecision::ProviderNativeAllowOnce]
    );
    let task_id = scheduler.active.as_ref().unwrap().scheduled.task_id.clone();
    let (events, _) = scheduler
        .coordinator
        .store
        .load_task_events_for_owner(&task_id, &actor, None, 64)
        .unwrap();
    let approval_id = events
        .into_iter()
        .find_map(|event| match event.event {
            TaskEvent::ApprovalRequested { approval } => Some(approval.approval_id),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_provider_permission_dispatch_record(&approval_id)
            .unwrap()
            .state,
        ProviderPermissionDispatchState::Written
    );
}

#[test]
fn delegated_acp_task_runs_to_a_durable_result_through_the_real_bridge() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let workspace_path = root.path().join("workspace");
    fs::create_dir(&workspace_path).unwrap();
    let response_path = root.path().join("permission-response.json");
    let adapter_path = root.path().join("codex-acp");
    fs::write(
        &adapter_path,
        format!(
            r#"#!/bin/sh
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{{"jsonrpc":"2.0","id":"cosh-acp-1","result":{{"protocolVersion":1,"agentCapabilities":{{}},"agentInfo":{{"name":"@agentclientprotocol/codex-acp","title":"Codex","version":"1.6.2"}},"_meta":{{"jetbrains":{{"air":{{"version":1,"capabilities":["sessionFailure","agentFileChangeReport"]}}}}}}}}}}' ;;
        2) printf '%s\n' '{{"jsonrpc":"2.0","id":"cosh-acp-2","result":{{"sessionId":"delegated-session"}}}}' ;;
        3)
            printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"delegated-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"started "}}}}}}}}'
            printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"delegated-session","update":{{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","title":"Read workspace","status":"in_progress"}}}}}}'
            printf '%s\n' '{{"jsonrpc":"2.0","id":41,"method":"session/request_permission","params":{{"sessionId":"delegated-session","toolCall":{{"toolCallId":"tool-1","status":"pending"}},"options":[{{"optionId":"allow","name":"Allow once","kind":"allow_once"}},{{"optionId":"always","name":"Always","kind":"allow_always"}}]}}}}' ;;
        4)
            printf '%s\n' "$line" > '{}'
            printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"delegated-session","update":{{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed"}}}}}}'
            printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"delegated-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"finished"}}}}}}}}'
            printf '%s\n' '{{"jsonrpc":"2.0","id":"cosh-acp-3","result":{{"stopReason":"end_turn"}}}}' ;;
    esac
done
"#,
            response_path.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&adapter_path, fs::Permissions::from_mode(0o700)).unwrap();

    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let profile = GatewayCapabilityProfile::delegated_acp_v1();
    let target = profile.governed_target();
    let actor_resolver = LocalOsActorResolver::new(installation.clone(), 1000);
    let actor_ref = actor_resolver.actor_ref().clone();
    let actor = actor_ref.actor_id.clone();
    let workspace = TrustedWorkspaceResolver::new(target, &workspace_path).unwrap();
    let workspace_ref = workspace.workspace_ref().clone();
    let factory = InstalledAcpRuntimePortFactory::new(
        installation.clone(),
        actor_resolver,
        workspace,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, adapter_path)]),
        BTreeMap::new(),
    )
    .unwrap();
    let mut coordinator = TaskCoordinator::open_for_capability_profile(
        &database_path,
        Some(installation.clone()),
        profile,
    )
    .unwrap();
    let task = coordinator
        .submit_admitted(
            &actor_ref,
            &workspace_ref,
            delegated_submission("real-acp-delegation"),
        )
        .unwrap();
    let mut scheduler = TaskScheduler::open_for_capability_profile(
        &database_path,
        Some(installation),
        BoundedOpaque::new("worker-real-acp").unwrap(),
        profile,
        ScheduledAgentRuntimeFactory::new(factory),
    )
    .unwrap();

    let mut settled = None;
    for _ in 0..200 {
        let tick = scheduler.tick(now_ms().unwrap()).unwrap();
        if let SchedulerTick::Settled(view) = tick {
            settled = Some(view);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let settled = settled.expect("delegated ACP Task must settle");
    if settled.state != TaskState::Succeeded {
        let (events, _) = scheduler
            .coordinator
            .store
            .load_task_events_for_owner(&task.task_id, &actor, None, 64)
            .unwrap();
        panic!("delegated ACP Task failed: {events:#?}");
    }
    assert_eq!(settled.state, TaskState::Succeeded);
    let permission_response = fs::read_to_string(&response_path).unwrap();
    assert!(permission_response.contains("\"optionId\":\"allow\""));
    assert!(!permission_response.contains("always"));

    let (events, _) = scheduler
        .coordinator
        .store
        .load_task_events_for_owner(&task.task_id, &actor, None, 64)
        .unwrap();
    let progress = events
        .iter()
        .filter_map(|event| match &event.event {
            TaskEvent::RuntimeEventRecorded {
                update: RuntimeUpdate::Progress { summary },
                ..
            } => Some(summary.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(progress, "started finished");
    let requested = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::ApprovalRequested { .. }))
        .expect("approval request must be durable before response dispatch");
    let resolved = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::ApprovalResolved { .. }))
        .expect("automatic allow-once resolution must be durable");
    let succeeded = events
        .iter()
        .position(|event| matches!(event.event, TaskEvent::TaskSucceeded))
        .expect("Task success must be durable");
    assert!(requested < resolved && resolved < succeeded);
    let approval_id = events
        .iter()
        .find_map(|event| match &event.event {
            TaskEvent::ApprovalRequested { approval } => Some(approval.approval_id.clone()),
            _ => None,
        })
        .expect("provider approval identity must be durable");
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_provider_permission_dispatch_record(&approval_id)
            .unwrap()
            .state,
        ProviderPermissionDispatchState::Written
    );
    let response_before_replay = fs::read(&response_path).unwrap();
    let replayed = scheduler
        .resolve_approval(
            &actor,
            IdempotencyKey::new(format!("delegated-acp-allow-once-{}", approval_id.as_str()))
                .unwrap(),
            &approval_id,
            ApprovalDecision::Approve,
            now_ms().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        replayed,
        SchedulerTick::Progressed(TaskView {
            state: TaskState::Succeeded,
            ..
        })
    ));
    assert_eq!(fs::read(&response_path).unwrap(), response_before_replay);
    assert_eq!(
        scheduler
            .coordinator
            .store
            .load_provider_permission_dispatch_record(&approval_id)
            .unwrap()
            .state,
        ProviderPermissionDispatchState::Written
    );
    assert!(events
        .iter()
        .any(|event| matches!(event.event, TaskEvent::TaskSucceeded)));
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
            abandon_before_decision: false,
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
            abandon_before_decision: false,
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
            abandon_before_decision: false,
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
