use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use actix_web::http::StatusCode;
use actix_web::test as awtest;
use actix_web::{App, web};
use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, Binding, BindingState, EventIdentity, FileAction, SecurityEvent,
    SecurityEventKind,
};
use serde_json::Value;
use uuid::Uuid;

use super::super::auth::{AuthMiddleware, DashboardAuth};
use super::super::{AppState, SecurityObservabilityConfig, configure_routes};
use super::{start_reconciler, stop_reconciler, trusted_candidates};
use crate::config::ServerAuthConfig;
use crate::enforcement::ApplyPolicy;
use crate::grader::EvaluationStore;
use crate::health::{AgentHealthState, AgentHealthStatus, AgentRole, HealthStore};
use crate::security::{
    ContainmentCoordinator, ContainmentEnforcer, ContainmentError, RiskCase, RiskCaseStatus,
    RiskSeverity, SecurityStore,
};

#[derive(Default)]
struct FakeEnforcer {
    bindings: Mutex<Vec<Binding>>,
    apply_count: AtomicUsize,
}

impl ContainmentEnforcer for FakeEnforcer {
    fn apply_credential_policy(&self, request: ApplyCredentialPolicy) -> Result<Binding, String> {
        self.apply_count.fetch_add(1, Ordering::AcqRel);
        let source = request
            .policy
            .source_patterns
            .first()
            .cloned()
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
                policy_dsl: policy("block", &source),
            },
            state: BindingState::Enforced,
            message: None,
            domain_id: Some(1),
        })
    }

    fn detach(&self, _: Uuid) -> Result<(), String> {
        Ok(())
    }

    fn bindings(&self) -> Result<Vec<Binding>, String> {
        Ok(self
            .bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

struct ApiFixture {
    case_id: Uuid,
    child: Child,
    store: Arc<SecurityStore>,
    health: Arc<RwLock<HealthStore>>,
    coordinator: Arc<ContainmentCoordinator>,
    enforcer: Arc<FakeEnforcer>,
    auth: Arc<DashboardAuth>,
    auth_dir: PathBuf,
}

impl ApiFixture {
    fn new(auth_enabled: bool) -> Self {
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("fixture process should start");
        let live_pid = child.id() as i32;
        let health = Arc::new(RwLock::new(HealthStore::new()));
        health
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .update(
                live_pid as u32,
                health_status(live_pid as u32, "hermes-test"),
            );

        let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
        let enforcer = Arc::new(FakeEnforcer::default());
        let source_binding_id = Uuid::new_v4();
        enforcer
            .bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(source_binding(source_binding_id));
        let event = evidence(source_binding_id);
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

        let auth_dir = std::env::temp_dir().join(format!("containment-auth-{case_id}"));
        std::fs::create_dir_all(&auth_dir).expect("auth fixture directory should exist");
        let auth = Arc::new(DashboardAuth::init(
            &ServerAuthConfig {
                enabled: auth_enabled,
            },
            &auth_dir,
        ));
        Self {
            case_id,
            child,
            store,
            health,
            coordinator,
            enforcer,
            auth,
            auth_dir,
        }
    }

    fn data(&self) -> web::Data<AppState> {
        self.data_with_containment(Some(Arc::clone(&self.coordinator)))
    }

    fn data_with_containment(
        &self,
        containment: Option<Arc<ContainmentCoordinator>>,
    ) -> web::Data<AppState> {
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::clone(&self.health),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:"))
                    .expect("evaluation fixture should open"),
            ),
            enforcement: None,
            containment,
            security_store: Arc::clone(&self.store),
            security_observability: SecurityObservabilityConfig::default(),
            auth: Arc::clone(&self.auth),
        })
    }

    fn contain_uri(&self) -> String {
        format!("/api/audit/cases/{}/contain", self.case_id)
    }
}

impl Drop for ApiFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.auth_dir);
    }
}

#[actix_web::test]
async fn plan_uses_live_health_and_omits_sensitive_policy_fields() {
    let fixture = ApiFixture::new(false);
    let app = awtest::init_service(
        App::new()
            .app_data(fixture.data())
            .configure(configure_routes),
    )
    .await;

    let request = awtest::TestRequest::get()
        .uri(&format!(
            "/api/audit/cases/{}/containment-plan",
            fixture.case_id
        ))
        .to_request();
    let response = awtest::call_service(&app, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = awtest::read_body(response).await;
    let value: Value = serde_json::from_slice(&body).expect("plan should be JSON");
    assert_eq!(value["data"]["original_target_valid"], false);
    assert_eq!(
        value["data"]["candidates"][0]["root_pid"],
        fixture.child.id()
    );
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("/root/secret.txt"));
    assert!(!text.contains("policy_dsl"));
}

