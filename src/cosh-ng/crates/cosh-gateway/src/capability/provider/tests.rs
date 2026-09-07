use cosh_gateway_contracts::profile::GatewayCapabilityProfileId;

use super::*;

/// Every profile an instance may configure, with the provider set it seals.
fn profiles() -> [(GatewayCapabilityProfile, &'static [CapabilityProviderId]); 2] {
    [
        (GatewayCapabilityProfile::task_only_v1(), &[]),
        (
            GatewayCapabilityProfile::workspace_checkpoint_v1(),
            &[CapabilityProviderId::WsCkpt],
        ),
    ]
}

#[test]
fn task_only_instance_admits_an_empty_provider_set() {
    let registry =
        SealedCapabilityProviderRegistry::task_only(GatewayCapabilityProfile::task_only_v1())
            .expect("a Task-only instance needs no provider");

    assert_eq!(registry.profile(), GatewayCapabilityProfile::task_only_v1());
    assert_eq!(registry.providers(), []);
}

#[test]
fn task_only_instance_starts_without_any_ws_ckpt_configuration() {
    // Nothing about ws-ckpt is consulted here, which is exactly the portable
    // Task-only deployment: no socket, no directory, no daemon.
    let registry =
        SealedCapabilityProviderRegistry::admit(GatewayCapabilityProfile::task_only_v1(), &[])
            .expect("a Task-only instance starts with no checkpoint dependency");

    assert_eq!(registry.providers(), []);
}

#[test]
fn task_only_instance_rejects_a_requested_checkpoint_provider() {
    let error = SealedCapabilityProviderRegistry::admit(
        GatewayCapabilityProfile::task_only_v1(),
        &[CapabilityProviderId::WsCkpt],
    )
    .expect_err("an installed provider is not authority for a Task-only instance");

    // The narrower sealed-set mismatch is reported before the withheld decision.
    assert_eq!(
        error,
        SealedProviderAdmissionError::ProviderSet(
            CapabilityProfileVerificationError::ProviderSetMismatch
        )
    );
}

/// The checkpoint provider is withheld even for the profile that seals it.
///
/// ws-ckpt has no identity-only checkpoint request, so a checkpoint-create permit
/// could cause workspace registration. Admission is refused at this boundary
/// rather than letting an adapter attempt to bound an effect it cannot undo.
#[test]
fn checkpoint_provider_admission_is_withheld_until_requests_are_identity_only() {
    let error = SealedCapabilityProviderRegistry::admit(
        GatewayCapabilityProfile::workspace_checkpoint_v1(),
        &[CapabilityProviderId::WsCkpt],
    )
    .expect_err("checkpoint side-effect authority is not granted yet");

    assert_eq!(
        error,
        SealedProviderAdmissionError::CheckpointProviderWithheld
    );
}

#[test]
fn checkpoint_instance_rejects_a_missing_provider() {
    let error = SealedCapabilityProviderRegistry::admit(
        GatewayCapabilityProfile::workspace_checkpoint_v1(),
        &[],
    )
    .expect_err("an unavailable provider must refuse admission");

    assert_eq!(
        error,
        SealedProviderAdmissionError::ProviderSet(
            CapabilityProfileVerificationError::ProviderSetMismatch
        )
    );
}

/// No profile and no requested set yields side-effect authority.
///
/// This enumerates every reachable admission outcome. No checkpoint execution
/// target exists in this crate, so there is no other path to cover.
#[test]
fn no_instance_can_obtain_checkpoint_side_effect_authority() {
    for (profile, sealed) in profiles() {
        for requested in [&[][..], &[CapabilityProviderId::WsCkpt][..]] {
            match SealedCapabilityProviderRegistry::admit(profile, requested) {
                Ok(registry) => {
                    assert_eq!(profile.id(), GatewayCapabilityProfileId::TaskOnlyV1);
                    assert_eq!(registry.providers(), []);
                }
                Err(SealedProviderAdmissionError::CheckpointProviderWithheld) => {
                    assert_eq!(sealed, [CapabilityProviderId::WsCkpt]);
                    assert_eq!(requested, [CapabilityProviderId::WsCkpt]);
                }
                Err(SealedProviderAdmissionError::ProviderSet(_)) => {
                    assert_ne!(requested, sealed);
                }
            }
        }
    }
}
