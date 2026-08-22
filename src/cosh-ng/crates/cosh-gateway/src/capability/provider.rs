//! Sealed side-effect provider registry admitted by trusted configuration.
//!
//! The registry is the single place where an admitted capability profile is
//! matched against the real execution targets an instance owns, and therefore the
//! only boundary at which an instance gains side-effect authority.
//!
//! Production configuration reaches exactly one shape: an instance with no
//! provider. No execution target exists here yet, so this registry is where a
//! requested provider is refused rather than resolved.

use cosh_gateway_contracts::profile::{
    CapabilityProfileVerificationError, CapabilityProviderId, GatewayCapabilityProfile,
};
use thiserror::Error;

/// Fail-closed failure raised before an instance owns any side-effect authority.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SealedProviderAdmissionError {
    /// Configuration admitted a provider set the profile does not seal.
    #[error(transparent)]
    ProviderSet(#[from] CapabilityProfileVerificationError),
    /// The checkpoint provider cannot yet be granted side-effect authority.
    ///
    /// The ws-ckpt checkpoint request is not identity-only: its dispatch path
    /// unconditionally runs workspace auto-initialization first, and the
    /// workspace identity a request carries is resolved as a relative path when
    /// its registration has disappeared. A checkpoint-create permit could
    /// therefore cause workspace registration, subvolume adoption, a directory
    /// move, or a symlink removal — none of which that permit grants, and none of
    /// which the Gateway can undo or bound after the fact.
    #[error("ws-ckpt checkpoint requests are not identity-only; provider admission is withheld")]
    CheckpointProviderWithheld,
}

/// Complete set of side-effect providers one Gateway instance may reach.
pub struct SealedCapabilityProviderRegistry {
    profile: GatewayCapabilityProfile,
    providers: Vec<CapabilityProviderId>,
}

impl SealedCapabilityProviderRegistry {
    /// Admits the exact provider set sealed into one capability profile.
    ///
    /// `requested` is the provider set trusted per-instance configuration asks
    /// for. A Task-only instance that requests a provider is rejected instead of
    /// widened: an installed ws-ckpt daemon is never authority on its own.
    ///
    /// Checkpoint provider admission is **withheld** for every profile, and no
    /// checkpoint execution target exists in this crate. The ws-ckpt protocol has
    /// no identity-only checkpoint request, so issuing one can make the daemon
    /// mutate the host outside the authority a checkpoint-create permit grants.
    /// Admission is refused here, at the boundary where an instance would gain that
    /// authority. Lifting it requires a checkpoint request that resolves a workspace
    /// identity strictly and never auto-initializes.
    ///
    /// # Errors
    ///
    /// Returns a provider-set mismatch when the requested set differs from the set
    /// the profile seals, and
    /// [`SealedProviderAdmissionError::CheckpointProviderWithheld`] whenever a
    /// checkpoint provider is requested by a profile that seals it.
    pub fn admit(
        profile: GatewayCapabilityProfile,
        requested: &[CapabilityProviderId],
    ) -> Result<Self, SealedProviderAdmissionError> {
        // Verify the sealed set first so a Task-only instance reports the narrower
        // provider-set mismatch rather than the withheld decision.
        profile.verify_providers(requested)?;
        if requested.contains(&CapabilityProviderId::WsCkpt) {
            return Err(SealedProviderAdmissionError::CheckpointProviderWithheld);
        }

        Ok(Self {
            profile,
            providers: requested.to_vec(),
        })
    }

    /// Admits the empty provider set required by a portable Task-only instance.
    ///
    /// # Errors
    ///
    /// Returns a provider-set mismatch when the profile seals a provider.
    pub fn task_only(
        profile: GatewayCapabilityProfile,
    ) -> Result<Self, SealedProviderAdmissionError> {
        Self::admit(profile, &[])
    }

    /// Returns the canonical admitted capability profile.
    #[must_use]
    pub const fn profile(&self) -> GatewayCapabilityProfile {
        self.profile
    }

    /// Returns the exact ordered provider set admitted for this instance.
    #[must_use]
    pub fn providers(&self) -> &[CapabilityProviderId] {
        &self.providers
    }
}

impl std::fmt::Debug for SealedCapabilityProviderRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedCapabilityProviderRegistry")
            .field("profile", &self.profile.id())
            .field("providers", &self.providers)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "provider/tests.rs"]
mod tests;
