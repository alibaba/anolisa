use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use asc_agentsight_client::{
    AgentSightClient, AgentSightClientErrorKind, AgentSightDeploymentState, AgentSightHttpMethod,
    AgentSightHttpRequest, AgentSightHttpResponse, AgentSightTransport, AgentSightTransportError,
    ProcessIdentityError, ProcessIdentityResolver,
};
use asc_policy_types::identifiers::{ResourceId, Revision};
use asc_policy_types::target::TargetBindingPlan;

const HEALTH_RESPONSE: &[u8] =
    include_bytes!("../../../../fixtures/clients/agentsight/file-deletion/health.response.json");
const APPLY_REQUEST: &[u8] =
    include_bytes!("../../../../fixtures/clients/agentsight/file-deletion/apply.request.json");
const APPLY_RESPONSE: &[u8] =
    include_bytes!("../../../../fixtures/clients/agentsight/file-deletion/apply.response.json");
const RETRYABLE_ERROR_RESPONSE: &[u8] = include_bytes!(
    "../../../../fixtures/clients/agentsight/file-deletion/retryable-error.response.json"
);
const BINDING_PLAN: &[u8] =
    include_bytes!("../../../../fixtures/clients/agentsight/file-deletion/deployment-plan.json");

type TransportResult = Result<AgentSightHttpResponse, AgentSightTransportError>;

#[derive(Clone)]
struct FakeTransport {
    state: Arc<Mutex<FakeTransportState>>,
}

struct FakeTransportState {
    responses: VecDeque<TransportResult>,
    requests: Vec<AgentSightHttpRequest>,
}

impl FakeTransport {
    fn new(responses: impl IntoIterator<Item = TransportResult>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeTransportState {
                responses: responses.into_iter().collect(),
                requests: Vec::new(),
            })),
        }
    }

    fn requests(&self) -> Vec<AgentSightHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }
}

impl AgentSightTransport for FakeTransport {
    fn send(
        &self,
        request: &AgentSightHttpRequest,
    ) -> Result<AgentSightHttpResponse, AgentSightTransportError> {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        state
            .responses
            .pop_front()
            .ok_or(AgentSightTransportError::InvalidRequest)?
    }
}

#[derive(Clone, Copy)]
struct FixedProcessIdentity(Result<u64, ProcessIdentityError>);

impl ProcessIdentityResolver for FixedProcessIdentity {
    fn process_start_time(&self, _pid: i32) -> Result<u64, ProcessIdentityError> {
        self.0
    }
}

fn response(status: u16, body: &[u8]) -> AgentSightHttpResponse {
    AgentSightHttpResponse {
        status,
        body: body.to_vec(),
    }
}

fn binding_id() -> ResourceId {
    resource_id("10000000-0000-4000-8000-000000000001")
}

fn binding_revision() -> Revision {
    Revision::new(7).unwrap()
}

fn binding_plan(content: Vec<u8>) -> TargetBindingPlan {
    TargetBindingPlan {
        format: "agentsight.actplane.binding.v1".to_owned(),
        content,
    }
}

fn resource_id(value: &str) -> ResourceId {
    ResourceId::new(value).unwrap()
}

#[test]
fn apply_checks_health_and_reuses_the_derived_idempotency_identity() {
    let transport = FakeTransport::new([
        Ok(response(200, HEALTH_RESPONSE)),
        Ok(response(200, APPLY_RESPONSE)),
        Ok(response(200, HEALTH_RESPONSE)),
        Ok(response(200, APPLY_RESPONSE)),
    ]);
    let client =
        AgentSightClient::with_dependencies(transport.clone(), FixedProcessIdentity(Ok(987_654)));
    let plan = binding_plan(BINDING_PLAN.to_vec());

    let first = client.apply(&plan).unwrap();
    let second = client.apply(&plan).unwrap();

    assert_eq!(first, second);
    assert_eq!(first, AgentSightDeploymentState::Present);
    let requests = transport.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].method, AgentSightHttpMethod::Get);
    assert_eq!(requests[0].path, "/enforcement/health");
    assert_eq!(requests[1].method, AgentSightHttpMethod::Post);
    assert_eq!(requests[1].path, "/enforcement/bindings");
    assert_eq!(requests[1].body, requests[3].body);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(requests[1].body.as_deref().unwrap()).unwrap(),
        serde_json::from_slice::<serde_json::Value>(APPLY_REQUEST).unwrap()
    );
}

#[test]
fn delete_treats_no_content_and_binding_not_found_as_absent() {
    let not_found = br#"{
        "error": {
            "code": "binding_not_found",
            "message": "sensitive remote detail",
            "retryable": false
        }
    }"#;
    let transport = FakeTransport::new([Ok(response(204, b"")), Ok(response(404, not_found))]);
    let client =
        AgentSightClient::with_dependencies(transport.clone(), FixedProcessIdentity(Ok(987_654)));
    let first = client.delete(&binding_id(), binding_revision()).unwrap();
    let second = client.delete(&binding_id(), binding_revision()).unwrap();

    assert_eq!(first, AgentSightDeploymentState::Absent);
    assert_eq!(second, AgentSightDeploymentState::Absent);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| {
        request.method == AgentSightHttpMethod::Delete
            && request.path == "/enforcement/bindings/d525d62c-2a3d-570b-9c25-d29c336c1d87"
            && request.body.is_none()
    }));
}

