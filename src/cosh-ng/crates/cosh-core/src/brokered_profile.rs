//! Private brokered-profile handshake values owned by the Core binary.
//!
//! Gateway remains authoritative for admission. Core mirrors only the closed
//! private wire values it must reject before accepting a brokered user turn.

use serde::{Deserialize, Serialize};

const TASK_ONLY_V1_PROFILE: &str = "task-only-v1";
const TASK_ONLY_V1_MANIFEST_DIGEST: &str =
    "2b95e0f3e28df8eb2b7930f2dec3650ffe399f971671c971865e4663c382c94a";
const TASK_ONLY_V1_RUNTIME_TOOLS: &[&str] = &["ask_user_question"];

/// Profile identity carried by the private brokered Core wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokeredCapabilityProfileIdentity {
    /// Exact versioned profile name selected by Gateway.
    pub profile_id: String,
    /// Exact canonical manifest digest selected by Gateway.
    pub manifest_digest: String,
}

impl BrokeredCapabilityProfileIdentity {
    /// Returns the only capability profile understood by private Core v3.
    pub fn task_only_v1() -> Self {
        Self {
            profile_id: TASK_ONLY_V1_PROFILE.to_owned(),
            manifest_digest: TASK_ONLY_V1_MANIFEST_DIGEST.to_owned(),
        }
    }

    /// Verifies that the requested identity is the closed private v3 profile.
    pub fn verify_task_only_v1(&self) -> Result<(), &'static str> {
        if self.profile_id != TASK_ONLY_V1_PROFILE {
            return Err("capability profile identity does not match the brokered profile");
        }
        if self.manifest_digest != TASK_ONLY_V1_MANIFEST_DIGEST {
            return Err("capability profile manifest digest does not match the brokered profile");
        }
        Ok(())
    }
}

/// Verifies the actual Core inventory against the private v3 task-only profile.
pub fn verify_task_only_runtime_tools(actual: &[String]) -> Result<(), &'static str> {
    if actual
        .iter()
        .map(String::as_str)
        .eq(TASK_ONLY_V1_RUNTIME_TOOLS.iter().copied())
    {
        Ok(())
    } else {
        Err("Runtime tool inventory does not match the brokered capability profile")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_only_identity_and_inventory_are_exact() {
        let identity = BrokeredCapabilityProfileIdentity::task_only_v1();
        assert_eq!(identity.profile_id, "task-only-v1");
        assert_eq!(identity.manifest_digest, TASK_ONLY_V1_MANIFEST_DIGEST);
        assert_eq!(identity.verify_task_only_v1(), Ok(()));
        assert_eq!(
            verify_task_only_runtime_tools(&["ask_user_question".to_owned()]),
            Ok(())
        );
    }

    #[test]
    fn task_only_identity_and_inventory_reject_drift() {
        let mut identity = BrokeredCapabilityProfileIdentity::task_only_v1();
        identity.manifest_digest = "0".repeat(64);
        assert!(identity.verify_task_only_v1().is_err());
        assert!(verify_task_only_runtime_tools(&[]).is_err());
        assert!(verify_task_only_runtime_tools(&[
            "ask_user_question".to_owned(),
            "shell".to_owned(),
        ])
        .is_err());
    }
}
