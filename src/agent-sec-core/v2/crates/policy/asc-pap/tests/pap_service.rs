use std::collections::BTreeMap;
use std::sync::{Arc, Barrier, Mutex, MutexGuard};

use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{
    Page, PapError, PapRepository, PapService, PolicyCompiler, PolicyRevisionState,
    ScopeRevisionState,
};
use asc_policy_types::authoring::{PolicyTemplate, TemplateEnvelope};
use asc_policy_types::binding::{BindingStatus, BindingView, PreparedBinding};
use asc_policy_types::error::ValidationError;
use asc_policy_types::identifiers::PolicyId;
use asc_policy_types::policy::{PolicyEnvelope, PreparedPolicy};
use asc_policy_types::scope::{PreparedScope, ScopeSelector};

const COMPLETE_BINDING: &str =
    include_str!("../../asc-policy-types/tests/fixtures/prepared-binding.json");

#[derive(Default)]
struct FakeState {
    policy_heads: BTreeMap<String, u32>,
    policies: BTreeMap<String, PreparedPolicy>,
    scope_heads: BTreeMap<String, u32>,
    scopes: BTreeMap<String, PreparedScope>,
    bindings: BTreeMap<String, BindingView>,
}

#[derive(Default)]
struct FakeRepository {
    state: Mutex<FakeState>,
    scope_read_gate: Mutex<ScopeReadGate>,
}

#[derive(Default)]
struct ScopeReadGate {
    barrier: Option<Arc<Barrier>>,
    remaining: usize,
}

impl FakeRepository {
    fn lock(&self) -> Result<MutexGuard<'_, FakeState>, PapError> {
        self.state.lock().map_err(|_| PapError::Persistence)
    }

    fn synchronize_next_scope_reads(&self, participants: usize) {
        let mut gate = self.scope_read_gate.lock().unwrap();
        assert!(gate.barrier.is_none());
        gate.barrier = Some(Arc::new(Barrier::new(participants)));
        gate.remaining = participants;
    }

    fn take_scope_read_barrier(&self) -> Result<Option<Arc<Barrier>>, PapError> {
        let mut gate = self
            .scope_read_gate
            .lock()
            .map_err(|_| PapError::Persistence)?;
        let Some(barrier) = gate.barrier.clone() else {
            return Ok(None);
        };
        gate.remaining -= 1;
        if gate.remaining == 0 {
            gate.barrier = None;
        }
        Ok(Some(barrier))
    }
}

impl PapRepository for FakeRepository {
    fn put_policy(&self, policy: &PreparedPolicy) -> Result<PreparedPolicy, PapError> {
        let mut state = self.lock()?;
        let id = policy.policy_id.as_str().to_owned();
        let revision = policy.revision.get();
        if let Some(existing) = state.policies.get(&id)
            && existing.revision.get() == revision
        {
            return if existing == policy {
                Ok(existing.clone())
            } else {
                Err(PapError::Conflict)
            };
        }
        if next_raw_revision(state.policy_heads.get(&id).copied()) != Some(revision) {
            return Err(PapError::Conflict);
        }
        state.policies.insert(id.clone(), policy.clone());
        state.policy_heads.insert(id, revision);
        Ok(policy.clone())
    }

    fn get_policy_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<PolicyRevisionState>, PapError> {
        let state = self.lock()?;
        let Some(last) = state.policy_heads.get(id.as_str()).copied() else {
            return Ok(None);
        };
        let current = state.policies.get(id.as_str()).cloned();
        Ok(Some(PolicyRevisionState {
            last_allocated_revision: Revision::new(last).map_err(|_| PapError::Persistence)?,
            current,
        }))
    }

    fn get_policy(&self, id: &ResourceId, revision: Revision) -> Result<PreparedPolicy, PapError> {
        self.lock()?
            .policies
            .get(id.as_str())
            .filter(|policy| policy.revision == revision)
            .cloned()
            .ok_or(PapError::NotFound)
    }

    fn list_policies(&self, limit: u32, offset: u32) -> Result<Page<PreparedPolicy>, PapError> {
        let items = self.lock()?.policies.values().cloned().collect();
        Ok(page(items, limit, offset))
    }

    fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PapError> {
        let mut state = self.lock()?;
        let id = id.as_str();
        if state
            .policies
            .get(id)
            .is_none_or(|policy| policy.revision != revision)
        {
            return Err(PapError::NotFound);
        }
        state.policies.remove(id).ok_or(PapError::Persistence)
    }

