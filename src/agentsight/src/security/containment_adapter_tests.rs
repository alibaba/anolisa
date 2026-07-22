use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, EventIdentity, FileAction, SecurityEvent, SecurityEventKind,
};

use super::enforcer::{ContainmentReadinessLease, StampedBinding, StampedBindings};
use super::*;
use crate::enforcement::{
    ApplyPolicy, Binding, BindingState, EnforcementClient, EnforcementCoordinator, EnforcementStore,
};
use crate::ingestion_readiness::{GenerationReadiness, ReadinessLease, ReadinessStamp};
use crate::security::{ContainmentLifecycle, RiskCase, RiskSeverity};

const SECOND_NS: u64 = 1_000_000_000;

struct LeasePause {
    entered: Barrier,
    resume: Barrier,
}

fn pause_next(slot: &Mutex<Option<Arc<LeasePause>>>) -> Arc<LeasePause> {
    let pause = Arc::new(LeasePause {
        entered: Barrier::new(2),
        resume: Barrier::new(2),
    });
    *slot.lock().expect("lease pause should lock") = Some(Arc::clone(&pause));
    pause
}

struct PausingProductionEnforcer {
    coordinator: Arc<EnforcementCoordinator>,
    lease_pause: Mutex<Option<Arc<LeasePause>>>,
    detach_calls: AtomicUsize,
}

impl PausingProductionEnforcer {
    fn new(coordinator: Arc<EnforcementCoordinator>) -> Self {
        Self {
            coordinator,
            lease_pause: Mutex::new(None),
            detach_calls: AtomicUsize::new(0),
        }
    }

    fn pause_next_lease(&self) -> Arc<LeasePause> {
        pause_next(&self.lease_pause)
    }
}

impl ContainmentEnforcer for PausingProductionEnforcer {
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<StampedBinding, ContainmentEnforcerError> {
        ContainmentEnforcer::apply_credential_policy(self.coordinator.as_ref(), request)
    }

    fn detach(&self, _: Uuid) -> Result<(), String> {
        self.detach_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn bindings(&self) -> Result<StampedBindings, ContainmentEnforcerError> {
        ContainmentEnforcer::bindings(self.coordinator.as_ref())
    }

    fn lease_ready(
        &self,
        stamp: ReadinessStamp,
    ) -> Result<Box<dyn ContainmentReadinessLease + '_>, ContainmentEnforcerError> {
        if let Some(pause) = self
            .lease_pause
            .lock()
            .expect("lease pause should lock")
            .take()
        {
            pause.entered.wait();
            pause.resume.wait();
        }
        ContainmentEnforcer::lease_ready(self.coordinator.as_ref(), stamp)
    }
}

#[test]
fn exact_binding_from_generation_a_cannot_activate_under_generation_b() {
    let enforcement_store = EnforcementStore::open(":memory:").expect("store should open");
    let enforcement = Arc::new(EnforcementCoordinator::new(
        EnforcementClient::new("/tmp/unused-enforcement.sock"),
        enforcement_store.clone(),
    ));
    let ready_worker = enforcement.ingestion_readiness().candidate();
    enforcement
        .ingestion_readiness()
        .install(Arc::clone(&ready_worker));
    assert!(enforcement.ingestion_readiness().mark_ready(&ready_worker));

    let source_binding_id = Uuid::new_v4();
    let action_binding_id = Uuid::new_v4();
    enforcement_store
        .upsert_binding(&binding(
            source_binding_id,
            999_999,
            42,
            audit_policy("/root/secret.txt"),
        ))
        .expect("source binding should persist");
    enforcement_store
        .upsert_binding(&binding(
            action_binding_id,
            999_999,
            42,
            enforce_policy("/root/secret.txt"),
        ))
        .expect("containment binding should persist");

    let (security_store, case_id, action) =
        security_fixture(source_binding_id, action_binding_id, 999_999, 42);
    let enforcer = Arc::new(PausingProductionEnforcer::new(Arc::clone(&enforcement)));
    let pause = enforcer.pause_next_lease();
    let enforcer_trait: Arc<dyn ContainmentEnforcer> = enforcer.clone();
    let containment = Arc::new(ContainmentCoordinator::new(
        Arc::clone(&security_store),
        enforcer_trait,
    ));
    let reconciling = Arc::clone(&containment);
    let worker = thread::spawn(move || reconciling.reconcile_once(1_000));
    pause.entered.wait();

    let successor = enforcement.ingestion_readiness().candidate();
    enforcement
        .ingestion_readiness()
        .install(Arc::clone(&successor));
    assert!(enforcement.ingestion_readiness().mark_ready(&successor));
    pause.resume.wait();
    assert!(matches!(
        worker.join().expect("reconciler should stop"),
        Err(ContainmentError::Enforcer(_))
    ));
    assert_pending(&security_store, case_id, &action);
    assert_eq!(enforcer.detach_calls.load(Ordering::Acquire), 0);

    containment
        .reconcile_once(1_000 + SECOND_NS)
        .expect("ready successor should recover the exact binding");
    assert_active(&security_store, case_id, &action);
    assert_eq!(
        EnforcementCoordinator::bindings(&enforcement)
            .expect("persisted bindings should remain readable")
            .len(),
        2
    );
}

