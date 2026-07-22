use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use agentsight::enforcement::{ApplyPolicy, Binding, BindingState};
use agentsight::security::{
    ContainmentCandidate, ContainmentCoordinator, ContainmentEnforcer, ContainmentError,
    ContainmentFailureStage, ContainmentLifecycle, ContainmentRequest, RiskCase, RiskCaseStatus,
    RiskSeverity, SecurityStore,
};
use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, EventIdentity, FileAction, PolicyMode, SecurityEvent, SecurityEventKind,
};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum AckMutation {
    State(BindingState),
    Session,
    Source,
    TrustedEndpoint,
    Notify,
}

struct ApplyPause {
    entered: Barrier,
    resume: Barrier,
}

#[derive(Default)]
struct FakeEnforcer {
    bindings: Mutex<Vec<Binding>>,
    applied: Mutex<Vec<ApplyCredentialPolicy>>,
    failure: Mutex<Option<String>>,
    detach_failure: Mutex<Option<String>>,
    acknowledgement: Mutex<Option<AckMutation>>,
    pause: Mutex<Option<Arc<ApplyPause>>>,
    detached: Mutex<Vec<Uuid>>,
    apply_calls: AtomicUsize,
}

impl FakeEnforcer {
    fn set_bindings(&self, bindings: Vec<Binding>) {
        *self.bindings.lock().expect("bindings should lock") = bindings;
    }

    fn fail_next_apply(&self, message: &str) {
        *self.failure.lock().expect("failure should lock") = Some(message.into());
    }

    fn fail_next_detach(&self, message: &str) {
        *self
            .detach_failure
            .lock()
            .expect("detach failure should lock") = Some(message.into());
    }

    fn mutate_ack(&self, mutation: AckMutation) {
        *self.acknowledgement.lock().expect("ack should lock") = Some(mutation);
    }

    fn pause_apply(&self) -> Arc<ApplyPause> {
        let pause = Arc::new(ApplyPause {
            entered: Barrier::new(2),
            resume: Barrier::new(2),
        });
        *self.pause.lock().expect("pause should lock") = Some(Arc::clone(&pause));
        pause
    }

    fn apply_calls(&self) -> usize {
        self.apply_calls.load(Ordering::Acquire)
    }

    fn applied(&self) -> Vec<ApplyCredentialPolicy> {
        self.applied.lock().expect("applied should lock").clone()
    }

    fn detached(&self) -> Vec<Uuid> {
        self.detached.lock().expect("detached should lock").clone()
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
        let pause = self.pause.lock().expect("pause should lock").take();
        if let Some(pause) = pause {
            pause.entered.wait();
            pause.resume.wait();
        }
        let mutation = self.acknowledgement.lock().expect("ack should lock").take();
        let state = match mutation {
            Some(AckMutation::State(state)) => state,
            _ => BindingState::Enforced,
        };
        let session_id = match mutation {
            Some(AckMutation::Session) => Some("session-other".into()),
            _ => request.session_id,
        };
        let source = match mutation {
            Some(AckMutation::Source) => "/root/other.txt",
            _ => &request.policy.source_patterns[0],
        };
        let trusted = match mutation {
            Some(AckMutation::TrustedEndpoint) => Some("other.example:443"),
            _ => request.policy.trusted_endpoints.first().map(String::as_str),
        };
        let action = if matches!(mutation, Some(AckMutation::Notify)) {
            "notify"
        } else {
            "block"
        };
        Ok(Binding {
            request: ApplyPolicy {
                binding_id: request.binding_id,
                agent_id: request.agent_id,
                session_id,
                root_pid: request.root_pid,
                process_start_time: request.process_start_time,
                policy_id: request.policy.policy_id,
                policy_revision: request.policy.revision.to_string(),
                policy_dsl: compiled_policy(action, source, trusted),
            },
            state,
            message: None,
            domain_id: Some(7),
        })
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), String> {
        self.detached
            .lock()
            .expect("detached should lock")
            .push(binding_id);
        match self
            .detach_failure
            .lock()
            .expect("detach failure should lock")
            .take()
        {
            Some(message) => Err(message),
            None => Ok(()),
        }
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
    coordinator: Arc<ContainmentCoordinator>,
}

impl Fixture {
    fn new(source_policy: Option<&str>, root_pid: i32, process_start_time: u64) -> Self {
        let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
        Self::with_store(store, source_policy, root_pid, process_start_time)
    }

