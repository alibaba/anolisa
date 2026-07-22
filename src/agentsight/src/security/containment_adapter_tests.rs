use std::sync::Arc;

use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, CredentialExfiltrationPolicy, EventIdentity, FileAction, PolicyMode,
    SecurityEvent, SecurityEventKind,
};

use super::*;
use crate::enforcement::{ApplyPolicy, BindingState, EnforcementClient, EnforcementStore};
use crate::security::{ContainmentLifecycle, RiskCase, RiskSeverity};

const SECOND_NS: u64 = 1_000_000_000;

#[test]
fn revoked_ingestion_keeps_an_exact_binding_pending_until_its_successor_is_ready() {
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
            audit_policy("/root/secret.txt"),
        ))
        .expect("source binding should persist");
    enforcement_store
        .upsert_binding(&binding(
            action_binding_id,
            enforce_policy("/root/secret.txt"),
        ))
        .expect("containment binding should persist");
    assert_eq!(
        ContainmentEnforcer::bindings(enforcement.as_ref())
            .expect("ready snapshots should remain available")
            .len(),
        2
    );

    let security_store = Arc::new(SecurityStore::open_in_memory().expect("store should open"));
    let event = evidence(source_binding_id);
    security_store
        .insert_event(&event)
        .expect("evidence should persist");
    let case_id = Uuid::new_v4();
    security_store
        .upsert_case(&risk_case(case_id), &[event.event_id])
        .expect("case should persist");
    let action = pending_action(case_id, action_binding_id);
    security_store
        .insert_containment_action(&action)
        .expect("action should persist");
    let enforcer: Arc<dyn ContainmentEnforcer> = enforcement.clone();
    let containment = ContainmentCoordinator::new(Arc::clone(&security_store), enforcer);

    let successor = enforcement.ingestion_readiness().candidate();
    enforcement
        .ingestion_readiness()
        .install(Arc::clone(&successor));
    assert!(matches!(
        ContainmentEnforcer::bindings(enforcement.as_ref()),
        Err(ContainmentEnforcerError::Unavailable(_))
    ));
    assert!(matches!(
        ContainmentEnforcer::apply_credential_policy(
            enforcement.as_ref(),
            apply_request(Uuid::new_v4()),
        ),
        Err(ContainmentEnforcerError::Unavailable(_))
    ));
    assert!(matches!(
        containment.reconcile_once(1_000),
        Err(ContainmentError::Enforcer(_))
    ));
    let pending = security_store
        .latest_containment_action(case_id)
        .expect("action query should work")
        .expect("action should exist");
    assert_eq!(pending.action_id, action.action_id);
    assert_eq!(pending.binding_id, action.binding_id);
    assert_eq!(pending.lifecycle_state, ContainmentLifecycle::Pending);
    assert_eq!(pending.attempt_count, 1);
    assert_eq!(pending.next_retry_at_ns, Some(1_000 + SECOND_NS));
    assert_eq!(
        security_store
            .case_detail(case_id)
            .expect("case should load")
            .case
            .status,
        RiskCaseStatus::Open
    );

    assert!(enforcement.ingestion_readiness().mark_ready(&successor));
    containment
        .reconcile_once(1_000 + SECOND_NS)
        .expect("ready successor should recover the exact binding");
    let active = security_store
        .latest_containment_action(case_id)
        .expect("action query should work")
        .expect("action should exist");
    assert_eq!(active.action_id, action.action_id);
    assert_eq!(active.binding_id, action.binding_id);
    assert_eq!(active.lifecycle_state, ContainmentLifecycle::Active);
    assert_eq!(
        security_store
            .case_detail(case_id)
            .expect("case should load")
            .case
            .status,
        RiskCaseStatus::Confirmed
    );
    assert_eq!(
        EnforcementCoordinator::bindings(&enforcement)
            .expect("persisted bindings should remain readable")
            .len(),
        2
    );
}

fn apply_request(binding_id: Uuid) -> ApplyCredentialPolicy {
    ApplyCredentialPolicy {
        binding_id,
        agent_id: "hermes-test".into(),
        session_id: Some("session-1".into()),
        root_pid: 999_999,
        process_start_time: 42,
        policy: CredentialExfiltrationPolicy {
            policy_id: "credential-exfiltration".into(),
            revision: 3,
            source_patterns: vec!["/root/secret.txt".into()],
            trusted_endpoints: vec!["trusted.example:443".into()],
            taint_label: "CREDENTIAL".into(),
            taint_ttl_secs: 900,
            mode: PolicyMode::Enforce,
        },
    }
}

fn binding(binding_id: Uuid, policy_dsl: String) -> Binding {
    Binding {
        request: ApplyPolicy {
            binding_id,
            agent_id: "hermes-test".into(),
            session_id: Some("session-1".into()),
            root_pid: 999_999,
            process_start_time: 42,
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

fn evidence(binding_id: Uuid) -> SecurityEvent {
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
            pid: 999_999,
            process_start_time: 42,
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

fn pending_action(case_id: Uuid, binding_id: Uuid) -> ContainmentAction {
    ContainmentAction {
        action_id: Uuid::new_v4(),
        case_id,
        binding_id,
        agent_id: "hermes-test".into(),
        root_pid: 999_999,
        process_start_time: 42,
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