#[test]
fn different_binding_revisions_derive_different_target_ids() {
    let transport = FakeTransport::new([Ok(response(204, b"")), Ok(response(204, b""))]);
    let client =
        AgentSightClient::with_dependencies(transport.clone(), FixedProcessIdentity(Ok(987_654)));

    client
        .delete(&binding_id(), Revision::new(7).unwrap())
        .unwrap();
    client
        .delete(&binding_id(), Revision::new(8).unwrap())
        .unwrap();

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].path,
        "/enforcement/bindings/d525d62c-2a3d-570b-9c25-d29c336c1d87"
    );
    assert_eq!(
        requests[1].path,
        "/enforcement/bindings/57f6ffc6-7960-5e22-8aef-404c2178c266"
    );
}

#[test]
fn remote_errors_are_classified_without_exposing_target_messages() {
    let conflict = br#"{
        "error": {
            "code": "policy_revision_conflict",
            "message": "another sensitive target detail",
            "retryable": false
        }
    }"#;
    let transport = FakeTransport::new([
        Ok(response(200, HEALTH_RESPONSE)),
        Ok(response(503, RETRYABLE_ERROR_RESPONSE)),
        Ok(response(200, HEALTH_RESPONSE)),
        Ok(response(409, conflict)),
    ]);
    let client = AgentSightClient::with_dependencies(transport, FixedProcessIdentity(Ok(987_654)));
    let plan = binding_plan(BINDING_PLAN.to_vec());

    let retryable = client.apply(&plan).unwrap_err();
    assert_eq!(retryable.kind, AgentSightClientErrorKind::Retryable);
    assert_eq!(retryable.code, "AGENTSIGHT_ENFORCER_UNAVAILABLE");
    assert!(!format!("{retryable:?}").contains("sensitive"));

    let rejected = client.apply(&plan).unwrap_err();
    assert_eq!(rejected.kind, AgentSightClientErrorKind::Rejected);
    assert_eq!(rejected.code, "AGENTSIGHT_POLICY_REVISION_CONFLICT");
    assert!(!format!("{rejected:?}").contains("sensitive"));
}

#[test]
fn local_transport_and_process_failures_keep_stable_retry_categories() {
    let transport = FakeTransport::new([Err(AgentSightTransportError::Unavailable)]);
    let client = AgentSightClient::with_dependencies(transport, FixedProcessIdentity(Ok(987_654)));
    let unavailable = client
        .apply(&binding_plan(BINDING_PLAN.to_vec()))
        .unwrap_err();
    assert_eq!(unavailable.kind, AgentSightClientErrorKind::Retryable);
    assert_eq!(unavailable.code, "AGENTSIGHT_TRANSPORT_UNAVAILABLE");

    let transport = FakeTransport::new([Ok(response(200, HEALTH_RESPONSE))]);
    let client = AgentSightClient::with_dependencies(
        transport,
        FixedProcessIdentity(Err(ProcessIdentityError::Unavailable)),
    );
    let unavailable = client
        .apply(&binding_plan(BINDING_PLAN.to_vec()))
        .unwrap_err();
    assert_eq!(unavailable.kind, AgentSightClientErrorKind::Retryable);
    assert_eq!(unavailable.code, "AGENTSIGHT_PROCESS_IDENTITY_UNAVAILABLE");
}

#[test]
fn apply_rejects_an_unsupported_plan_format_before_target_io() {
    let transport = FakeTransport::new([]);
    let client =
        AgentSightClient::with_dependencies(transport.clone(), FixedProcessIdentity(Ok(987_654)));
    let mut plan = binding_plan(BINDING_PLAN.to_vec());
    plan.format = "agentsight.actplane.binding.v2".to_owned();

    let error = client.apply(&plan).unwrap_err();

    assert_eq!(error.kind, AgentSightClientErrorKind::Rejected);
    assert_eq!(error.code, "AGENTSIGHT_UNSUPPORTED_PLAN_FORMAT");
    assert!(transport.requests().is_empty());
}

#[test]
fn apply_fails_closed_when_file_delete_guard_is_unavailable() {
    let health = br#"{
        "ready": true,
        "backend": "actplane",
        "capabilities": { "file_delete_guard": false }
    }"#;
    let transport = FakeTransport::new([Ok(response(200, health))]);
    let client =
        AgentSightClient::with_dependencies(transport.clone(), FixedProcessIdentity(Ok(987_654)));

    let error = client
        .apply(&binding_plan(BINDING_PLAN.to_vec()))
        .unwrap_err();

    assert_eq!(error.kind, AgentSightClientErrorKind::Rejected);
    assert_eq!(error.code, "AGENTSIGHT_FILE_DELETE_GUARD_UNAVAILABLE");
    assert_eq!(transport.requests().len(), 1);
}
