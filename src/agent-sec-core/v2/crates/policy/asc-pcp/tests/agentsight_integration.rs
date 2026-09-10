//! Actual Adapter -> Reconciler -> actual Client/Ureq -> loopback HTTP PEP mock.
#![allow(clippy::too_many_lines)]

#[path = "../../../integrations/asc-agentsight-client/tests/common/mod.rs"]
mod common;
#[path = "support/http.rs"]
mod http;

#[path = "support/store.rs"]
mod store;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use store::TestStore;

use asc_agentsight_client::*;
use asc_pap_repository_memory::ProcessLocalPapRepository;
use asc_pcp::*;
use asc_policy_adapter_agentsight::{
    AGENTSIGHT_BINDING_PLAN_FORMAT, AgentSightAdapter, AgentSightBindingPlan,
};
use asc_policy_types::binding::{BindingStatus, BindingView, PreparedBinding};
use asc_policy_types::identifiers::{ResourceId, Revision};
use asc_policy_types::target::TargetBindingPlan;
use common::*;
use http::{Exchange, MockHttp};
use serde_json::Value;

#[derive(Default)]
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base_delay_ms: 100,
        max_delay_ms: 150,
    }
}

fn spec(revision: u32) -> PreparedBinding {
    let mut spec: PreparedBinding = serde_json::from_str(include_str!(
        "../../asc-policy-types/tests/fixtures/prepared-binding.json"
    ))
    .unwrap();
    spec.binding_revision = Revision::new(revision).unwrap();
    spec
}

fn saved(revision: u32, is_update: bool) -> SavedApply {
    let mut golden: AgentSightBindingPlan = serde_json::from_str(include_str!("../../../../fixtures/adapters/agentsight/prevent-file-deletion/agentsight-binding-plan.json")).unwrap();
    golden.source.binding_revision = Revision::new(revision).unwrap();
    SavedApply {
        revision: Revision::new(revision).unwrap(),
        is_update,
        prepared: prepared(revision),
        plan: TargetBindingPlan {
            format: AGENTSIGHT_BINDING_PLAN_FORMAT.into(),
            content: serde_json::to_vec(&golden).unwrap(),
        },
    }
}

fn initial() -> ReconcileRecord {
    ReconcileRecord {
        binding: BindingView {
            spec: spec(7),
            status: BindingStatus::PendingApply,
        },
        runtime: RuntimeState::default(),
        deployments: vec![],
    }
}

fn deployment(revision: u32, presence: Presence, confirmed: Option<Presence>) -> Deployment {
    Deployment {
        target: prepared(revision).target,
        revision: Revision::new(revision).unwrap(),
        presence,
        last_confirmed: confirmed,
    }
}

fn ready(revision: u32, attempts: u32, is_update: bool) -> ReconcileRecord {
    ReconcileRecord {
        binding: BindingView {
            spec: spec(revision),
            status: BindingStatus::Ready,
        },
        runtime: RuntimeState {
            attempts_started: attempts,
            next_attempt_at: None,
            retry_policy: Some(policy()),
            last_error: None,
            prepared: Some(saved(revision, is_update)),
        },
        deployments: vec![deployment(
            revision,
            Presence::Present,
            Some(Presence::Present),
        )],
    }
}

fn request(
    method: AgentSightHttpMethod,
    path: &str,
    body: Option<Vec<u8>>,
) -> AgentSightHttpRequest {
    AgentSightHttpRequest {
        method,
        path: path.into(),
        body,
    }
}
fn health() -> Exchange {
    Exchange {
        request: request(AgentSightHttpMethod::Get, "/enforcement/health", None),
        response: Some(response(200, HEALTH)),
    }
}
fn post(revision: u32, response: AgentSightHttpResponse) -> Exchange {
    let payload: Value = serde_json::from_slice(&prepared(revision).content).unwrap();
    Exchange {
        request: request(
            AgentSightHttpMethod::Post,
            "/enforcement/bindings",
            Some(serde_json::from_value(payload["request"].clone()).unwrap()),
        ),
        response: Some(response),
    }
}
fn delete(revision: u32, response: Option<AgentSightHttpResponse>) -> Exchange {
    Exchange {
        request: request(
            AgentSightHttpMethod::Delete,
            &path(if revision == 7 { ID7 } else { ID8 }),
            None,
        ),
        response,
    }
}

