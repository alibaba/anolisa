//! Versioned Gateway capability profiles and their closed Runtime tool manifests.
//!
//! Profiles are fixed contracts selected by trusted Gateway configuration. Task
//! input and Runtime output may verify a profile, but cannot extend its tools.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::common::{BoundedName, BoundedOpaque, Digest, TargetRef};

/// Canonical wire and configuration name of the portable Task-only profile.
pub const TASK_ONLY_V1_PROFILE: &str = "task-only-v1";
/// Runtime tool resolved by the Gateway without a host side effect.
pub const ASK_USER_QUESTION_TOOL: &str = "ask_user_question";
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

const TASK_ONLY_V1_RUNTIME_TOOLS: &[&str] = &[ASK_USER_QUESTION_TOOL];

/// Failure returned when a profile name is not an admitted production profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unsupported capability profile; expected `task-only-v1`")]
pub struct CapabilityProfileParseError;

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
}

/// Versioned identity of an admitted Gateway capability profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayCapabilityProfileId {
    /// Portable profile whose only Runtime tool asks the user a question.
    TaskOnlyV1,
}

impl GatewayCapabilityProfileId {
    /// Returns the canonical wire and configuration name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskOnlyV1 => TASK_ONLY_V1_PROFILE,
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
            _ => Err(CapabilityProfileParseError),
        }
    }

    /// Returns the complete closed profile for this identity.
    #[must_use]
    pub const fn profile(self) -> GatewayCapabilityProfile {
        match self {
            Self::TaskOnlyV1 => GatewayCapabilityProfile::task_only_v1(),
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
    /// Returns the only production profile currently admitted by this contract.
    #[must_use]
    pub const fn task_only_v1() -> Self {
        Self {
            id: GatewayCapabilityProfileId::TaskOnlyV1,
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
        }
    }

    /// Returns the pinned digest of the complete canonical manifest.
    #[must_use]
    pub fn manifest_digest(self) -> Digest {
        let digest = match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TASK_ONLY_V1_MANIFEST_DIGEST,
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
        match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TargetRef {
                kind: BoundedName::new("workspace")
                    .unwrap_or_else(|_| unreachable!("static profile target names are bounded")),
                authority: BoundedName::new("cosh")
                    .unwrap_or_else(|_| unreachable!("static profile target names are bounded")),
                identifier: BoundedOpaque::new(TASK_ONLY_V1_PROFILE)
                    .unwrap_or_else(|_| unreachable!("static profile target IDs are bounded")),
            },
        }
    }

    /// Returns the exact ordered Runtime tool inventory admitted by the profile.
    #[must_use]
    pub const fn runtime_tools(self) -> &'static [&'static str] {
        match self.id {
            GatewayCapabilityProfileId::TaskOnlyV1 => TASK_ONLY_V1_RUNTIME_TOOLS,
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
    fn profile_names_are_exact_and_fail_closed() {
        assert_eq!(
            GatewayCapabilityProfileId::parse(TASK_ONLY_V1_PROFILE),
            Ok(GatewayCapabilityProfileId::TaskOnlyV1)
        );
        assert_eq!(
            GatewayCapabilityProfileId::TaskOnlyV1.profile(),
            GatewayCapabilityProfile::task_only_v1()
        );
        for unknown in [
            "",
            "task-only",
            "task-only-v2",
            "TASK-ONLY-V1",
            "ws-ckpt-v1",
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
            &[ASK_USER_QUESTION_TOOL, "workspace_checkpoint_create"][..],
            &["ask_user"][..],
        ] {
            assert_eq!(
                profile.verify_runtime_tools(drifted),
                Err(CapabilityProfileVerificationError::RuntimeToolInventoryMismatch)
            );
        }
    }

    #[test]
    fn profile_identity_uses_canonical_wire_names() {
        let identity = GatewayCapabilityProfile::task_only_v1().identity();
        let encoded = serde_json::to_value(&identity).expect("profile identity serializes");

        assert_eq!(encoded["profile_id"], TASK_ONLY_V1_PROFILE);
        assert_eq!(encoded["manifest_digest"], TASK_ONLY_V1_MANIFEST_DIGEST);
        assert_eq!(
            serde_json::from_value::<GatewayCapabilityProfileIdentity>(encoded)
                .expect("profile identity deserializes"),
            identity
        );
    }
}