    fn put_scope(&self, scope: &PreparedScope) -> Result<PreparedScope, PapError> {
        let mut state = self.lock()?;
        let id = scope.scope_id.as_str().to_owned();
        let revision = scope.revision.get();
        if let Some(existing) = state.scopes.get(&id)
            && existing.revision.get() == revision
        {
            return if existing == scope {
                Ok(existing.clone())
            } else {
                Err(PapError::Conflict)
            };
        }
        if next_raw_revision(state.scope_heads.get(&id).copied()) != Some(revision) {
            return Err(PapError::Conflict);
        }
        state.scopes.insert(id.clone(), scope.clone());
        state.scope_heads.insert(id, revision);
        Ok(scope.clone())
    }

    fn get_scope_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ScopeRevisionState>, PapError> {
        let state = self.lock()?;
        let result = state
            .scope_heads
            .get(id.as_str())
            .copied()
            .map(|last| {
                Ok(ScopeRevisionState {
                    last_allocated_revision: Revision::new(last)
                        .map_err(|_| PapError::Persistence)?,
                    current: state.scopes.get(id.as_str()).cloned(),
                })
            })
            .transpose()?;
        drop(state);
        if let Some(barrier) = self.take_scope_read_barrier()? {
            barrier.wait();
        }
        Ok(result)
    }

    fn get_scope(&self, id: &ResourceId, revision: Revision) -> Result<PreparedScope, PapError> {
        self.lock()?
            .scopes
            .get(id.as_str())
            .filter(|scope| scope.revision == revision)
            .cloned()
            .ok_or(PapError::NotFound)
    }

    fn list_scopes(&self, limit: u32, offset: u32) -> Result<Page<PreparedScope>, PapError> {
        let items = self.lock()?.scopes.values().cloned().collect();
        Ok(page(items, limit, offset))
    }

    fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PapError> {
        let mut state = self.lock()?;
        let id = id.as_str();
        if state
            .scopes
            .get(id)
            .is_none_or(|scope| scope.revision != revision)
        {
            return Err(PapError::NotFound);
        }
        state.scopes.remove(id).ok_or(PapError::Persistence)
    }

    fn update_binding(&self, binding: &BindingView) -> Result<BindingView, PapError> {
        let mut state = self.lock()?;
        if !matches!(
            binding.status,
            BindingStatus::PendingApply | BindingStatus::PendingDelete
        ) {
            return Err(PapError::Conflict);
        }

        let id = binding.spec.binding_id.as_str().to_owned();
        let revision = binding.spec.binding_revision.get();
        if let Some(current) = state.bindings.get(&id) {
            if current == binding {
                return Ok(current.clone());
            }
            if current.status.is_reconciling() {
                return Err(PapError::OperationInProgress);
            }
            if next_raw_revision(Some(current.spec.binding_revision.get())) != Some(revision) {
                return Err(PapError::Conflict);
            }
        } else if revision != 1 || binding.status != BindingStatus::PendingApply {
            return Err(PapError::Conflict);
        }
        state.bindings.insert(id, binding.clone());
        Ok(binding.clone())
    }

    fn update_binding_status(
        &self,
        id: &ResourceId,
        binding_revision: Revision,
        expected_status: BindingStatus,
        next_status: BindingStatus,
    ) -> Result<BindingStatus, PapError> {
        let mut state = self.lock()?;
        let binding = state
            .bindings
            .get_mut(id.as_str())
            .ok_or(PapError::NotFound)?;
        if binding.spec.binding_revision != binding_revision {
            return Err(PapError::Conflict);
        }
        if binding.status != expected_status {
            return Err(PapError::Conflict);
        }
        expected_status
            .validate_successor(next_status)
            .map_err(|_| PapError::Conflict)?;
        binding.status = next_status;
        Ok(next_status)
    }

    fn get_binding(&self, id: &ResourceId) -> Result<BindingView, PapError> {
        self.lock()?
            .bindings
            .get(id.as_str())
            .cloned()
            .ok_or(PapError::NotFound)
    }

    fn list_bindings(&self, limit: u32, offset: u32) -> Result<Page<BindingView>, PapError> {
        let items = self.lock()?.bindings.values().cloned().collect();
        Ok(page(items, limit, offset))
    }
}

struct FixtureCompiler {
    mismatch_identity: bool,
}