fn core(
    repo: Arc<dyn BindingStateRepository>,
    clock: Arc<TestClock>,
    url: &str,
) -> BindingReconciler {
    let transport = UreqAgentSightTransport::new(url, "test-local-only").unwrap();
    let client: Arc<dyn TargetDeploymentClient> = Arc::new(AgentSightClient::with_dependencies(
        transport,
        Identity::default(),
    ));
    let adapter = |binding: &PreparedBinding| AgentSightAdapter.translate(binding);
    BindingReconciler::new(
        repo,
        Arc::new(adapter),
        BTreeMap::from([(DEFAULT_AGENTSIGHT_ROUTE.into(), client)]),
        DEFAULT_AGENTSIGHT_ROUTE.into(),
        clock,
        policy(),
        Arc::new(ReconcileExecution::default()),
    )
    .unwrap()
}

fn inspect_registered(repo: &ProcessLocalPapRepository, request: &AgentSightHttpRequest) {
    if request.method == AgentSightHttpMethod::Get {
        return;
    }
    let record = repo.read(&spec(7).binding_id).unwrap().unwrap();
    if request.method == AgentSightHttpMethod::Post {
        let prepared = &record.runtime.prepared.as_ref().unwrap().prepared;
        let payload: Value = serde_json::from_slice(&prepared.content).unwrap();
        assert_eq!(
            request.body.as_ref().unwrap(),
            &serde_json::from_value::<Vec<u8>>(payload["request"].clone()).unwrap()
        );
        assert!(
            record
                .deployments
                .iter()
                .any(|d| d.target == prepared.target && d.presence == Presence::Unknown)
        );
    } else {
        assert!(
            record
                .deployments
                .iter()
                .any(|d| path(&d.target.id) == request.path && d.presence == Presence::Unknown)
        );
    }
}

#[test]
fn real_http_apply_then_uncertain_delete_and_retry() {
    let repo = Arc::new(ProcessLocalPapRepository::with_binding_states(vec![initial()]).unwrap());
    let inspect = repo.clone();
    let server = MockHttp::start(
        vec![
            health(),
            post(7, applied(7)),
            delete(7, None),
            delete(7, Some(response(204, b""))),
        ],
        move |req| inspect_registered(&inspect, req),
    );
    let clock = Arc::new(TestClock::default());
    let worker = core(repo.clone(), clock.clone(), &server.base_url);
    let id = spec(7).binding_id;
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Completed);
    assert_eq!(repo.read(&id).unwrap(), Some(ready(7, 1, false)));

    let mut desired = ready(7, 1, false).binding;
    desired.status = BindingStatus::PendingDelete;
    assert!(
        repo.compare_exchange_reconcile_intent(
            &ExpectedBinding::from_binding(&ready(7, 1, false).binding),
            &desired
        )
        .unwrap()
    );
    assert_eq!(
        worker.reconcile(&id).unwrap(),
        Disposition::RetryAt { at: 100 }
    );
    let failed = ReconcileRecord {
        binding: desired.clone(),
        runtime: RuntimeState {
            attempts_started: 1,
            next_attempt_at: Some(100),
            retry_policy: Some(policy()),
            last_error: Some(Failure::new(
                FailureKind::Retryable,
                "AGENTSIGHT_TRANSPORT_UNAVAILABLE",
            )),
            prepared: None,
        },
        deployments: vec![deployment(7, Presence::Unknown, Some(Presence::Present))],
    };
    assert_eq!(repo.read(&id).unwrap(), Some(failed));
    clock.0.store(99, Ordering::SeqCst);
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Skipped);
    clock.0.store(100, Ordering::SeqCst);
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Completed);
    assert_eq!(repo.read(&id).unwrap(), None);
    server.finish(4);
}

