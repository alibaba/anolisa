//! Versioned Gateway capability profiles and their closed Runtime tool manifests.
//!
//! Profiles are fixed contracts selected by trusted Gateway configuration. Task
//! input and Runtime output may verify a profile, but cannot extend its tools.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::common::{BoundedName, BoundedOpaque, Digest, TargetRef};

/// Canonical wire and configuration name of the portable Task-only profile.
pub const TASK_ONLY_V1_PROFILE: &str = "task-only-v1";
/// Canonical wire and configuration name of the workspace-checkpoint profile.
pub const WORKSPACE_CHECKPOINT_V1_PROFILE: &str = "workspace-checkpoint-v1";
/// Runtime tool resolved by the Gateway without a host side effect.
pub const ASK_USER_QUESTION_TOOL: &str = "ask_user_question";
/// Runtime tool whose side effect must cross a governed checkpoint target.
pub const WORKSPACE_CHECKPOINT_CREATE_TOOL: &str = "workspace_checkpoint_create";
/// Canonical name of the only checkpoint provider admitted by this contract.
pub const WS_CKPT_PROVIDER: &str = "ws-ckpt";
/// Domain separator for the first capability-profile manifest format.
pub const CAPABILITY_PROFILE_MANIFEST_DOMAIN: &str = "cosh.gateway.capability-profile.v1";
/// Canonical manifest of the portable Task-only profile.
pub const TASK_ONLY_V1_CANONICAL_MANIFEST: &str = concat!(
    "cosh.gateway.capability-profile.v1\n",
    "profile:task-only-v1\n",
    "target:\n",
    "workspace/cosh/task-only-v1\n",
    "runtime-tools:\n",
    "ask_user_question\n",
);
/// Pinned SHA-256 digest of [`TASK_ONLY_V1_CANONICAL_MANIFEST`].
pub const TASK_ONLY_V1_MANIFEST_DIGEST: &str =
    "2b95e0f3e28df8eb2b7930f2dec3650ffe399f971671c971865e4663c382c94a";
/// Canonical manifest of the optional workspace-checkpoint profile.
///
/// Each profile manifest is an opaque pinned constant compared by digest, never
/// a parsed structure. A profile without providers therefore omits the trailing
/// `providers:` section entirely, which keeps
/// [`TASK_ONLY_V1_CANONICAL_MANIFEST`] byte-identical to its original revision.
pub const WORKSPACE_CHECKPOINT_V1_CANONICAL_MANIFEST: &str = concat!(
    "cosh.gateway.capability-profile.v1\n",
    "profile:workspace-checkpoint-v1\n",
    "target:\n",
    "workspace/cosh/workspace-checkpoint-v1\n",
    "runtime-tools:\n",
    "ask_user_question\n",
    "workspace_checkpoint_create\n",
    "providers:\n",
    "ws-ckpt\n",
);
/// Pinned SHA-256 digest of [`WORKSPACE_CHECKPOINT_V1_CANONICAL_MANIFEST`].
pub const WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST: &str =
    "6b3e7093e7b8656d4a7cf21faa85b9eed761ef415d002623cfc442f3ef3c8ae1";

const TASK_ONLY_V1_RUNTIME_TOOLS: &[&str] = &[ASK_USER_QUESTION_TOOL];
const WORKSPACE_CHECKPOINT_V1_RUNTIME_TOOLS: &[&str] =
    &[ASK_USER_QUESTION_TOOL, WORKSPACE_CHECKPOINT_CREATE_TOOL];
const NO_PROVIDERS: &[CapabilityProviderId] = &[];
const WS_CKPT_PROVIDER_SET: &[CapabilityProviderId] = &[CapabilityProviderId::WsCkpt];

/// Failure returned when a profile name is not an admitted production profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unsupported capability profile; expected `task-only-v1` or `workspace-checkpoint-v1`")]
pub struct CapabilityProfileParseError;

