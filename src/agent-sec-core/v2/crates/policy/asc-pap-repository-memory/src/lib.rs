//! **TEMPORARY IMPLEMENTATION -- NOT PART OF THE REVIEW SCOPE.**
//!
//! This process-local adapter exists only to make the PAP daemon request path
//! runnable before the durable Repository work package lands. Its internal
//! implementation is disposable, must not be treated as a production storage
//! design, and does not require review in the current PAP daemon-handler change.
//! All state is lost when the daemon restarts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{Page, PapError, PapRepository, PolicyRevisionState, ScopeRevisionState};
use asc_policy_types::binding::{BindingStatus, BindingView};
use asc_policy_types::policy::PreparedPolicy;
use asc_policy_types::scope::PreparedScope;

/// Process-local PAP Repository used until durable persistence is integrated.
///
/// This adapter implements the current-record Repository contract but loses all
/// state on daemon restart. It is an explicitly replaceable composition-root
/// dependency, not production persistence evidence.
#[derive(Debug, Default)]
pub struct ProcessLocalPapRepository {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    policy_heads: BTreeMap<String, Revision>,
    policies: BTreeMap<String, PreparedPolicy>,
    scope_heads: BTreeMap<String, Revision>,
    scopes: BTreeMap<String, PreparedScope>,
    bindings: BTreeMap<String, BindingView>,
}

impl ProcessLocalPapRepository {
    fn lock(&self) -> Result<MutexGuard<'_, State>, PapError> {
        self.state.lock().map_err(|_| PapError::Persistence)
    }
}

impl PapRepository for ProcessLocalPapRepository {
    fn put_policy(&self, policy: &PreparedPolicy) -> Result<PreparedPolicy, PapError> {
        let mut state = self.lock()?;
        let id = policy.policy_id.as_str().to_owned();
        if let Some(existing) = state.policies.get(&id)
            && existing.revision == policy.revision
        {
            return if existing == policy {
                Ok(existing.clone())
            } else {
                Err(PapError::Conflict)
            };
        }
        if !is_next_revision(state.policy_heads.get(&id).copied(), policy.revision) {
            return Err(PapError::Conflict);
        }
        state.policies.insert(id.clone(), policy.clone());
        state.policy_heads.insert(id, policy.revision);
        Ok(policy.clone())
    }