struct ApplyingGenerationEnforcer {
    readiness: GenerationReadiness,
    bindings: Mutex<Vec<Binding>>,
    lease_pause: Mutex<Option<Arc<LeasePause>>>,
    apply_calls: AtomicUsize,
    detach_calls: AtomicUsize,
}

struct TestReadinessLease<'a> {
    _lease: ReadinessLease<'a>,
}

impl ContainmentReadinessLease for TestReadinessLease<'_> {}

impl ApplyingGenerationEnforcer {
    fn new(readiness: GenerationReadiness, source: Binding) -> Self {
        Self {
            readiness,
            bindings: Mutex::new(vec![source]),
            lease_pause: Mutex::new(None),
            apply_calls: AtomicUsize::new(0),
            detach_calls: AtomicUsize::new(0),
        }
    }

    fn pause_next_lease(&self) -> Arc<LeasePause> {
        pause_next(&self.lease_pause)
    }

    fn stamp(&self) -> Result<ReadinessStamp, ContainmentEnforcerError> {
        self.readiness
            .ready_stamp()
            .ok_or_else(|| ContainmentEnforcerError::Unavailable("ingestion unavailable".into()))
    }
}

impl ContainmentEnforcer for ApplyingGenerationEnforcer {
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<StampedBinding, ContainmentEnforcerError> {
        let stamp = self.stamp()?;
        self.apply_calls.fetch_add(1, Ordering::AcqRel);
        let binding = binding(
            request.binding_id,
            request.root_pid,
            request.process_start_time,
            enforce_policy(&request.policy.source_patterns[0]),
        );
        self.bindings
            .lock()
            .expect("bindings should lock")
            .push(binding.clone());
        Ok(StampedBinding::new(binding, stamp))
    }

    fn detach(&self, _: Uuid) -> Result<(), String> {
        self.detach_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn bindings(&self) -> Result<StampedBindings, ContainmentEnforcerError> {
        let stamp = self.stamp()?;
        Ok(StampedBindings::new(
            self.bindings.lock().expect("bindings should lock").clone(),
            stamp,
        ))
    }

    fn lease_ready(
        &self,
        stamp: ReadinessStamp,
    ) -> Result<Box<dyn ContainmentReadinessLease + '_>, ContainmentEnforcerError> {
        if let Some(pause) = self
            .lease_pause
            .lock()
            .expect("lease pause should lock")
            .take()
        {
            pause.entered.wait();
            pause.resume.wait();
        }
        self.readiness
            .lease_ready(stamp)
            .map(|lease| {
                Box::new(TestReadinessLease { _lease: lease }) as Box<dyn ContainmentReadinessLease>
            })
            .ok_or_else(|| ContainmentEnforcerError::Unavailable("ingestion changed".into()))
    }
}

#[test]
fn apply_ack_from_generation_a_cannot_activate_under_generation_b() {
    let readiness = GenerationReadiness::new("ingestion unavailable");
    let generation_a = readiness.candidate();
    readiness.install(Arc::clone(&generation_a));
    assert!(readiness.mark_ready(&generation_a));
    let source_binding_id = Uuid::new_v4();
    let action_binding_id = Uuid::new_v4();
    let enforcer = Arc::new(ApplyingGenerationEnforcer::new(
        readiness.clone(),
        binding(
            source_binding_id,
            4242,
            99,
            audit_policy("/root/secret.txt"),
        ),
    ));
    let (security_store, case_id, action) =
        security_fixture(source_binding_id, action_binding_id, 4242, 99);
    let pause = enforcer.pause_next_lease();
    let enforcer_trait: Arc<dyn ContainmentEnforcer> = enforcer.clone();
    let containment = Arc::new(ContainmentCoordinator::new(
        Arc::clone(&security_store),
        enforcer_trait,
    ));
    let reconciling = Arc::clone(&containment);
    let worker = thread::spawn(move || reconciling.reconcile_once(1_000));
    pause.entered.wait();

    let generation_b = readiness.candidate();
    readiness.install(Arc::clone(&generation_b));
    assert!(readiness.mark_ready(&generation_b));
    pause.resume.wait();
    assert!(matches!(
        worker.join().expect("reconciler should stop"),
        Err(ContainmentError::Enforcer(_))
    ));
    assert_pending(&security_store, case_id, &action);
    assert_eq!(enforcer.apply_calls.load(Ordering::Acquire), 1);
    assert_eq!(enforcer.detach_calls.load(Ordering::Acquire), 0);

    containment
        .reconcile_once(1_000 + SECOND_NS)
        .expect("generation B should recover the exact binding");
    assert_active(&security_store, case_id, &action);
    assert_eq!(enforcer.apply_calls.load(Ordering::Acquire), 1);
    assert_eq!(enforcer.detach_calls.load(Ordering::Acquire), 0);
}