    fn at_path(
        path: &std::path::Path,
        source_policy: Option<&str>,
        root_pid: i32,
        process_start_time: u64,
    ) -> Self {
        let store = Arc::new(SecurityStore::open(path).expect("fixture store should open"));
        Self::with_store(store, source_policy, root_pid, process_start_time)
    }

    fn with_identities(
        source_policy: Option<&str>,
        root_pid: i32,
        process_start_time: u64,
        evidence_pid: i32,
        evidence_start_time: u64,
    ) -> Self {
        let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
        Self::with_store_and_evidence(
            store,
            source_policy,
            root_pid,
            process_start_time,
            evidence_pid,
            evidence_start_time,
        )
    }

    fn with_store(
        store: Arc<SecurityStore>,
        source_policy: Option<&str>,
        root_pid: i32,
        process_start_time: u64,
    ) -> Self {
        Self::with_store_and_evidence(
            store,
            source_policy,
            root_pid,
            process_start_time,
            root_pid,
            process_start_time,
        )
    }

    fn with_store_and_evidence(
        store: Arc<SecurityStore>,
        source_policy: Option<&str>,
        root_pid: i32,
        process_start_time: u64,
        evidence_pid: i32,
        evidence_start_time: u64,
    ) -> Self {
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
        let event = evidence(binding_id, evidence_pid, evidence_start_time);
        store.insert_event(&event).expect("evidence should persist");
        let case_id = Uuid::new_v4();
        store
            .upsert_case(&risk_case(case_id), &[event.event_id])
            .expect("case should persist");
        let enforcer_trait: Arc<dyn ContainmentEnforcer> = enforcer.clone();
        let coordinator = Arc::new(ContainmentCoordinator::new(
            Arc::clone(&store),
            enforcer_trait,
        ));
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

    fn status(&self) -> RiskCaseStatus {
        self.store
            .case_detail(self.case_id)
            .expect("case should load")
            .case
            .status
    }

    fn candidate(&self) -> ContainmentCandidate {
        let identity = &self
            .store
            .case_detail(self.case_id)
            .expect("case should load")
            .evidence[0]
            .identity;
        ContainmentCandidate {
            agent_id: identity.agent_id.clone(),
            root_pid: identity.pid,
            process_start_time: identity.process_start_time,
            display_name: "Hermes test".into(),
        }
    }

    fn contain_as(
        &self,
        duration_secs: Option<u64>,
        candidates: &[ContainmentCandidate],
        requested_by: &str,
    ) -> Result<agentsight::security::ContainmentAction, ContainmentError> {
        let root_pid = candidates.first().map_or(0, |candidate| candidate.root_pid);
        self.coordinator.contain(
            self.case_id,
            ContainmentRequest {
                root_pid,
                duration_secs,
            },
            candidates,
            requested_by,
        )
    }

    fn contain(
        &self,
        duration_secs: Option<u64>,
    ) -> Result<agentsight::security::ContainmentAction, ContainmentError> {
        self.contain_as(
            duration_secs,
            &[self.candidate()],
            "principal:test-operator",
        )
    }

    fn latest_action(&self) -> agentsight::security::ContainmentAction {
        self.store
            .latest_containment_action(self.case_id)
            .expect("action query should work")
            .expect("action should exist")
    }
}

fn coordinator(
    store: Arc<SecurityStore>,
    enforcer: Arc<FakeEnforcer>,
) -> Arc<ContainmentCoordinator> {
    let enforcer_trait: Arc<dyn ContainmentEnforcer> = enforcer;
    Arc::new(ContainmentCoordinator::new(store, enforcer_trait))
}

fn contain_candidate(
    coordinator: &ContainmentCoordinator,
    case_id: Uuid,
    candidate: ContainmentCandidate,
    duration_secs: Option<u64>,
) -> Result<agentsight::security::ContainmentAction, ContainmentError> {
    coordinator.contain(
        case_id,
        ContainmentRequest {
            root_pid: candidate.root_pid,
            duration_secs,
        },
        std::slice::from_ref(&candidate),
        "principal:test-operator",
    )
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

fn compiled_policy(action: &str, source: &str, trusted: Option<&str>) -> String {
    let trusted = trusted
        .map(|value| format!(" unless target \"{value}\""))
        .unwrap_or_default();
    format!(
        "source AGENT = exec \"**\"\nsource CREDENTIAL = file \"{source}\"\nrule agentsight-credential-exfiltration:\n  {action} connect endpoint \"*\" if CREDENTIAL{trusted}\n  because \"credential-derived data reached an untrusted network target\"\n"
    )
}

fn policy(source: &str) -> String {
    compiled_policy("notify", source, Some("trusted.example:443"))
}

#[test]
fn plan_recovers_only_the_original_binding_path() {
    let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 42);

    let plan = fixture
        .coordinator
        .plan(fixture.case_id, Vec::new())
        .expect("plan should load");

    assert_eq!(plan.source_path, "/root/secret.txt");
    assert!(plan.candidates.is_empty());
    assert!(!plan.original_target_valid);
    assert_eq!(plan.default_duration_secs, 900);
    assert_eq!(
        (plan.min_duration_secs, plan.max_duration_secs),
        (60, 86_400)
    );
    let candidate = ContainmentCandidate {
        agent_id: "hermes-test".into(),
        root_pid: 12_345,
        process_start_time: 88,
        display_name: "replacement".into(),
    };
    assert!(matches!(
        fixture
            .coordinator
            .plan(fixture.case_id, vec![candidate.clone(), candidate]),
        Err(ContainmentError::AmbiguousCandidate(12_345))
    ));
}

#[test]
fn missing_original_binding_never_uses_redacted_evidence_path() {
    let fixture = Fixture::new(None, 999_999, 42);
    assert!(matches!(
        fixture.coordinator.plan(fixture.case_id, Vec::new()),
        Err(ContainmentError::SourcePolicyUnavailable(id)) if id == fixture.case_id
    ));
    let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 42);
    let mut original = binding(fixture.binding_id, 999_999, 42, &policy("/root/secret.txt"));
    original.state = BindingState::Pending;
    fixture.enforcer.set_bindings(vec![original]);

