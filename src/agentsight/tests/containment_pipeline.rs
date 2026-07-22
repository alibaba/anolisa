use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agentsight::enforcement::{ApplyPolicy, Binding, BindingState};
use agentsight::security::{
    ContainmentCandidate, ContainmentCoordinator, ContainmentEnforcer, ContainmentError,
    ContainmentLifecycle, ContainmentRequest, RiskCase, RiskCaseStatus, RiskSeverity,
    SecurityStore,
};
use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, EventIdentity, FileAction, PolicyMode, SecurityEvent, SecurityEventKind,
};
use uuid::Uuid;

#[derive(Default)]
struct FakeEnforcer {
    bindings: Mutex<Vec<Binding>>,
    applied: Mutex<Vec<ApplyCredentialPolicy>>,
    failure: Mutex<Option<String>>,
    acknowledgement: Mutex<Option<BindingState>>,
    apply_calls: AtomicUsize,
}

impl FakeEnforcer {
    fn set_bindings(&self, bindings: Vec<Binding>) {
        *self.bindings.lock().expect("bindings should lock") = bindings;
    }

    fn fail_next_apply(&self, message: &str) {
        *self.failure.lock().expect("failure should lock") = Some(message.into());
    }

    fn acknowledge_as(&self, state: BindingState) {
        *self.acknowledgement.lock().expect("ack should lock") = Some(state);
    }

    fn apply_calls(&self) -> usize {
        self.apply_calls.load(Ordering::Acquire)
    }

    fn applied(&self) -> Vec<ApplyCredentialPolicy> {
        self.applied.lock().expect("applied should lock").clone()
    }
}

impl ContainmentEnforcer for FakeEnforcer {
    fn apply_credential_policy(&self, request: ApplyCredentialPolicy) -> Result<Binding, String> {
        self.apply_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(message) = self.failure.lock().expect("failure should lock").take() {
            return Err(message);
        }
        self.applied
            .lock()
            .expect("applied should lock")
            .push(request.clone());
        let state = self
            .acknowledgement
            .lock()
            .expect("ack should lock")
            .take()
            .unwrap_or(BindingState::Enforced);
        let trusted = request
            .policy
            .trusted_endpoints
            .first()
            .map(|value| format!(" unless target \"{value}\""))
            .unwrap_or_default();
        Ok(Binding {
            request: ApplyPolicy {
                binding_id: request.binding_id,
                agent_id: request.agent_id,
                session_id: request.session_id,
                root_pid: request.root_pid,
                process_start_time: request.process_start_time,
                policy_id: request.policy.policy_id,
                policy_revision: request.policy.revision.to_string(),
                policy_dsl: format!(
                    "source AGENT = exec \"**\"\nsource CREDENTIAL = file \"{}\"\nrule agentsight-credential-exfiltration:\n  block connect endpoint \"*\" if CREDENTIAL{trusted}\n",
                    request.policy.source_patterns[0]
                ),
            },
            state,
            message: None,
            domain_id: Some(7),
        })
    }

    fn detach(&self, _binding_id: Uuid) -> Result<(), String> {
        Ok(())
    }

    fn bindings(&self) -> Result<Vec<Binding>, String> {
        Ok(self.bindings.lock().expect("bindings should lock").clone())
    }
}

struct Fixture {
    case_id: Uuid,
    binding_id: Uuid,
    store: Arc<SecurityStore>,
    enforcer: Arc<FakeEnforcer>,
    coordinator: ContainmentCoordinator,
}

impl Fixture {
    fn new(source_policy: Option<&str>, root_pid: i32, process_start_time: u64) -> Self {
        let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
        let enforcer = Arc::new(FakeEnforcer::default());
        let binding_id = Uuid::new_v4();
        if let Some(policy_dsl) = source_policy {
            enforcer.set_bindings(vec![binding(
                binding_id,
                root_pid,
                process_start_time,
                policy_dsl,
            )]);
        }
        let event = evidence(binding_id, root_pid, process_start_time);
        store.insert_event(&event).expect("evidence should persist");
        let case_id = Uuid::new_v4();
        store
            .upsert_case(&risk_case(case_id), &[event.event_id])
            .expect("case should persist");
        let enforcer_trait: Arc<dyn ContainmentEnforcer> = enforcer.clone();
        let coordinator = ContainmentCoordinator::new(Arc::clone(&store), enforcer_trait);
        Self {
            case_id,
            binding_id,
            store,
            enforcer,
            coordinator,
        }
    }

    fn set_status(&self, status: RiskCaseStatus) {
        self.store
            .review_case(self.case_id, status, 2)
            .expect("case status should update");
    }
}

fn binding(binding_id: Uuid, root_pid: i32, start_time: u64, policy_dsl: &str) -> Binding {
    Binding {
        request: ApplyPolicy {
            binding_id,
            agent_id: "hermes-test".into(),
            session_id: Some("session-1".into()),
            root_pid,
            process_start_time: start_time,
            policy_id: "credential-exfiltration".into(),
            policy_revision: "3".into(),
            policy_dsl: policy_dsl.into(),
        },
        state: BindingState::Enforced,
        message: None,
        domain_id: Some(1),
    }
}