    fn get_policy_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<PolicyRevisionState>, PapError> {
        let state = self.lock()?;
        Ok(state
            .policy_heads
            .get(id.as_str())
            .copied()
            .map(|last_allocated_revision| PolicyRevisionState {
                last_allocated_revision,
                current: state.policies.get(id.as_str()).cloned(),
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
        let state = self.lock()?;
        Ok(page(state.policies.values(), limit, offset))
    }

    fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PapError> {
        let mut state = self.lock()?;
        if state
            .policies
            .get(id.as_str())
            .is_none_or(|policy| policy.revision != revision)
        {
            return Err(PapError::NotFound);
        }
        state
            .policies
            .remove(id.as_str())
            .ok_or(PapError::Persistence)
    }

    fn put_scope(&self, scope: &PreparedScope) -> Result<PreparedScope, PapError> {
        let mut state = self.lock()?;
        let id = scope.scope_id.as_str().to_owned();
        if let Some(existing) = state.scopes.get(&id)
            && existing.revision == scope.revision
        {
            return if existing == scope {
                Ok(existing.clone())
            } else {
                Err(PapError::Conflict)
            };
        }
        if !is_next_revision(state.scope_heads.get(&id).copied(), scope.revision) {
            return Err(PapError::Conflict);
        }
        state.scopes.insert(id.clone(), scope.clone());
        state.scope_heads.insert(id, scope.revision);
        Ok(scope.clone())
    }

    fn get_scope_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ScopeRevisionState>, PapError> {
        let state = self.lock()?;
        Ok(state
            .scope_heads
            .get(id.as_str())
            .copied()
            .map(|last_allocated_revision| ScopeRevisionState {
                last_allocated_revision,
                current: state.scopes.get(id.as_str()).cloned(),
            }))
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
        let state = self.lock()?;
        Ok(page(state.scopes.values(), limit, offset))
    }

    fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PapError> {
        let mut state = self.lock()?;
        if state
            .scopes
            .get(id.as_str())
            .is_none_or(|scope| scope.revision != revision)
        {
            return Err(PapError::NotFound);
        }
        state
            .scopes
            .remove(id.as_str())
            .ok_or(PapError::Persistence)
    }

    fn update_binding(&self, binding: &BindingView) -> Result<BindingView, PapError> {
        if !matches!(
            binding.status,
            BindingStatus::PendingApply | BindingStatus::PendingDelete
        ) {
            return Err(PapError::Conflict);
        }

        let mut state = self.lock()?;
        let id = binding.spec.binding_id.as_str().to_owned();
        if let Some(current) = state.bindings.get(&id) {
            if current == binding {
                return Ok(current.clone());
            }
            if current.status.is_reconciling() {
                return Err(PapError::OperationInProgress);
            }
            if !is_next_revision(
                Some(current.spec.binding_revision),
                binding.spec.binding_revision,
            ) {
                return Err(PapError::Conflict);
            }
        } else if binding.spec.binding_revision.get() != 1
            || binding.status != BindingStatus::PendingApply
        {
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
        if binding.spec.binding_revision != binding_revision || binding.status != expected_status {
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
        let state = self.lock()?;
        Ok(page(state.bindings.values(), limit, offset))
    }
}

fn is_next_revision(current: Option<Revision>, candidate: Revision) -> bool {
    match current {
        None => candidate.get() == 1,
        Some(current) => current.checked_next() == Ok(candidate),
    }
}

fn page<'a, T: Clone + 'a>(
    items: impl ExactSizeIterator<Item = &'a T>,
    limit: u32,
    offset: u32,
) -> Page<T> {
    let total = u64::try_from(items.len()).unwrap_or(u64::MAX);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    Page {
        items: items.skip(offset).take(limit).cloned().collect(),
        total,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use asc_pap::PapService;
    use asc_policy_engine::PolicyTemplateCompiler;
    use asc_policy_types::authoring::PolicyTemplate;
    use asc_policy_types::binding::BindingStatus;
    use asc_policy_types::scope::ScopeSelector;

    use super::*;

    #[derive(Debug)]
    struct CloneTracked<'a> {
        value: u32,
        clones: &'a AtomicUsize,
    }

    impl Clone for CloneTracked<'_> {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                value: self.value,
                clones: self.clones,
            }
        }
    }

    #[test]
    fn pagination_clones_only_the_selected_records() {
        let clones = AtomicUsize::new(0);
        let records: Vec<_> = (0..5)
            .map(|value| CloneTracked {
                value,
                clones: &clones,
            })
            .collect();

        let selected = page(records.iter(), 2, 2);

        assert_eq!(selected.total, 5);
        assert_eq!(
            selected
                .items
                .iter()
                .map(|record| record.value)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(clones.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn repository_runs_the_current_pap_request_slice_without_a_reconciler() {
        let repository = Arc::new(ProcessLocalPapRepository::default());
        let pap = PapService::new(Arc::clone(&repository), Arc::new(PolicyTemplateCompiler));
        let policy = pap
            .create_policy(
                "protect files",
                &PolicyTemplate::PreventFileDeletion {
                    files: vec!["/workspace/important".to_owned()],
                },
            )
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

        assert_eq!(binding.status, BindingStatus::PendingApply);
        assert_eq!(pap.list_policies(10, 0).unwrap().total, 1);
        assert_eq!(pap.list_scopes(10, 0).unwrap().total, 1);
        assert_eq!(pap.list_bindings(10, 0).unwrap().items, [binding]);
        assert_eq!(
            ProcessLocalPapRepository::default()
                .list_policies(10, 0)
                .unwrap()
                .total,
            0,
            "a new process-local Repository intentionally has no prior state"
        );
    }
}