fn security_fixture(
    source_binding_id: Uuid,
    action_binding_id: Uuid,
    pid: i32,
    process_start_time: u64,
) -> (Arc<SecurityStore>, Uuid, ContainmentAction) {
    let store = Arc::new(SecurityStore::open_in_memory().expect("store should open"));
    let event = evidence(source_binding_id, pid, process_start_time);
    store.insert_event(&event).expect("evidence should persist");
    let case_id = Uuid::new_v4();
    store
        .upsert_case(&risk_case(case_id), &[event.event_id])
        .expect("case should persist");
    let action = pending_action(case_id, action_binding_id, pid, process_start_time);
    store
        .insert_containment_action(&action)
        .expect("action should persist");
    (store, case_id, action)
}

fn assert_pending(store: &SecurityStore, case_id: Uuid, expected: &ContainmentAction) {
    let action = latest_action(store, case_id);
    assert_eq!(action.action_id, expected.action_id);
    assert_eq!(action.binding_id, expected.binding_id);
    assert_eq!(action.lifecycle_state, ContainmentLifecycle::Pending);
    assert_eq!(action.attempt_count, 1);
    assert_eq!(action.next_retry_at_ns, Some(1_000 + SECOND_NS));
}

fn assert_active(store: &SecurityStore, case_id: Uuid, expected: &ContainmentAction) {
    let action = latest_action(store, case_id);
    assert_eq!(action.action_id, expected.action_id);
    assert_eq!(action.binding_id, expected.binding_id);
    assert_eq!(action.lifecycle_state, ContainmentLifecycle::Active);
}

fn latest_action(store: &SecurityStore, case_id: Uuid) -> ContainmentAction {
    store
        .latest_containment_action(case_id)
        .expect("action query should work")
        .expect("action should exist")
}

fn binding(
    binding_id: Uuid,
    root_pid: i32,
    process_start_time: u64,
    policy_dsl: String,
) -> Binding {
    Binding {
        request: ApplyPolicy {
            binding_id,
            agent_id: "hermes-test".into(),
            session_id: Some("session-1".into()),
            root_pid,
            process_start_time,
            policy_id: "credential-exfiltration".into(),
            policy_revision: "3".into(),
            policy_dsl,
        },
        state: BindingState::Enforced,
        message: None,
        domain_id: Some(1),
    }
}

fn audit_policy(source: &str) -> String {
    compiled_policy("notify", source)
}

fn enforce_policy(source: &str) -> String {
    compiled_policy("block", source)
}

fn compiled_policy(action: &str, source: &str) -> String {
    format!(
        "source AGENT = exec \"**\"\nsource CREDENTIAL = file \"{source}\"\nrule agentsight-credential-exfiltration:\n  {action} connect endpoint \"*\" if CREDENTIAL unless target \"trusted.example:443\"\n  because \"credential-derived data reached an untrusted network target\"\n"
    )
}

fn evidence(binding_id: Uuid, pid: i32, process_start_time: u64) -> SecurityEvent {
    SecurityEvent {
        event_id: Uuid::new_v4(),
        occurred_at_ns: 1,
        observed_at_ns: 1,
        identity: EventIdentity {
            binding_id,
            agent_id: "hermes-test".into(),
            agent_name: Some("Hermes test".into()),
            session_id: Some("session-1".into()),
            conversation_id: None,
            tool_call_id: None,
            pid,
            process_start_time,
            ppid: None,
            cgroup_id: None,
            protocol_version: 1,
            enforcer_version: "test".into(),
            actplane_revision: "test".into(),
        },
        kind: SecurityEventKind::FileAction(FileAction {
            policy_id: "credential-exfiltration".into(),
            policy_revision: 3,
            operation: "read".into(),
            path: "~/redacted-secret".into(),
            resource_class: "credential".into(),
            succeeded: true,
            errno: None,
            rule_id: None,
        }),
    }
}

fn risk_case(case_id: Uuid) -> RiskCase {
    RiskCase {
        case_id,
        correlation_key: format!("case-{case_id}"),
        policy_id: "credential-exfiltration".into(),
        policy_revision: 3,
        agent_id: "hermes-test".into(),
        session_id: Some("session-1".into()),
        severity: RiskSeverity::High,
        risk_score: 85,
        status: RiskCaseStatus::Open,
        blocked: false,
        opened_at_ns: 1,
        updated_at_ns: 1,
        summary: "credential reached an untrusted target".into(),
    }
}

fn pending_action(
    case_id: Uuid,
    binding_id: Uuid,
    root_pid: i32,
    process_start_time: u64,
) -> ContainmentAction {
    ContainmentAction {
        action_id: Uuid::new_v4(),
        case_id,
        binding_id,
        agent_id: "hermes-test".into(),
        root_pid,
        process_start_time,
        source_path: "/root/secret.txt".into(),
        duration_secs: Some(60),
        expires_at_ns: Some(3 * SECOND_NS),
        lifecycle_state: ContainmentLifecycle::Pending,
        blocked_at_ns: None,
        requested_by: "principal:test-operator".into(),
        failure_stage: None,
        failure_reason: None,
        attempt_count: 0,
        next_retry_at_ns: Some(1_000),
        created_at_ns: 10,
        updated_at_ns: 10,
    }
}