fn evidence(binding_id: Uuid, pid: i32, start_time: u64) -> SecurityEvent {
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
            process_start_time: start_time,
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

fn policy(source: &str) -> String {
    format!(
        "source AGENT = exec \"**\"\nsource CREDENTIAL = file \"{source}\"\nrule agentsight-credential-exfiltration:\n  notify connect endpoint \"*\" if CREDENTIAL unless target \"trusted.example:443\"\n  because \"credential-derived data reached an untrusted network target\"\n"
    )
}

#[test]
fn plan_recovers_only_the_original_binding_path() {
    let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 42);
    let candidate = ContainmentCandidate {
        agent_id: "hermes-test".into(),
        root_pid: 1234,
        process_start_time: 88,
        display_name: "replacement".into(),
    };

    let plan = fixture
        .coordinator
        .plan(fixture.case_id, vec![candidate.clone()])
        .expect("plan should load");

    assert_eq!(plan.source_path, "/root/secret.txt");
    assert_eq!(plan.candidates, vec![candidate]);
    assert!(!plan.original_target_valid);
    assert_eq!(plan.default_duration_secs, 900);
    assert_eq!(
        (plan.min_duration_secs, plan.max_duration_secs),
        (60, 86_400)
    );
}

#[test]
fn missing_original_binding_never_uses_redacted_evidence_path() {
    let fixture = Fixture::new(None, 999_999, 42);
    assert!(matches!(
        fixture.coordinator.plan(fixture.case_id, Vec::new()),
        Err(ContainmentError::SourcePolicyUnavailable(id)) if id == fixture.case_id
    ));
}