#[test]
fn real_http_partial_update_retry_cleans_old_once_and_reuses_new_request() {
    let repo =
        Arc::new(ProcessLocalPapRepository::with_binding_states(vec![ready(7, 1, false)]).unwrap());
    let inspect = repo.clone();
    let server = MockHttp::start(
        vec![
            health(),
            delete(7, Some(response(204, b""))),
            post(8, remote_error(503, "enforcer_unavailable", true)),
            health(),
            post(8, applied(8)),
        ],
        move |req| inspect_registered(&inspect, req),
    );
    let clock = Arc::new(TestClock::default());
    let worker = core(repo.clone(), clock.clone(), &server.base_url);
    let id = spec(7).binding_id;
    let desired = BindingView {
        spec: spec(8),
        status: BindingStatus::PendingApply,
    };
    assert!(
        repo.compare_exchange_reconcile_intent(
            &ExpectedBinding::from_binding(&ready(7, 1, false).binding),
            &desired
        )
        .unwrap()
    );
    assert_eq!(
        worker.reconcile(&id).unwrap(),
        Disposition::RetryAt { at: 100 }
    );
    assert_eq!(
        repo.read(&id).unwrap(),
        Some(ReconcileRecord {
            binding: desired,
            runtime: RuntimeState {
                attempts_started: 1,
                next_attempt_at: Some(100),
                retry_policy: Some(policy()),
                last_error: Some(Failure::new(
                    FailureKind::Retryable,
                    "AGENTSIGHT_ENFORCER_UNAVAILABLE"
                )),
                prepared: Some(saved(8, true))
            },
            deployments: vec![deployment(8, Presence::Unknown, None)]
        })
    );
    clock.0.store(100, Ordering::SeqCst);
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Completed);
    assert_eq!(repo.read(&id).unwrap(), Some(ready(8, 2, true)));
    server.finish(5);
}

#[test]
fn same_revision_delete_during_real_http_apply_preserves_target_for_cleanup() {
    let repo = Arc::new(ProcessLocalPapRepository::with_binding_states(vec![initial()]).unwrap());
    let inspect = repo.clone();
    let server = MockHttp::start(
        vec![
            health(),
            post(7, applied(7)),
            delete(7, Some(response(204, b""))),
        ],
        move |req| {
            inspect_registered(&inspect, req);
            if req.method == AgentSightHttpMethod::Post {
                let record = inspect.read(&spec(7).binding_id).unwrap().unwrap();
                let mut desired = record.binding.clone();
                desired.status = BindingStatus::PendingDelete;
                assert!(
                    inspect
                        .compare_exchange_reconcile_intent(
                            &ExpectedBinding::from_binding(&record.binding),
                            &desired
                        )
                        .unwrap()
                );
            }
        },
    );
    let worker = core(
        repo.clone(),
        Arc::new(TestClock::default()),
        &server.base_url,
    );
    let id = spec(7).binding_id;
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Superseded);
    assert_eq!(
        repo.read(&id).unwrap(),
        Some(ReconcileRecord {
            binding: BindingView {
                spec: spec(7),
                status: BindingStatus::PendingDelete
            },
            runtime: RuntimeState::default(),
            deployments: vec![deployment(7, Presence::Present, Some(Presence::Present))]
        })
    );
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Completed);
    assert_eq!(repo.read(&id).unwrap(), None);
    server.finish(3);
}

struct FailFinishOnce {
    repo: Arc<ProcessLocalPapRepository>,
    fail: AtomicBool,
}
impl BindingStateRepository for FailFinishOnce {
    fn get_binding_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<BindingStateSnapshot>, StoreError> {
        self.repo.get_binding_state(id)
    }
    fn compare_exchange_binding_state(
        &self,
        expected: &BindingStateSnapshot,
        write: &BindingStateWrite,
    ) -> Result<WriteResult, StoreError> {
        if store::write_phase(expected, write) == "finish"
            && self.fail.swap(false, Ordering::SeqCst)
        {
            return Err(StoreError::Unavailable);
        }
        self.repo.compare_exchange_binding_state(expected, write)
    }
}

#[test]
fn completed_http_is_not_repeated_after_result_storage_failure() {
    let repo = Arc::new(ProcessLocalPapRepository::with_binding_states(vec![initial()]).unwrap());
    let inspect = repo.clone();
    let server = MockHttp::start(vec![health(), post(7, applied(7))], move |req| {
        inspect_registered(&inspect, req);
    });
    let wrapped = Arc::new(FailFinishOnce {
        repo: repo.clone(),
        fail: AtomicBool::new(true),
    });
    let worker = core(wrapped, Arc::new(TestClock::default()), &server.base_url);
    let id = spec(7).binding_id;
    assert_eq!(worker.reconcile(&id), Err(StoreError::Unavailable));
    let mut expected = ready(7, 1, false);
    expected.binding.status = BindingStatus::Applying;
    expected.deployments = vec![deployment(7, Presence::Unknown, None)];
    assert_eq!(repo.read(&id).unwrap(), Some(expected));
    // The HTTP mock has consumed its script; any replay would fail the case.
    server.finish(2);
    assert_eq!(worker.reconcile(&id).unwrap(), Disposition::Completed);
    assert_eq!(repo.read(&id).unwrap(), Some(ready(7, 1, false)));
}