impl PolicyCompiler for FixtureCompiler {
    fn lower(&self, template: &TemplateEnvelope) -> Result<PolicyEnvelope, ValidationError> {
        let fixture: PreparedBinding = serde_json::from_str(COMPLETE_BINDING)
            .map_err(|error| ValidationError::new("fixture", error.to_string()))?;
        let mut policy = fixture.policy.canonical_policy;
        policy.policy_id = if self.mismatch_identity {
            PolicyId::new("compiler-mismatch")
                .map_err(|error| ValidationError::new("policyId", error))?
        } else {
            template.policy_id.clone()
        };
        policy.revision = template.revision;
        Ok(policy)
    }
}

type Service = PapService<FakeRepository, FixtureCompiler>;

fn service() -> (Service, Arc<FakeRepository>) {
    let repository = Arc::new(FakeRepository::default());
    let compiler = Arc::new(FixtureCompiler {
        mismatch_identity: false,
    });
    (
        PapService::new(Arc::clone(&repository), compiler),
        repository,
    )
}

fn policy_template(path: &str) -> PolicyTemplate {
    PolicyTemplate::PreventFileDeletion {
        files: vec![path.to_owned()],
    }
}

#[test]
fn policy_crud_keeps_only_the_current_record_and_never_reuses_revisions() {
    let (pap, repository) = service();
    let first = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    assert_eq!(first.revision.get(), 1);
    assert_eq!(
        pap.update_policy(
            &first.policy_id,
            "protect files",
            &policy_template("/workspace/a")
        )
        .unwrap(),
        first
    );

    let second = pap
        .update_policy(
            &first.policy_id,
            "protect more files",
            &policy_template("/workspace/b"),
        )
        .unwrap();
    assert_eq!(second.revision.get(), 2);
    assert_eq!(
        pap.list_policies(100, 0).unwrap().items,
        vec![second.clone()]
    );
    assert_eq!(
        pap.get_policy(&first.policy_id, first.revision),
        Err(PapError::NotFound)
    );
    assert_eq!(
        pap.delete_policy_revision(&first.policy_id, second.revision)
            .unwrap(),
        second
    );
    assert_eq!(pap.list_policies(100, 0).unwrap().total, 0);

    let third = pap
        .update_policy(
            &first.policy_id,
            "protect newest files",
            &policy_template("/workspace/c"),
        )
        .unwrap();
    assert_eq!(third.revision.get(), 3);
    assert_eq!(
        pap.get_policy(&first.policy_id, second.revision),
        Err(PapError::NotFound)
    );
    assert_eq!(pap.list_policies(100, 0).unwrap().items, vec![third]);
    let state = repository.lock().unwrap();
    assert_eq!(state.policies.len(), 1);
    assert_eq!(state.policy_heads[&first.policy_id.to_string()], 3);
    drop(state);

    let missing = ResourceId::new("missing-policy").unwrap();
    assert_eq!(
        pap.update_policy(&missing, "missing", &policy_template("/workspace/missing")),
        Err(PapError::NotFound)
    );
}

#[test]
fn scope_crud_keeps_only_the_current_record_and_preserves_revision_heads() {
    let (pap, repository) = service();
    let first = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    assert_eq!(first.revision.get(), 1);
    assert_eq!(
        pap.update_scope(&first.scope_id, &ScopeSelector::Pid { pid: 4242 })
            .unwrap(),
        first
    );

    let second = pap
        .update_scope(&first.scope_id, &ScopeSelector::CgroupId { cgroup_id: 99 })
        .unwrap();
    assert_eq!(second.revision.get(), 2);
    assert_eq!(pap.list_scopes(100, 0).unwrap().items, vec![second.clone()]);
    assert_eq!(
        pap.get_scope(&first.scope_id, first.revision),
        Err(PapError::NotFound)
    );
    pap.delete_scope_revision(&first.scope_id, second.revision)
        .unwrap();
    assert_eq!(pap.list_scopes(100, 0).unwrap().total, 0);
    let third = pap
        .update_scope(&first.scope_id, &ScopeSelector::Pid { pid: 7 })
        .unwrap();
    assert_eq!(third.revision.get(), 3);
    assert_eq!(pap.list_scopes(100, 0).unwrap().items, vec![third]);
    let state = repository.lock().unwrap();
    assert_eq!(state.scopes.len(), 1);
    assert_eq!(state.scope_heads[&first.scope_id.to_string()], 3);
    drop(state);

    let legacy = ScopeSelector::LegacyExecutionDomain {
        execution_domain_id: ResourceId::new("legacy-domain").unwrap(),
    };
    assert!(matches!(
        pap.create_scope(&legacy),
        Err(PapError::InvalidScope(_))
    ));
    let missing = ResourceId::new("missing-scope").unwrap();
    assert_eq!(
        pap.update_scope(&missing, &ScopeSelector::Pid { pid: 9 }),
        Err(PapError::NotFound)
    );
}