#[test]
fn malformed_source_declarations_are_rejected() {
    let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 42);
    let malformed = [
        "source AGENT = exec \"**\"\n",
        "source CREDENTIAL = file \"relative/secret\"\n",
        "source CREDENTIAL = file \"/root/secret\\\"\n",
        "source CREDENTIAL=file \"/root/secret\"\n",
        "source CREDENTIAL = file \"/root/a\"\nsource CREDENTIAL = file \"/root/b\"\n",
        "source OTHER = file \"/root/secret\"\n",
    ];

    for dsl in malformed {
        fixture
            .enforcer
            .set_bindings(vec![binding(fixture.binding_id, 999_999, 42, dsl)]);
        assert!(matches!(
            fixture.coordinator.plan(fixture.case_id, Vec::new()),
            Err(ContainmentError::SourcePolicyUnavailable(_))
        ));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::process::{Child, Command, Stdio};

    use super::*;

    struct LiveProcess(Child);

    impl LiveProcess {
        fn spawn() -> Self {
            Self(
                Command::new("sleep")
                    .arg("60")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("sleep fixture should start"),
            )
        }

        fn pid(&self) -> i32 {
            self.0.id() as i32
        }

        fn start_time(&self) -> u64 {
            let stat = fs::read_to_string(format!("/proc/{}/stat", self.pid()))
                .expect("child stat should load");
            let close = stat.rfind(')').expect("child stat should have a name");
            stat[close + 1..]
                .split_whitespace()
                .nth(19)
                .expect("child stat should have a start time")
                .parse()
                .expect("child start time should parse")
        }
    }

    impl Drop for LiveProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn live_fixture() -> (LiveProcess, Fixture) {
        let process = LiveProcess::spawn();
        let fixture = Fixture::new(
            Some(&policy("/root/secret.txt")),
            process.pid(),
            process.start_time(),
        );
        (process, fixture)
    }

    fn contain(
        fixture: &Fixture,
        duration_secs: Option<u64>,
    ) -> Result<agentsight::security::ContainmentAction, ContainmentError> {
        let root_pid = fixture
            .store
            .case_detail(fixture.case_id)
            .expect("case should load")
            .evidence[0]
            .identity
            .pid;
        fixture.coordinator.contain(
            fixture.case_id,
            ContainmentRequest {
                root_pid,
                duration_secs,
            },
            "dashboard-token",
        )
    }

    #[test]
    fn contain_uses_original_policy_and_confirms_only_after_enforced_ack() {
        let (_process, fixture) = live_fixture();
        let action = contain(&fixture, Some(900)).expect("containment should apply");

        assert_eq!(action.source_path, "/root/secret.txt");
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Active);
        assert_eq!(
            fixture
                .store
                .case_detail(fixture.case_id)
                .expect("case should load")
                .case
                .status,
            RiskCaseStatus::Confirmed
        );
        let applied = fixture.enforcer.applied();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].policy.mode, PolicyMode::Enforce);
        assert_eq!(applied[0].policy.revision, 3);
        assert_eq!(applied[0].policy.source_patterns, ["/root/secret.txt"]);
        assert_eq!(applied[0].policy.trusted_endpoints, ["trusted.example:443"]);
        assert_eq!(applied[0].policy.taint_label, "CREDENTIAL");
    }

    #[test]
    fn repeated_active_request_returns_the_existing_action() {
        let (_process, fixture) = live_fixture();
        let first = contain(&fixture, Some(900)).expect("first containment should apply");
        fixture.enforcer.set_bindings(Vec::new());
        let second = contain(&fixture, Some(900)).expect("repeat should be idempotent");

        assert_eq!(first.action_id, second.action_id);
        assert_eq!(fixture.enforcer.apply_calls(), 1);
        assert!(matches!(
            contain(&fixture, Some(901)),
            Err(ContainmentError::IncompatibleAction(_))
        ));
    }

    #[test]
    fn duration_and_process_identity_are_validated_before_apply() {
        let (_process, fixture) = live_fixture();
        for duration_secs in [Some(59), Some(86_401)] {
            assert!(matches!(
                contain(&fixture, duration_secs),
                Err(ContainmentError::InvalidDuration)
            ));
        }
        let root_pid = fixture.enforcer.bindings().expect("bindings should load")[0]
            .request
            .root_pid;
        fixture.enforcer.set_bindings(vec![binding(
            fixture.binding_id,
            root_pid,
            1,
            &policy("/root/secret.txt"),
        )]);
        assert!(matches!(
            contain(&fixture, Some(900)),
            Err(ContainmentError::RootProcessStale(pid)) if pid == root_pid
        ));
        assert_eq!(fixture.enforcer.apply_calls(), 0);
    }

    #[test]
    fn persistent_duration_stays_explicitly_persistent() {
        let (_process, fixture) = live_fixture();
        let action = contain(&fixture, None).expect("persistent containment should apply");
        assert_eq!(action.duration_secs, None);
        assert_eq!(action.expires_at_ns, None);
    }

    #[test]
    fn attach_failure_is_persisted_and_does_not_confirm_the_case() {
        let (_process, fixture) = live_fixture();
        fixture.enforcer.fail_next_apply("adapter unavailable");
        assert!(matches!(
            contain(&fixture, Some(900)),
            Err(ContainmentError::Enforcer(message)) if message == "adapter unavailable"
        ));
        let action = fixture
            .store
            .latest_containment_action(fixture.case_id)
            .expect("action query should work")
            .expect("failed action should persist");
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Failed);
        assert_eq!(
            action.failure_reason.as_deref(),
            Some("adapter unavailable")
        );
        assert_eq!(
            action.failure_stage,
            Some(agentsight::security::ContainmentFailureStage::Attach)
        );
        assert_eq!(
            fixture
                .store
                .case_detail(fixture.case_id)
                .expect("case should load")
                .case
                .status,
            RiskCaseStatus::Open
        );
    }

    #[test]
    fn non_enforced_acknowledgement_is_not_reported_active() {
        let (_process, fixture) = live_fixture();
        fixture.enforcer.acknowledge_as(BindingState::Pending);
        assert!(matches!(
            contain(&fixture, Some(900)),
            Err(ContainmentError::Enforcer(_))
        ));
        assert_eq!(
            fixture
                .store
                .latest_containment_action(fixture.case_id)
                .expect("action query should work")
                .expect("action should exist")
                .lifecycle_state,
            ContainmentLifecycle::Failed
        );
    }

    #[test]
    fn ineligible_case_states_never_apply() {
        for status in [
            RiskCaseStatus::FalsePositive,
            RiskCaseStatus::AcceptedRisk,
            RiskCaseStatus::Resolved,
        ] {
            let (_process, fixture) = live_fixture();
            fixture.set_status(status);
            assert!(matches!(
                contain(&fixture, Some(900)),
                Err(ContainmentError::IneligibleCase { status: actual, .. }) if actual == status
            ));
            assert_eq!(fixture.enforcer.apply_calls(), 0);
        }
    }

    #[test]
    fn replacement_candidate_identity_must_match_proc_start_time() {
        let candidate_process = LiveProcess::spawn();
        let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 1);
        let mut candidate = ContainmentCandidate {
            agent_id: "hermes-test".into(),
            root_pid: candidate_process.pid(),
            process_start_time: candidate_process.start_time() + 1,
            display_name: "replacement".into(),
        };
        fixture
            .coordinator
            .plan(fixture.case_id, vec![candidate.clone()])
            .expect("plan should cache candidates");
        assert!(matches!(
            fixture.coordinator.contain(
                fixture.case_id,
                ContainmentRequest {
                    root_pid: candidate.root_pid,
                    duration_secs: Some(900)
                },
                "dashboard-token",
            ),
            Err(ContainmentError::RootProcessStale(_))
        ));

        candidate.process_start_time = candidate_process.start_time();
        fixture
            .coordinator
            .plan(fixture.case_id, vec![candidate.clone()])
            .expect("plan should refresh candidates");
        let action = fixture
            .coordinator
            .contain(
                fixture.case_id,
                ContainmentRequest {
                    root_pid: candidate.root_pid,
                    duration_secs: Some(900),
                },
                "dashboard-token",
            )
            .expect("valid candidate should apply");
        assert_eq!(action.process_start_time, candidate.process_start_time);
    }
}
