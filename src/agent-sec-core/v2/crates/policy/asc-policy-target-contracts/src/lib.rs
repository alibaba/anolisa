//! PEP-neutral Adapter and Client ports. No runtime, storage or transport implementation.

#![forbid(unsafe_code)]

use asc_policy_types::binding::PreparedBinding;
use asc_policy_types::target::{
    AdapterFault, DeploymentReport, Failure, PreparedApply, TargetBindingPlan, TargetRef,
    TranslationOutcome,
};

/// Stateless complete-Binding translation; no repository or target I/O.
pub trait TargetBindingAdapter: Send + Sync {
    /// # Errors
    /// Returns an internal fault, distinct from semantic rejection.
    fn translate(&self, binding: &PreparedBinding) -> Result<TranslationOutcome, AdapterFault>;
}

impl<F> TargetBindingAdapter for F
where
    F: Fn(&PreparedBinding) -> Result<TranslationOutcome, AdapterFault> + Send + Sync,
{
    fn translate(&self, binding: &PreparedBinding) -> Result<TranslationOutcome, AdapterFault> {
        self(binding)
    }
}

/// Synchronous Client calls must be bounded by the concrete Client's timeouts.
/// They must not return while a local modifying task is still running. Remote
/// late completion is outside this process-local ownership guarantee.
pub trait TargetDeploymentClient: Send + Sync {
    /// No target modification. Return stable, non-secret replay/cleanup bytes.
    /// # Errors
    /// Returns a classified preparation failure.
    fn prepare_apply(&self, plan: &TargetBindingPlan) -> Result<PreparedApply, Failure>;
    fn create(&self, prepared: &PreparedApply) -> DeploymentReport;
    /// `previous` excludes the current target identity, including on retries.
    /// A PEP using one identity may therefore receive an empty previous set.
    fn update(&self, previous: &[TargetRef], prepared: &PreparedApply) -> DeploymentReport;
    fn delete(&self, targets: &[TargetRef]) -> DeploymentReport;
}