/// Failure returned when a provider name is not an admitted production provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unsupported capability provider; expected `ws-ckpt`")]
pub struct CapabilityProviderParseError;

/// Versioned identity of a side-effect provider sealed into a capability profile.
///
/// Providers are selected only by the profile a trusted Gateway configuration
/// admits. Task input, Runtime tool names, and ACP payloads never widen this set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityProviderId {
    /// Workspace checkpoint provider backed by the local `ws-ckpt` daemon.
    WsCkpt,
}

impl CapabilityProviderId {
    /// Returns the canonical wire and configuration name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WsCkpt => WS_CKPT_PROVIDER,
        }
    }

    /// Parses an exact canonical production provider name.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityProviderParseError`] for every unknown name. Unknown
    /// names never fall back to an admitted provider.
    pub fn parse(value: &str) -> Result<Self, CapabilityProviderParseError> {
        match value {
            WS_CKPT_PROVIDER => Ok(Self::WsCkpt),
            _ => Err(CapabilityProviderParseError),
        }
    }
}

/// Failure returned when advertised profile state differs from its closed contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CapabilityProfileVerificationError {
    /// The profile name differs from the configured profile.
    #[error("capability profile identity does not match the configured profile")]
    ProfileMismatch,
    /// The profile manifest digest differs from the pinned contract digest.
    #[error("capability profile manifest digest does not match the configured profile")]
    ManifestDigestMismatch,
    /// The Runtime tool inventory differs from the profile's closed inventory.
    #[error("Runtime tool inventory does not match the configured capability profile")]
    RuntimeToolInventoryMismatch,
    /// The admitted provider set differs from the profile's sealed provider set.
    #[error("capability provider set does not match the configured capability profile")]
    ProviderSetMismatch,
}

/// Versioned identity of an admitted Gateway capability profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayCapabilityProfileId {
    /// Portable profile whose only Runtime tool asks the user a question.
    TaskOnlyV1,
    /// Optional profile that additionally admits one governed checkpoint target.
    WorkspaceCheckpointV1,
}

impl GatewayCapabilityProfileId {
    /// Returns the canonical wire and configuration name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskOnlyV1 => TASK_ONLY_V1_PROFILE,
            Self::WorkspaceCheckpointV1 => WORKSPACE_CHECKPOINT_V1_PROFILE,
        }
    }

    /// Parses an exact canonical production profile name.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityProfileParseError`] for every unknown name. Unknown
    /// names never fall back to the Task-only profile.
    pub fn parse(value: &str) -> Result<Self, CapabilityProfileParseError> {
        match value {
            TASK_ONLY_V1_PROFILE => Ok(Self::TaskOnlyV1),
            WORKSPACE_CHECKPOINT_V1_PROFILE => Ok(Self::WorkspaceCheckpointV1),
            _ => Err(CapabilityProfileParseError),
        }
    }

    /// Returns the complete closed profile for this identity.
    #[must_use]
    pub const fn profile(self) -> GatewayCapabilityProfile {
        match self {
            Self::TaskOnlyV1 => GatewayCapabilityProfile::task_only_v1(),
            Self::WorkspaceCheckpointV1 => GatewayCapabilityProfile::workspace_checkpoint_v1(),
        }
    }
}

/// Durable identity that binds a profile name to its exact manifest revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayCapabilityProfileIdentity {
    /// Versioned profile name.
    pub profile_id: GatewayCapabilityProfileId,
    /// Digest of the complete canonical profile manifest.
    pub manifest_digest: Digest,
}

/// Closed capability profile admitted by a production Gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayCapabilityProfile {
    id: GatewayCapabilityProfileId,
}

impl GatewayCapabilityProfile {
    /// Returns the portable profile that admits no side-effect provider.
    #[must_use]
    pub const fn task_only_v1() -> Self {
        Self {
            id: GatewayCapabilityProfileId::TaskOnlyV1,
        }
    }

