//! Generation-stamped enforcement boundary used by containment activation.

use agentsight_enforcement_protocol::{ApplyCredentialPolicy, Binding};
use thiserror::Error;
use uuid::Uuid;

use crate::enforcement::{
    EnforcementCoordinator, EnforcementCoordinatorError, EnforcementError,
    IngestionGenerationLease, IngestionLease,
};

/// Typed failures returned by the containment enforcement boundary.
#[derive(Debug, Error)]
pub enum ContainmentEnforcerError {
    /// Readiness, transport, or local enforcement state is temporarily unavailable.
    #[error("{0}")]
    Unavailable(String),
    /// The enforcement boundary rejected or contradicted the durable request.
    #[error("{0}")]
    Rejected(String),
}

/// Applied binding paired with the ingestion generation that acknowledged it.
pub struct StampedBinding {
    binding: Binding,
    readiness_stamp: ContainmentReadinessStamp,
}

impl StampedBinding {
    /// Wraps a binding for an enforcer whose readiness does not change.
    pub fn stable(binding: Binding) -> Self {
        Self::new(binding, ContainmentReadinessStamp::stable())
    }

    pub(crate) fn new(binding: Binding, readiness_stamp: ContainmentReadinessStamp) -> Self {
        Self {
            binding,
            readiness_stamp,
        }
    }

    pub(crate) fn into_parts(self) -> (Binding, ContainmentReadinessStamp) {
        (self.binding, self.readiness_stamp)
    }
}

/// Persisted binding snapshot paired with the ingestion generation that observed it.
pub struct StampedBindings {
    bindings: Vec<Binding>,
    readiness_stamp: ContainmentReadinessStamp,
}

impl StampedBindings {
    /// Wraps a snapshot for an enforcer whose readiness does not change.
    pub fn stable(bindings: Vec<Binding>) -> Self {
        Self::new(bindings, ContainmentReadinessStamp::stable())
    }

    pub(crate) fn new(bindings: Vec<Binding>, readiness_stamp: ContainmentReadinessStamp) -> Self {
        Self {
            bindings,
            readiness_stamp,
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<Binding>, ContainmentReadinessStamp) {
        (self.bindings, self.readiness_stamp)
    }
}

/// Short-lived proof that one stamped ingestion generation remains ready.
pub trait ContainmentReadinessLease {}

/// Opaque identity of the ingestion generation that observed an acknowledgement.
pub struct ContainmentReadinessStamp(ReadinessStampKind);

enum ReadinessStampKind {
    Stable,
    Enforcement(IngestionLease),
}

impl ContainmentReadinessStamp {
    const fn stable() -> Self {
        Self(ReadinessStampKind::Stable)
    }
}

struct StableReadinessLease;

impl ContainmentReadinessLease for StableReadinessLease {}

/// Creates a lease for test or in-process enforcers with immutable readiness.
pub fn stable_readiness_lease() -> Box<dyn ContainmentReadinessLease> {
    Box::new(StableReadinessLease)
}

/// Enforcement operations required by containment orchestration.
pub trait ContainmentEnforcer: Send + Sync {
    /// Bounds foreground apply ownership before restart reconciliation may take over.
    fn foreground_claim_lease(&self) -> Duration;
    /// Compiles and applies one product-level credential policy.
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<StampedBinding, ContainmentEnforcerError>;
    /// Detaches a previously applied binding.
    fn detach(&self, binding_id: Uuid) -> Result<(), String>;
    /// Lists persisted enforcement bindings.
    fn bindings(&self) -> Result<StampedBindings, ContainmentEnforcerError>;
    /// Leases the stamped ready generation for a short activation transaction.
    fn lease_ready(
        &self,
        stamp: ContainmentReadinessStamp,
    ) -> Result<Box<dyn ContainmentReadinessLease + '_>, ContainmentEnforcerError>;
}

struct EnforcementReadinessLease<'a> {
    _lease: IngestionGenerationLease<'a>,
}

impl ContainmentReadinessLease for EnforcementReadinessLease<'_> {}

impl ContainmentEnforcer for EnforcementCoordinator {
    fn foreground_claim_lease(&self) -> Duration {
        self.credential_apply_claim_lease()
    }

    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<StampedBinding, ContainmentEnforcerError> {
        let stamp = current_stamp(self)?;
        let binding = EnforcementCoordinator::apply_credential_policy(self, request)
            .map_err(containment_enforcer_error)?;
        ensure_current_stamp(self, &stamp)?;
        Ok(StampedBinding::new(binding, stamp))
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), String> {
        EnforcementCoordinator::detach(self, binding_id).map_err(|error| {
            log::error!("containment detach failed: {error}");
            "enforcement detach failed".into()
        })
    }

    fn bindings(&self) -> Result<StampedBindings, ContainmentEnforcerError> {
        let stamp = current_stamp(self)?;
        let bindings =
            EnforcementCoordinator::bindings(self).map_err(containment_enforcer_error)?;
        ensure_current_stamp(self, &stamp)?;
        Ok(StampedBindings::new(bindings, stamp))
    }

    fn lease_ready(
        &self,
        stamp: ContainmentReadinessStamp,
    ) -> Result<Box<dyn ContainmentReadinessLease + '_>, ContainmentEnforcerError> {
        let ReadinessStampKind::Enforcement(stamp) = stamp.0 else {
            return Err(ingestion_changed());
        };
        self.lease_ingestion_generation(&stamp)
            .map(|lease| {
                Box::new(EnforcementReadinessLease { _lease: lease })
                    as Box<dyn ContainmentReadinessLease>
            })
            .ok_or_else(ingestion_changed)
    }
}

fn current_stamp(
    coordinator: &EnforcementCoordinator,
) -> Result<ContainmentReadinessStamp, ContainmentEnforcerError> {
    coordinator
        .ingestion_generation()
        .map(|lease| ContainmentReadinessStamp(ReadinessStampKind::Enforcement(lease)))
        .map_err(containment_enforcer_error)
}

fn ensure_current_stamp(
    coordinator: &EnforcementCoordinator,
    expected: &ContainmentReadinessStamp,
) -> Result<(), ContainmentEnforcerError> {
    let ReadinessStampKind::Enforcement(expected) = &expected.0 else {
        return Err(ingestion_changed());
    };
    if coordinator.ingestion_generation_is_current(expected) {
        return Ok(());
    }
    Err(ingestion_changed())
}

fn ingestion_changed() -> ContainmentEnforcerError {
    ContainmentEnforcerError::Unavailable("enforcement ingestion generation changed".into())
}

fn containment_enforcer_error(error: EnforcementCoordinatorError) -> ContainmentEnforcerError {
    let unavailable = matches!(
        &error,
        EnforcementCoordinatorError::IngestionUnavailable
            | EnforcementCoordinatorError::EnforcementUnavailable(_)
            | EnforcementCoordinatorError::Store(_)
            | EnforcementCoordinatorError::Thread(_)
            | EnforcementCoordinatorError::Client(
                EnforcementError::Io(_) | EnforcementError::Disconnected
            )
    );
    let message = error.to_string();
    if unavailable {
        ContainmentEnforcerError::Unavailable(message)
    } else {
        ContainmentEnforcerError::Rejected(message)
    }
}