#[test]
fn concurrent_scope_updates_retry_after_repository_cas_conflict() {
    let (pap, repository) = service();
    let first = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    repository.synchronize_next_scope_reads(2);

    let left = {
        let pap = pap.clone();
        let scope_id = first.scope_id.clone();
        std::thread::spawn(move || pap.update_scope(&scope_id, &ScopeSelector::Pid { pid: 7 }))
    };
    let right = {
        let pap = pap.clone();
        let scope_id = first.scope_id.clone();
        std::thread::spawn(move || {
            pap.update_scope(&scope_id, &ScopeSelector::CgroupId { cgroup_id: 99 })
        })
    };

    let mut updates = [
        left.join().unwrap().unwrap(),
        right.join().unwrap().unwrap(),
    ];
    updates.sort_by_key(|scope| scope.revision);

    assert_eq!(updates[0].revision.get(), 2);
    assert_eq!(updates[1].revision.get(), 3);
    assert_ne!(updates[0].selector, updates[1].selector);
    assert_eq!(
        pap.get_scope(&first.scope_id, updates[0].revision),
        Err(PapError::NotFound)
    );
    assert_eq!(
        pap.get_scope(&first.scope_id, updates[1].revision).unwrap(),
        updates[1]
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end lifecycle scenario keeps transition assertions reviewable"
)]
fn binding_requests_replace_one_current_record_and_fence_running_work() {
    let (pap, repository) = service();
    let policy_v1 = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    let scope = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    let binding_v1 = pap
        .create_binding(
            &policy_v1.policy_id,
            policy_v1.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    assert_eq!(binding_v1.spec.binding_revision.get(), 1);
    assert_eq!(binding_v1.status, BindingStatus::PendingApply);
    assert_eq!(
        pap.update_binding(
            &binding_v1.spec.binding_id,
            &policy_v1.policy_id,
            policy_v1.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap(),
        binding_v1
    );

    let policy_v2 = pap
        .update_policy(
            &policy_v1.policy_id,
            "protect more files",
            &policy_template("/workspace/b"),
        )
        .unwrap();
    let binding_v2 = pap
        .update_binding(
            &binding_v1.spec.binding_id,
            &policy_v2.policy_id,
            policy_v2.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    assert_eq!(binding_v2.spec.binding_revision.get(), 2);
    assert_eq!(binding_v2.spec.policy, policy_v2);
    assert_eq!(binding_v2.status, BindingStatus::PendingApply);
    {
        let state = repository.lock().unwrap();
        assert_eq!(state.bindings.len(), 1);
        assert_eq!(
            state.bindings[&binding_v1.spec.binding_id.to_string()],
            binding_v2
        );
    }

    let pending_delete = pap.delete_binding(&binding_v1.spec.binding_id).unwrap();
    assert_eq!(pending_delete.spec.binding_revision.get(), 3);
    assert_eq!(pending_delete.status, BindingStatus::PendingDelete);
    let pending_delete_wire = serde_json::to_value(&pending_delete).unwrap();
    assert!(pending_delete_wire.get("lifecycle").is_none());
    assert_eq!(pending_delete_wire["status"], "PENDING_DELETE");
    assert!(pending_delete_wire["spec"].get("desiredState").is_none());
    assert_eq!(
        serde_json::from_value::<BindingView>(pending_delete_wire).unwrap(),
        pending_delete
    );
    assert_eq!(
        pap.delete_binding(&binding_v1.spec.binding_id).unwrap(),
        pending_delete
    );
    assert_eq!(
        pap.get_binding(&binding_v1.spec.binding_id).unwrap(),
        pending_delete
    );

    let reactivated = pap
        .update_binding(
            &binding_v1.spec.binding_id,
            &policy_v2.policy_id,
            policy_v2.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    assert_eq!(reactivated.spec.binding_revision.get(), 4);
    assert_eq!(reactivated.status, BindingStatus::PendingApply);

    let applying = reactivated.status.start_reconcile().unwrap();
    repository
        .update_binding_status(
            &reactivated.spec.binding_id,
            reactivated.spec.binding_revision,
            reactivated.status,
            applying,
        )
        .unwrap();
    assert_eq!(
        pap.update_binding(
            &binding_v1.spec.binding_id,
            &policy_v2.policy_id,
            policy_v2.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap()
        .status,
        BindingStatus::Applying,
        "an identical update is idempotent while Apply is running"
    );

    let policy_v3 = pap
        .update_policy(
            &policy_v1.policy_id,
            "protect newest files",
            &policy_template("/workspace/c"),
        )
        .unwrap();
    assert_eq!(
        pap.update_binding(
            &binding_v1.spec.binding_id,
            &policy_v3.policy_id,
            policy_v3.revision,
            &scope.scope_id,
            scope.revision,
        ),
        Err(PapError::OperationInProgress)
    );
    assert_eq!(
        pap.delete_binding(&binding_v1.spec.binding_id),
        Err(PapError::OperationInProgress)
    );

    repository
        .update_binding_status(
            &reactivated.spec.binding_id,
            reactivated.spec.binding_revision,
            applying,
            BindingStatus::Ready,
        )
        .unwrap();
    let reactivated = pap
        .update_binding(
            &binding_v1.spec.binding_id,
            &policy_v3.policy_id,
            policy_v3.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    assert_eq!(reactivated.spec.binding_revision.get(), 5);
    assert_eq!(reactivated.status, BindingStatus::PendingApply);
    assert_eq!(
        repository.update_binding_status(
            &reactivated.spec.binding_id,
            Revision::new(4).unwrap(),
            reactivated.status,
            BindingStatus::Applying,
        ),
        Err(PapError::Conflict),
        "status CAS must target the current Binding revision"
    );

    let pending_delete = pap.delete_binding(&binding_v1.spec.binding_id).unwrap();
    assert_eq!(pending_delete.spec.binding_revision.get(), 6);
    let deleting = pending_delete.status.start_reconcile().unwrap();
    repository
        .update_binding_status(
            &pending_delete.spec.binding_id,
            pending_delete.spec.binding_revision,
            pending_delete.status,
            deleting,
        )
        .unwrap();
    assert_eq!(
        pap.update_binding(
            &binding_v1.spec.binding_id,
            &policy_v3.policy_id,
            policy_v3.revision,
            &scope.scope_id,
            scope.revision,
        ),
        Err(PapError::OperationInProgress)
    );
    assert_eq!(
        pap.delete_binding(&binding_v1.spec.binding_id)
            .unwrap()
            .status,
        BindingStatus::Deleting,
        "a repeated Delete remains idempotent while Delete is running"
    );
    repository
        .update_binding_status(
            &pending_delete.spec.binding_id,
            pending_delete.spec.binding_revision,
            deleting,
            BindingStatus::Deleted,
        )
        .unwrap();
    let reapplied = pap
        .update_binding(
            &binding_v1.spec.binding_id,
            &policy_v3.policy_id,
            policy_v3.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    assert_eq!(reapplied.spec.binding_revision.get(), 7);
    assert_eq!(reapplied.status, BindingStatus::PendingApply);
    assert_eq!(
        pap.list_bindings(100, 0).unwrap().items,
        vec![reapplied.clone()]
    );

    let state = repository.lock().unwrap();
    assert_eq!(state.bindings.len(), 1);
    assert_eq!(
        state.bindings[&binding_v1.spec.binding_id.to_string()],
        reapplied
    );
}

#[test]
fn binding_requires_exact_policy_and_scope_revisions() {
    let (pap, _) = service();
    let policy = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    let scope = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    let missing = ResourceId::new("missing").unwrap();

    assert_eq!(
        pap.create_binding(
            &missing,
            Revision::new(1).unwrap(),
            &scope.scope_id,
            scope.revision,
        ),
        Err(PapError::ReferencedPolicyRevisionNotFound)
    );
    assert_eq!(
        pap.create_binding(
            &policy.policy_id,
            policy.revision,
            &missing,
            Revision::new(1).unwrap(),
        ),
        Err(PapError::ReferencedScopeRevisionNotFound)
    );
    assert_eq!(
        pap.update_binding(
            &missing,
            &policy.policy_id,
            policy.revision,
            &scope.scope_id,
            scope.revision,
        ),
        Err(PapError::NotFound)
    );
}

#[test]
fn existing_binding_can_reuse_embedded_sources_after_current_records_advance() {
    let (pap, _) = service();
    let policy_v1 = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    let scope_v1 = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    let binding = pap
        .create_binding(
            &policy_v1.policy_id,
            policy_v1.revision,
            &scope_v1.scope_id,
            scope_v1.revision,
        )
        .unwrap();

    pap.update_policy(
        &policy_v1.policy_id,
        "protect more files",
        &policy_template("/workspace/b"),
    )
    .unwrap();
    pap.update_scope(
        &scope_v1.scope_id,
        &ScopeSelector::CgroupId { cgroup_id: 99 },
    )
    .unwrap();

    let pending_delete = pap.delete_binding(&binding.spec.binding_id).unwrap();
    let reapplied = pap
        .update_binding(
            &binding.spec.binding_id,
            &policy_v1.policy_id,
            policy_v1.revision,
            &scope_v1.scope_id,
            scope_v1.revision,
        )
        .unwrap();
    assert_eq!(reapplied.spec.binding_revision.get(), 3);
    assert_eq!(reapplied.spec.policy, policy_v1);
    assert_eq!(reapplied.spec.scope, scope_v1);
    assert_eq!(reapplied.status, BindingStatus::PendingApply);
    assert_eq!(pending_delete.spec.binding_revision.get(), 2);

    assert_eq!(
        pap.create_binding(
            &reapplied.spec.policy.policy_id,
            reapplied.spec.policy.revision,
            &reapplied.spec.scope.scope_id,
            reapplied.spec.scope.revision,
        ),
        Err(PapError::ReferencedPolicyRevisionNotFound),
        "a new Binding cannot select source revisions no longer current"
    );
}

#[test]
fn repository_atomically_rejects_a_binding_update_after_worker_claim() {
    let (pap, repository) = service();
    let policy = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    let scope = pap.create_scope(&ScopeSelector::Pid { pid: 4242 }).unwrap();
    let binding = pap
        .create_binding(
            &policy.policy_id,
            policy.revision,
            &scope.scope_id,
            scope.revision,
        )
        .unwrap();
    let mut stale_replacement = binding.clone();
    stale_replacement.spec.binding_revision = Revision::new(2).unwrap();

    repository
        .update_binding_status(
            &binding.spec.binding_id,
            binding.spec.binding_revision,
            BindingStatus::PendingApply,
            BindingStatus::Applying,
        )
        .unwrap();

    assert_eq!(
        repository.update_binding(&stale_replacement),
        Err(PapError::OperationInProgress),
        "the repository gate must close the service-read/worker-claim race"
    );
}

#[test]
fn compiler_output_identity_is_checked_before_storage() {
    let repository = Arc::new(FakeRepository::default());
    let compiler = Arc::new(FixtureCompiler {
        mismatch_identity: true,
    });
    let pap = PapService::new(repository, compiler);

    let error = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap_err();
    let PapError::InvalidPolicy(error) = error else {
        panic!("expected invalid compiler output");
    };
    assert_eq!(error.path, "canonicalPolicy.policyId");
}

#[test]
fn revision_exhaustion_and_pagination_bounds_are_explicit() {
    let (pap, repository) = service();
    let first = pap
        .create_policy("protect files", &policy_template("/workspace/a"))
        .unwrap();
    let maximum = Revision::new(u32::MAX).unwrap();
    let mut exhausted = first.clone();
    exhausted.revision = maximum;
    exhausted.canonical_policy.revision = maximum;
    {
        let mut state = repository.lock().unwrap();
        state.policies.clear();
        state
            .policies
            .insert(first.policy_id.as_str().to_owned(), exhausted);
        state
            .policy_heads
            .insert(first.policy_id.as_str().to_owned(), u32::MAX);
    }

    assert_eq!(
        pap.update_policy(
            &first.policy_id,
            "changed",
            &policy_template("/workspace/b"),
        ),
        Err(PapError::RevisionExhausted)
    );
    assert_eq!(pap.list_policies(0, 0), Err(PapError::InvalidPagination));
    assert_eq!(
        pap.list_policies(1_001, 0),
        Err(PapError::InvalidPagination)
    );
}

fn next_raw_revision(current: Option<u32>) -> Option<u32> {
    match current {
        Some(revision) => revision.checked_add(1),
        None => Some(1),
    }
}

fn page<T>(items: Vec<T>, limit: u32, offset: u32) -> Page<T> {
    let total = u64::try_from(items.len()).expect("test item count fits u64");
    let offset = usize::try_from(offset).expect("u32 offset fits usize");
    let limit = usize::try_from(limit).expect("u32 limit fits usize");
    Page {
        items: items.into_iter().skip(offset).take(limit).collect(),
        total,
    }
}