    /// Returns the optional profile that admits exactly one checkpoint provider.
    ///
    /// Selecting this profile does not make a checkpoint reachable on its own. A
    /// trusted Gateway configuration must additionally admit the sealed provider
    /// set as a real, identity-bound execution target.
    #[must_use]
    pub const fn workspace_checkpoint_v1() -> Self {
        Self {
            id: GatewayCapabilityProfileId::WorkspaceCheckpointV1,
        }
    }

    /// Returns the versioned profile identity.
    #[must_use]
    pub const fn id(self) -> GatewayCapabilityProfileId {
        self.id
    }

    /// Returns the canonical manifest covered by [`Self::manifest_digest`].
    #[must_use]
    pub const fn canonical_manifest(self) -> &'static str {
        match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TASK_ONLY_V1_CANONICAL_MANIFEST,
            GatewayCapabilityProfileId::WorkspaceCheckpointV1 => {
                WORKSPACE_CHECKPOINT_V1_CANONICAL_MANIFEST
            }
        }
    }

    /// Returns the pinned digest of the complete canonical manifest.
    #[must_use]
    pub fn manifest_digest(self) -> Digest {
        let digest = match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TASK_ONLY_V1_MANIFEST_DIGEST,
            GatewayCapabilityProfileId::WorkspaceCheckpointV1 => {
                WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST
            }
        };
        Digest::parse(digest)
            .unwrap_or_else(|_| unreachable!("reviewed static profile digests are canonical"))
    }

    /// Returns the durable identity for this exact manifest revision.
    #[must_use]
    pub fn identity(self) -> GatewayCapabilityProfileIdentity {
        GatewayCapabilityProfileIdentity {
            profile_id: self.id,
            manifest_digest: self.manifest_digest(),
        }
    }

    /// Returns the single governed target bound into the profile manifest.
    #[must_use]
    pub fn governed_target(self) -> TargetRef {
        TargetRef {
            kind: BoundedName::new("workspace")
                .unwrap_or_else(|_| unreachable!("static profile target names are bounded")),
            authority: BoundedName::new("cosh")
                .unwrap_or_else(|_| unreachable!("static profile target names are bounded")),
            identifier: BoundedOpaque::new(self.id.as_str())
                .unwrap_or_else(|_| unreachable!("static profile target IDs are bounded")),
        }
    }

    /// Returns the exact ordered Runtime tool inventory admitted by the profile.
    #[must_use]
    pub const fn runtime_tools(self) -> &'static [&'static str] {
        match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TASK_ONLY_V1_RUNTIME_TOOLS,
            GatewayCapabilityProfileId::WorkspaceCheckpointV1 => {
                WORKSPACE_CHECKPOINT_V1_RUNTIME_TOOLS
            }
        }
    }

    /// Returns the exact ordered side-effect provider set sealed into the profile.
    ///
    /// The Task-only profile returns an empty set, so a Task-only instance can
    /// never reach a side-effect provider even when one is installed on the host.
    #[must_use]
    pub const fn providers(self) -> &'static [CapabilityProviderId] {
        match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => NO_PROVIDERS,
            GatewayCapabilityProfileId::WorkspaceCheckpointV1 => WS_CKPT_PROVIDER_SET,
        }
    }

    /// Verifies a durable or advertised identity against the configured profile.
    ///
    /// # Errors
    ///
    /// Returns a mismatch when either the versioned name or manifest digest
    /// differs. Callers must reject the binding instead of selecting a fallback.
    pub fn verify_identity(
        self,
        actual: &GatewayCapabilityProfileIdentity,
    ) -> Result<(), CapabilityProfileVerificationError> {
        if actual.profile_id != self.id {
            return Err(CapabilityProfileVerificationError::ProfileMismatch);
        }
        if actual.manifest_digest != self.manifest_digest() {
            return Err(CapabilityProfileVerificationError::ManifestDigestMismatch);
        }
        Ok(())
    }

    /// Verifies that a Runtime advertises exactly the closed profile inventory.
    ///
    /// # Errors
    ///
    /// Returns a mismatch for missing, additional, reordered, or renamed tools.
    pub fn verify_runtime_tools(
        self,
        actual: &[&str],
    ) -> Result<(), CapabilityProfileVerificationError> {
        if actual == self.runtime_tools() {
            Ok(())
        } else {
            Err(CapabilityProfileVerificationError::RuntimeToolInventoryMismatch)
        }
    }

    /// Verifies that trusted configuration admitted exactly the sealed provider set.
    ///
    /// # Errors
    ///
    /// Returns a mismatch for missing, additional, reordered, or substituted
    /// providers. A Task-only instance therefore rejects any admitted provider.
    pub fn verify_providers(
        self,
        actual: &[CapabilityProviderId],
    ) -> Result<(), CapabilityProfileVerificationError> {
        if actual == self.providers() {
            Ok(())
        } else {
            Err(CapabilityProfileVerificationError::ProviderSetMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::*;

    #[test]
    fn task_only_manifest_and_digest_are_pinned() {
        let profile = GatewayCapabilityProfile::task_only_v1();
        let actual_digest = format!("{:x}", Sha256::digest(profile.canonical_manifest()));

        assert_eq!(profile.id().as_str(), TASK_ONLY_V1_PROFILE);
        assert!(profile
            .canonical_manifest()
            .starts_with(CAPABILITY_PROFILE_MANIFEST_DOMAIN));
        assert_eq!(
            profile.canonical_manifest(),
            TASK_ONLY_V1_CANONICAL_MANIFEST
        );
        assert_eq!(
            profile.manifest_digest().as_str(),
            TASK_ONLY_V1_MANIFEST_DIGEST
        );
        assert_eq!(actual_digest, TASK_ONLY_V1_MANIFEST_DIGEST);
        assert_eq!(profile.runtime_tools(), [ASK_USER_QUESTION_TOOL]);
        assert_eq!(profile.verify_identity(&profile.identity()), Ok(()));
        let target = profile.governed_target();
        assert_eq!(target.kind.as_str(), "workspace");
        assert_eq!(target.authority.as_str(), "cosh");
        assert_eq!(target.identifier.as_str(), TASK_ONLY_V1_PROFILE);
        assert!(profile.canonical_manifest().contains(&format!(
            "target:\n{}/{}/{}\n",
            target.kind.as_str(),
            target.authority.as_str(),
            target.identifier.as_str(),
        )));
    }

    #[test]
    fn workspace_checkpoint_manifest_and_digest_are_pinned() {
        let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
        let actual_digest = format!("{:x}", Sha256::digest(profile.canonical_manifest()));

        assert_eq!(profile.id().as_str(), WORKSPACE_CHECKPOINT_V1_PROFILE);
        assert!(profile
            .canonical_manifest()
            .starts_with(CAPABILITY_PROFILE_MANIFEST_DOMAIN));
        assert_eq!(
            profile.canonical_manifest(),
            WORKSPACE_CHECKPOINT_V1_CANONICAL_MANIFEST
        );
        assert_eq!(
            profile.manifest_digest().as_str(),
            WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST
        );
        assert_eq!(actual_digest, WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST);
        assert_eq!(
            profile.runtime_tools(),
            [ASK_USER_QUESTION_TOOL, WORKSPACE_CHECKPOINT_CREATE_TOOL]
        );
        assert_eq!(profile.providers(), [CapabilityProviderId::WsCkpt]);
        assert_eq!(profile.verify_identity(&profile.identity()), Ok(()));
        let target = profile.governed_target();
        assert_eq!(target.kind.as_str(), "workspace");
        assert_eq!(target.authority.as_str(), "cosh");
        assert_eq!(target.identifier.as_str(), WORKSPACE_CHECKPOINT_V1_PROFILE);
        assert!(profile.canonical_manifest().contains(&format!(
            "target:\n{}/{}/{}\n",
            target.kind.as_str(),
            target.authority.as_str(),
            target.identifier.as_str(),
        )));
        assert!(profile
            .canonical_manifest()
            .ends_with(&format!("providers:\n{WS_CKPT_PROVIDER}\n")));
    }

    #[test]
    fn optional_profile_never_alters_the_task_only_contract() {
        let task_only = GatewayCapabilityProfile::task_only_v1();
        let checkpoint = GatewayCapabilityProfile::workspace_checkpoint_v1();

        // The Task-only manifest is byte-identical to its original revision, so
        // the private Core v3 wire mirror keeps verifying the pinned digest.
        assert_eq!(
            task_only.manifest_digest().as_str(),
            TASK_ONLY_V1_MANIFEST_DIGEST
        );
        assert!(!task_only.canonical_manifest().contains("providers:"));
        assert_eq!(task_only.providers(), []);
        assert_eq!(task_only.verify_providers(&[]), Ok(()));
        assert_ne!(task_only.governed_target(), checkpoint.governed_target());
        assert_ne!(task_only.manifest_digest(), checkpoint.manifest_digest());
        assert_eq!(
            task_only.verify_identity(&checkpoint.identity()),
            Err(CapabilityProfileVerificationError::ProfileMismatch)
        );
        assert_eq!(
            checkpoint.verify_identity(&task_only.identity()),
            Err(CapabilityProfileVerificationError::ProfileMismatch)
        );
    }

    #[test]
    fn provider_sets_are_exact_and_never_widened_by_installation() {
        let task_only = GatewayCapabilityProfile::task_only_v1();
        let checkpoint = GatewayCapabilityProfile::workspace_checkpoint_v1();

        // A host that happens to run ws-ckpt is not authority for a Task-only
        // instance; the empty sealed set rejects the installed provider.
        assert_eq!(
            task_only.verify_providers(&[CapabilityProviderId::WsCkpt]),
            Err(CapabilityProfileVerificationError::ProviderSetMismatch)
        );
        assert_eq!(
            checkpoint.verify_providers(&[CapabilityProviderId::WsCkpt]),
            Ok(())
        );
        assert_eq!(
            checkpoint.verify_providers(&[]),
            Err(CapabilityProfileVerificationError::ProviderSetMismatch)
        );
        assert_eq!(
            checkpoint
                .verify_providers(&[CapabilityProviderId::WsCkpt, CapabilityProviderId::WsCkpt]),
            Err(CapabilityProfileVerificationError::ProviderSetMismatch)
        );
    }

    #[test]
    fn provider_names_are_exact_and_fail_closed() {
        assert_eq!(
            CapabilityProviderId::parse(WS_CKPT_PROVIDER),
            Ok(CapabilityProviderId::WsCkpt)
        );
        assert_eq!(CapabilityProviderId::WsCkpt.as_str(), "ws-ckpt");
        for unknown in ["", "ws_ckpt", "WS-CKPT", "ws-ckpt-v1", "shell"] {
            assert_eq!(
                CapabilityProviderId::parse(unknown),
                Err(CapabilityProviderParseError)
            );
        }
        assert_eq!(
            serde_json::to_value(CapabilityProviderId::WsCkpt).expect("provider ID serializes"),
            serde_json::json!(WS_CKPT_PROVIDER)
        );
    }

    #[test]
    fn profile_names_are_exact_and_fail_closed() {
        assert_eq!(
            GatewayCapabilityProfileId::parse(TASK_ONLY_V1_PROFILE),
            Ok(GatewayCapabilityProfileId::TaskOnlyV1)
        );
        assert_eq!(
            GatewayCapabilityProfileId::TaskOnlyV1.profile(),
            GatewayCapabilityProfile::task_only_v1()
        );
        assert_eq!(
            GatewayCapabilityProfileId::parse(WORKSPACE_CHECKPOINT_V1_PROFILE),
            Ok(GatewayCapabilityProfileId::WorkspaceCheckpointV1)
        );
        assert_eq!(
            GatewayCapabilityProfileId::WorkspaceCheckpointV1.profile(),
            GatewayCapabilityProfile::workspace_checkpoint_v1()
        );
        for unknown in [
            "",
            "task-only",
            "task-only-v2",
            "TASK-ONLY-V1",
            // The contract rejected this provider-shaped name; it must not be
            // revived as an alias for the capability-shaped profile name.
            "ws-ckpt-v1",
            "ws-ckpt",
            "workspace-checkpoint",
            "workspace-checkpoint-v2",
            "WORKSPACE-CHECKPOINT-V1",
        ] {
            assert_eq!(
                GatewayCapabilityProfileId::parse(unknown),
                Err(CapabilityProfileParseError)
            );
        }
    }

    #[test]
    fn profile_identity_rejects_digest_drift() {
        let profile = GatewayCapabilityProfile::task_only_v1();
        let drifted = GatewayCapabilityProfileIdentity {
            profile_id: GatewayCapabilityProfileId::TaskOnlyV1,
            manifest_digest: Digest::parse("0".repeat(64)).expect("test digest is canonical"),
        };

        assert_eq!(
            profile.verify_identity(&drifted),
            Err(CapabilityProfileVerificationError::ManifestDigestMismatch)
        );
    }

    #[test]
    fn runtime_inventory_rejects_missing_or_additional_tools() {
        let profile = GatewayCapabilityProfile::task_only_v1();

        assert_eq!(
            profile.verify_runtime_tools(&[ASK_USER_QUESTION_TOOL]),
            Ok(())
        );
        for drifted in [
            &[][..],
            &[ASK_USER_QUESTION_TOOL, WORKSPACE_CHECKPOINT_CREATE_TOOL][..],
            &["ask_user"][..],
        ] {
            assert_eq!(
                profile.verify_runtime_tools(drifted),
                Err(CapabilityProfileVerificationError::RuntimeToolInventoryMismatch)
            );
        }
    }

    #[test]
    fn checkpoint_inventory_rejects_missing_extra_and_reordered_tools() {
        let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();

        assert_eq!(
            profile
                .verify_runtime_tools(&[ASK_USER_QUESTION_TOOL, WORKSPACE_CHECKPOINT_CREATE_TOOL]),
            Ok(())
        );
        for drifted in [
            &[][..],
            &[ASK_USER_QUESTION_TOOL][..],
            &[WORKSPACE_CHECKPOINT_CREATE_TOOL][..],
            &[WORKSPACE_CHECKPOINT_CREATE_TOOL, ASK_USER_QUESTION_TOOL][..],
            &[
                ASK_USER_QUESTION_TOOL,
                WORKSPACE_CHECKPOINT_CREATE_TOOL,
                "workspace_checkpoint_rollback",
            ][..],
            &[ASK_USER_QUESTION_TOOL, "workspace_checkpoint"][..],
        ] {
            assert_eq!(
                profile.verify_runtime_tools(drifted),
                Err(CapabilityProfileVerificationError::RuntimeToolInventoryMismatch)
            );
        }
    }

    #[test]
    fn profile_identity_uses_canonical_wire_names() {
        for (profile, name, digest) in [
            (
                GatewayCapabilityProfile::task_only_v1(),
                TASK_ONLY_V1_PROFILE,
                TASK_ONLY_V1_MANIFEST_DIGEST,
            ),
            (
                GatewayCapabilityProfile::workspace_checkpoint_v1(),
                WORKSPACE_CHECKPOINT_V1_PROFILE,
                WORKSPACE_CHECKPOINT_V1_MANIFEST_DIGEST,
            ),
        ] {
            let identity = profile.identity();
            let encoded = serde_json::to_value(&identity).expect("profile identity serializes");

            assert_eq!(encoded["profile_id"], name);
            assert_eq!(encoded["manifest_digest"], digest);
            assert_eq!(
                serde_json::from_value::<GatewayCapabilityProfileIdentity>(encoded)
                    .expect("profile identity deserializes"),
                identity
            );
        }
    }
}
