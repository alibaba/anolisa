use crate::*;
use asc_foundation_types::ResourceId;
use asc_policy_types::binding::BindingView;

pub trait TestStore: BindingStateRepository {
    fn read(&self, id: &ResourceId) -> Result<Option<ReconcileRecord>, StoreError> {
        self.get_binding_state(id)
    }
    // Simulates an already admitted request racing with a worker. This is not
    // the PAP product API (which rejects reversal of running operations).
    fn compare_exchange_reconcile_intent(
        &self,
        expected: &ExpectedBinding,
        desired: &BindingView,
    ) -> Result<bool, StoreError> {
        let Some(before) = self.get_binding_state(&expected.id)? else {
            return Ok(false);
        };
        if !expected.matches(&before.binding) {
            return Ok(false);
        }
        if &before.binding == desired {
            return Ok(true);
        }
        let mut next = before.clone();
        next.binding = desired.clone();
        next.runtime = RuntimeState::default();
        Ok(
            self.compare_exchange_binding_state(&before, &BindingStateWrite::new(next))?
                == WriteResult::Applied,
        )
    }
}
impl<T: BindingStateRepository + ?Sized> TestStore for T {}

pub fn write_phase(expected: &BindingStateSnapshot, write: &BindingStateWrite) -> &'static str {
    let Some(next) = &write.next else {
        return "finish";
    };
    if !expected.binding.status.is_reconciling() && next.binding.status.is_reconciling() {
        "claim"
    } else if expected.binding.status.is_reconciling()
        && next.binding.status == expected.binding.status
    {
        "register"
    } else {
        "finish"
    }
}