#[actix_web::test]
async fn post_rebuilds_candidates_instead_of_trusting_the_plan() {
    let fixture = ApiFixture::new(false);
    let app = awtest::init_service(
        App::new()
            .app_data(fixture.data())
            .configure(configure_routes),
    )
    .await;
    fixture
        .health
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove_by_pid(fixture.child.id());

    let response = awtest::call_service(
        &app,
        awtest::TestRequest::post()
            .uri(&fixture.contain_uri())
            .set_json(serde_json::json!({
                "root_pid": fixture.child.id(),
                "duration_secs": 60,
            }))
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let value: Value = awtest::read_body_json(response).await;
    assert_eq!(value["error"]["code"], "root_process_stale");
}

#[actix_web::test]
async fn active_post_is_idempotent_and_persists_only_dashboard_principal() {
    let fixture = ApiFixture::new(true);
    let token = fixture.auth.token().unwrap_or_default().to_string();
    let app = awtest::init_service(
        App::new()
            .wrap(AuthMiddleware::new(Arc::clone(&fixture.auth)))
            .app_data(fixture.data())
            .configure(configure_routes),
    )
    .await;
    let mut action_id = None;
    for _ in 0..2 {
        let response = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri(&fixture.contain_uri())
                .insert_header(("Authorization", format!("Bearer {token}")))
                .set_json(serde_json::json!({
                    "root_pid": fixture.child.id(),
                    "duration_secs": 60,
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = awtest::read_body_json(response).await;
        assert_eq!(value["state"], "policy_active");
        assert_eq!(value["data"]["requested_by"], "dashboard");
        assert_eq!(value["data"]["lifecycle_state"], "active");
        let observed = value["data"]["action_id"].clone();
        assert!(
            action_id
                .as_ref()
                .is_none_or(|expected| expected == &observed)
        );
        action_id = Some(observed);
    }
    assert_eq!(fixture.enforcer.apply_count.load(Ordering::Acquire), 1);
    let stored = fixture
        .store
        .latest_containment_action(fixture.case_id)
        .expect("action query should work")
        .expect("action should persist");
    assert_eq!(stored.requested_by, "dashboard");
    assert!(!stored.requested_by.contains(&token));
    fixture
        .store
        .mark_containment_blocked(stored.binding_id, stored.updated_at_ns.saturating_add(1))
        .expect("block correlation should persist");
    let response = awtest::call_service(
        &app,
        awtest::TestRequest::post()
            .uri(&fixture.contain_uri())
            .insert_header(("Authorization", format!("Bearer {token}")))
            .set_json(serde_json::json!({
                "root_pid": fixture.child.id(),
                "duration_secs": 60,
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = awtest::read_body_json(response).await;
    assert_eq!(value["state"], "contained");
}

#[actix_web::test]
async fn unavailable_and_invalid_requests_use_json_error_envelopes() {
    let fixture = ApiFixture::new(false);
    let data = fixture.data_with_containment(None);
    let app = awtest::init_service(App::new().app_data(data).configure(configure_routes)).await;

    for (request, expected_status) in [
        (
            awtest::TestRequest::get()
                .uri("/api/audit/cases/not-a-uuid/containment-plan")
                .to_request(),
            StatusCode::BAD_REQUEST,
        ),
        (
            awtest::TestRequest::get()
                .uri(&format!(
                    "/api/audit/cases/{}/containment-plan",
                    fixture.case_id
                ))
                .to_request(),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            awtest::TestRequest::post()
                .uri(&fixture.contain_uri())
                .insert_header(("content-type", "application/json"))
                .set_payload("{")
                .to_request(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = awtest::call_service(&app, request).await;
        assert_eq!(response.status(), expected_status);
        let value: Value = awtest::read_body_json(response).await;
        assert!(value["error"]["code"].is_string());
        assert!(value["error"]["retryable"].is_boolean());
    }
}

#[test]
fn trusted_health_candidates_are_filtered_and_deterministic() {
    let mut first = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("first process");
    let mut second = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("second process");
    let mut statuses = vec![
        health_status(first.id(), "z-agent"),
        health_status(second.id(), "a-agent"),
        health_status(1, "protected"),
        health_status(first.id(), ""),
    ];
    statuses.push(statuses[0].clone());
    let mut offline = health_status(second.id(), "offline");
    offline.status = AgentHealthState::Offline;
    statuses.push(offline);

    let candidates = trusted_candidates(statuses);

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].agent_id, "a-agent");
    assert_eq!(candidates[1].agent_id, "z-agent");
    let _ = first.kill();
    let _ = first.wait();
    let _ = second.kill();
    let _ = second.wait();
}

#[test]
fn reconciler_lifecycle_stops_joins_and_allows_restart() {
    let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
    let enforcer: Arc<dyn ContainmentEnforcer> = Arc::new(FakeEnforcer::default());
    let coordinator = ContainmentCoordinator::new(store, enforcer);
    let first = start_reconciler(&coordinator).expect("worker should start");
    assert!(matches!(
        start_reconciler(&coordinator),
        Err(ContainmentError::AlreadyRunning)
    ));
    stop_reconciler(&coordinator, first);
    let second = start_reconciler(&coordinator).expect("worker should restart");
    stop_reconciler(&coordinator, second);
}

fn health_status(pid: u32, agent_name: &str) -> AgentHealthStatus {
    AgentHealthStatus {
        pid,
        agent_name: agent_name.into(),
        category: "test".into(),
        exe_path: "/usr/bin/sleep".into(),
        ports: Vec::new(),
        status: AgentHealthState::Healthy,
        last_check_time: 1,
        latency_ms: None,
        error_message: None,
        restart_cmd: None,
        offline_since: None,
        role: AgentRole::Client,
        parent_pid: None,
        has_crash: false,
    }
}

fn source_binding(binding_id: Uuid) -> Binding {
    Binding {
        request: ApplyPolicy {
            binding_id,
            agent_id: "hermes-test".into(),
            session_id: Some("session-1".into()),
            root_pid: 999_999,
            process_start_time: 42,
            policy_id: "credential-exfiltration".into(),
            policy_revision: "3".into(),
            policy_dsl: policy("notify", "/root/secret.txt"),
        },
        state: BindingState::Enforced,
        message: None,
        domain_id: Some(1),
    }
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

fn policy(action: &str, source: &str) -> String {
    format!(
        "source AGENT = exec \"**\"\nsource CREDENTIAL = file \"{source}\"\nrule agentsight-credential-exfiltration:\n  {action} connect endpoint \"*\" if CREDENTIAL unless target \"trusted.example:443\"\n  because \"credential-derived data reached an untrusted network target\"\n"
    )
}