    assert!(matches!(
        fixture.coordinator.plan(fixture.case_id, Vec::new()),
        Err(ContainmentError::SourcePolicyUnavailable(_))
    ));
}

#[test]
fn malformed_source_declarations_are_rejected() {
    let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 42);
    let malformed = vec![
        "source AGENT = exec \"**\"\n".into(),
        "source CREDENTIAL = file \"relative/secret\"\n".into(),
        "source CREDENTIAL = file \"/root/secret\\\"\n".into(),
        "source CREDENTIAL=file \"/root/secret\"\n".into(),
        "source CREDENTIAL = file \"/root/a\"\nsource CREDENTIAL = file \"/root/b\"\n".into(),
        "source OTHER = file \"/root/secret\"\n".into(),
        policy("/root/./secret"),
        policy("/root/../secret"),
        format!(
            "{}source  CREDENTIAL = file \"/root/other\"\n",
            policy("/root/secret")
        ),
        format!(
            "{}source\tCREDENTIAL = file \"/root/other\"\n",
            policy("/root/secret")
        ),
    ];

    for dsl in malformed {
        fixture
            .enforcer
            .set_bindings(vec![binding(fixture.binding_id, 999_999, 42, &dsl)]);
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

    #[test]
    fn root_binding_accepts_child_process_evidence() {
        let root = LiveProcess::spawn();
        let child = LiveProcess::spawn();
        let fixture = Fixture::with_identities(
            Some(&policy("/root/secret.txt")),
            root.pid(),
            root.start_time(),
            child.pid(),
            child.start_time(),
        );

        let plan = fixture
            .coordinator
            .plan(fixture.case_id, Vec::new())
            .expect("root binding should authorize descendant evidence");

        assert_eq!(
            plan.original_target.expect("original target").root_pid,
            root.pid()
        );
        assert!(plan.original_target_valid);
    }

    #[test]
    fn plan_validates_live_original_without_candidates() {
        let (_process, fixture) = live_fixture();

        let plan = fixture
            .coordinator
            .plan(fixture.case_id, Vec::new())
            .expect("plan should validate its original binding directly");

        assert!(plan.candidates.is_empty());
        assert!(plan.original_target_valid);
        let root_pid = plan.original_target.expect("original target").root_pid;
        let action = fixture
            .coordinator
            .contain(
                fixture.case_id,
                ContainmentRequest {
                    root_pid,
                    duration_secs: Some(900),
                },
                &[],
                "principal:test-operator",
            )
            .expect("original binding should not require a replacement candidate");
        assert_eq!(action.root_pid, root_pid);
    }

    #[test]
    fn contain_uses_original_policy_and_confirms_only_after_enforced_ack() {
        let (_process, fixture) = live_fixture();
        let candidate = fixture.candidate();
        let action = fixture
            .contain_as(Some(900), &[candidate], "  principal:test-operator  ")
            .expect("containment should apply");

        assert_eq!(action.source_path, "/root/secret.txt");
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Active);
        assert_eq!(action.requested_by, "principal:test-operator");
        assert_eq!(fixture.status(), RiskCaseStatus::Confirmed);
        let applied = fixture.enforcer.applied();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].policy.mode, PolicyMode::Enforce);
        assert_eq!(applied[0].policy.revision, 3);
        assert_eq!(applied[0].policy.source_patterns, ["/root/secret.txt"]);
        assert_eq!(applied[0].policy.trusted_endpoints, ["trusted.example:443"]);
        assert_eq!(applied[0].policy.taint_label, "CREDENTIAL");
    }

    #[test]
    fn duration_and_process_identity_are_validated_before_apply() {
        let (_process, fixture) = live_fixture();
        for duration_secs in [Some(59), Some(86_401)] {
            assert!(matches!(
                fixture.contain(duration_secs),
                Err(ContainmentError::InvalidDuration)
            ));
        }
        let stale = ContainmentCandidate {
            agent_id: "hermes-test".into(),
            root_pid: 999_999,
            process_start_time: 1,
            display_name: "stale replacement".into(),
        };
        assert!(matches!(
            fixture.contain_as(
                Some(900),
                &[stale.clone()],
                "principal:test-operator",
            ),
            Err(ContainmentError::RootProcessStale(pid)) if pid == stale.root_pid
        ));
        assert_eq!(fixture.enforcer.apply_calls(), 0);
    }

    #[test]
    fn exact_duration_boundaries_and_persistent_mode_are_accepted() {
        for duration_secs in [Some(60), Some(86_400), None] {
            let (_process, fixture) = live_fixture();
            let action = fixture
                .contain(duration_secs)
                .expect("valid duration should apply");
            assert_eq!(action.duration_secs, duration_secs);
            assert_eq!(action.expires_at_ns.is_none(), duration_secs.is_none());
        }
    }

    #[test]
    fn attach_failure_is_persisted_and_does_not_confirm_the_case() {
        let (_process, fixture) = live_fixture();
        fixture.enforcer.fail_next_apply("adapter unavailable");
        assert!(matches!(
            fixture.contain(Some(900)),
            Err(ContainmentError::Enforcer(message)) if message == "adapter unavailable"
        ));
        let action = fixture.latest_action();
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Failed);
        assert_eq!(
            action.failure_reason.as_deref(),
            Some("adapter unavailable")
        );
        assert_eq!(
            action.failure_stage,
            Some(agentsight::security::ContainmentFailureStage::Attach)
        );
        assert_eq!(fixture.status(), RiskCaseStatus::Open);
    }

    #[test]
    fn acknowledgement_must_match_exact_enforce_semantics() {
        for mutation in [
            AckMutation::State(BindingState::Pending),
            AckMutation::Session,
            AckMutation::Source,
            AckMutation::TrustedEndpoint,
            AckMutation::Notify,
        ] {
            let (_process, fixture) = live_fixture();
            fixture.enforcer.mutate_ack(mutation);
            assert!(matches!(
                fixture.contain(Some(900)),
                Err(ContainmentError::Enforcer(_))
            ));
            let action = fixture.latest_action();
            assert_eq!(action.lifecycle_state, ContainmentLifecycle::Failed);
            assert_eq!(fixture.enforcer.detached(), [action.binding_id]);
        }
    }

    #[test]
    fn detach_failure_keeps_the_claim_actionable() {
        let (_process, fixture) = live_fixture();
        fixture
            .enforcer
            .mutate_ack(AckMutation::State(BindingState::Pending));
        fixture
            .enforcer
            .fail_next_detach("detach adapter unavailable");

        let error = fixture
            .contain(Some(900))
            .expect_err("invalid acknowledgement must be detached");
        assert!(matches!(
            &error,
            ContainmentError::CleanupRequired { reason, .. }
                if reason.contains("detach adapter unavailable")
        ));
        let action = fixture.latest_action();
        assert!(matches!(
            error,
            ContainmentError::CleanupRequired { action_id, binding_id, .. }
                if action_id == action.action_id && binding_id == action.binding_id
        ));
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Expiring);
        assert_eq!(action.failure_stage, Some(ContainmentFailureStage::Detach));
        assert_eq!(action.attempt_count, 1);
        assert!(action.next_retry_at_ns.is_some());
        assert!(
            action
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("detach adapter unavailable"))
        );
        assert!(matches!(
            fixture.contain(Some(900)),
            Err(ContainmentError::ContainmentExpiring(id)) if id == action.action_id
        ));
        assert_eq!(fixture.enforcer.apply_calls(), 1);
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
                fixture.contain(Some(900)),
                Err(ContainmentError::IneligibleCase { status: actual, .. }) if actual == status
            ));
            assert_eq!(fixture.enforcer.apply_calls(), 0);
        }
    }

    #[test]
    fn replacement_candidate_identity_must_match_proc_start_time() {
        let candidate_process = LiveProcess::spawn();
        let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 1);
        let candidate = ContainmentCandidate {
            agent_id: "hermes-test".into(),
            root_pid: candidate_process.pid(),
            process_start_time: candidate_process.start_time(),
            display_name: "replacement".into(),
        };
        let action = contain_candidate(
            &fixture.coordinator,
            fixture.case_id,
            candidate.clone(),
            Some(900),
        )
        .expect("fresh valid candidate should apply without a plan call");
        assert_eq!(action.process_start_time, candidate.process_start_time);
    }

    #[test]
    fn plan_returns_only_live_candidates_and_creates_no_post_authority() {
        let replacement = LiveProcess::spawn();
        let fixture = Fixture::new(Some(&policy("/root/secret.txt")), 999_999, 1);
        let candidate = ContainmentCandidate {
            agent_id: "hermes-test".into(),
            root_pid: replacement.pid(),
            process_start_time: replacement.start_time(),
            display_name: "replacement".into(),
        };
        let stale = ContainmentCandidate {
            agent_id: "hermes-test".into(),
            root_pid: 999_999,
            process_start_time: 1,
            display_name: "stale".into(),
        };
        let plan = fixture
            .coordinator
            .plan(fixture.case_id, vec![candidate.clone(), stale])
            .expect("plan should load");
        assert_eq!(plan.candidates, [candidate.clone()]);
        assert!(!plan.original_target_valid);

        assert!(matches!(
            fixture.coordinator.contain(
                fixture.case_id,
                ContainmentRequest {
                    root_pid: candidate.root_pid,
                    duration_secs: Some(900),
                },
                &[],
                "principal:test-operator",
            ),
            Err(ContainmentError::RootProcessStale(pid)) if pid == candidate.root_pid
        ));
        assert_eq!(fixture.enforcer.apply_calls(), 0);
    }

    #[test]
    fn requested_by_is_validated_before_mutation() {
        let (_process, fixture) = live_fixture();
        let candidate = fixture.candidate();
        for requested_by in [
            String::new(),
            "   ".into(),
            "x".repeat(129),
            "principal:\noperator".into(),
        ] {
            assert!(matches!(
                fixture.contain_as(Some(900), &[candidate.clone()], &requested_by),
                Err(ContainmentError::InvalidRequestedBy)
            ));
        }
        assert_eq!(fixture.enforcer.apply_calls(), 0);
        assert_eq!(
            fixture
                .store
                .latest_containment_action(fixture.case_id)
                .expect("action query should work"),
            None
        );
    }

    #[test]
    fn durable_claim_serializes_coordinators_and_types_live_states() {
        let process = LiveProcess::spawn();
        let path = std::env::temp_dir().join(format!("containment-race-{}.db", Uuid::new_v4()));
        let fixture = Fixture::at_path(
            &path,
            Some(&policy("/root/secret.txt")),
            process.pid(),
            process.start_time(),
        );
        let second_store = Arc::new(SecurityStore::open(&path).expect("second store should open"));
        let second = coordinator(Arc::clone(&second_store), Arc::clone(&fixture.enforcer));
        let candidate = fixture.candidate();
        let pause = fixture.enforcer.pause_apply();
        let first = Arc::clone(&fixture.coordinator);
        let first_candidate = candidate.clone();
        let case_id = fixture.case_id;
        let worker = std::thread::spawn(move || {
            contain_candidate(&first, case_id, first_candidate, Some(900))
        });
        pause.entered.wait();
        let repeat =
            |duration| contain_candidate(&second, fixture.case_id, candidate.clone(), duration);

        assert!(matches!(
            repeat(Some(900)),
            Err(ContainmentError::ContainmentInProgress(_))
        ));
        pause.resume.wait();
        let active = worker
            .join()
            .expect("containment worker should join")
            .expect("first containment should activate");
        fixture.enforcer.set_bindings(Vec::new());
        let repeated = repeat(Some(900)).expect("active claim should be idempotent");
        assert_eq!(repeated.action_id, active.action_id);
        assert_eq!(fixture.enforcer.apply_calls(), 1);
        assert!(matches!(
            repeat(Some(901)),
            Err(ContainmentError::IncompatibleAction(_))
        ));

        let mut expiring = active;
        expiring.lifecycle_state = ContainmentLifecycle::Expiring;
        second_store
            .update_containment_action(&expiring)
            .expect("action should become expiring");
        assert!(matches!(
            repeat(Some(900)),
            Err(ContainmentError::ContainmentExpiring(id)) if id == expiring.action_id
        ));
        drop(fixture);
        drop(second);
        drop(second_store);
        std::fs::remove_file(path).expect("fixture database should be removed");
    }

    #[test]
    fn review_race_detaches_and_persists_reconcile_failure() {
        let (_process, fixture) = live_fixture();
        let pause = fixture.enforcer.pause_apply();
        let coordinator = Arc::clone(&fixture.coordinator);
        let candidate = fixture.candidate();
        let case_id = fixture.case_id;
        let worker = std::thread::spawn(move || {
            contain_candidate(&coordinator, case_id, candidate, Some(900))
        });
        pause.entered.wait();
        fixture.set_status(RiskCaseStatus::FalsePositive);
        pause.resume.wait();

        assert!(matches!(
            worker.join().expect("containment worker should join"),
            Err(ContainmentError::CaseEligibilityChanged {
                status: RiskCaseStatus::FalsePositive,
                ..
            })
        ));
        let action = fixture.latest_action();
        assert_eq!(action.lifecycle_state, ContainmentLifecycle::Failed);
        assert_eq!(
            action.failure_stage,
            Some(ContainmentFailureStage::Reconcile)
        );
        assert!(
            action
                .failure_reason
                .as_deref()
                .is_some_and(|reason| !reason.chars().any(char::is_control))
        );
        assert_eq!(fixture.enforcer.detached(), [action.binding_id]);
        assert_eq!(fixture.status(), RiskCaseStatus::FalsePositive);
    }
}
