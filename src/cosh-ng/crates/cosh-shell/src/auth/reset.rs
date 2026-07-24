//! Pure decision logic for the credential-reset / configure-submission flow.
//!
//! These helpers take primitive inputs (or the public `AuthResponse`) so they
//! can be unit-tested without an `InlineState`, a rendered panel, or a spawned
//! cosh-core process. `runtime.rs` keeps only the orchestration that wires them
//! to state transitions and rendering.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::runtime::prelude::AuthResponse;

#[derive(Debug)]
pub(crate) enum CoreAuthConfigureError {
    ResetRequired,
    Other(String),
}

/// The registry error string that means "encrypted credentials are unreadable;
/// resetting them is required before this configuration can be saved". This is
/// the only signal the shell turns into its reset confirmation, so the mapping
/// must stay in lockstep with cosh-core's `credential_reset_required`.
pub(crate) const CREDENTIAL_RESET_REQUIRED_SIGNAL: &str = "credential_reset_required";

pub(crate) fn classify_core_configure_error(error: String) -> CoreAuthConfigureError {
    if error == CREDENTIAL_RESET_REQUIRED_SIGNAL {
        CoreAuthConfigureError::ResetRequired
    } else {
        CoreAuthConfigureError::Other(error)
    }
}

/// Builds the registry `configure` params for a response. Split out so the
/// serialized request (notably the `reset_unavailable_credentials` flag on a
/// reset resubmission) can be asserted without spawning cosh-core.
pub(crate) fn configure_params(response: &AuthResponse) -> Value {
    json!({
        "provider_id": response.provider_id,
        "provider_type": response.provider_type,
        "values": response.values,
        "reset_unavailable_credentials": response.reset_unavailable_credentials,
    })
}

/// True when a submission carries a credential that will be encrypted and
/// persisted (api_key or manual Aliyun AK/SK/token). Aliyun ECS RAM role and
/// other credential-less submissions write nothing to encrypt, so they must not
/// force a reset of unrelated providers' unreadable credentials.
pub(crate) fn values_write_encryptable_credentials(values: &HashMap<String, String>) -> bool {
    if values.get("auth_source").map(String::as_str) == Some("ecs_ram_role") {
        return false;
    }
    [
        "api_key",
        "access_key_id",
        "access_key_secret",
        "security_token",
    ]
    .iter()
    .any(|key| {
        values
            .get(*key)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// ActiveRun confirms a reset before submitting only when unreadable
/// credentials exist AND this submission writes an encryptable credential that
/// would need the salt. CoreRegistry never pre-confirms — it relies on the
/// server's `credential_reset_required` signal.
pub(crate) fn should_confirm_reset_before_submit(
    is_active_run: bool,
    credentials_unavailable: bool,
    reset_already_confirmed: bool,
    submits_encryptable_credentials: bool,
) -> bool {
    is_active_run
        && credentials_unavailable
        && submits_encryptable_credentials
        && !reset_already_confirmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_reset_required_signal_maps_to_reset_required() {
        // The shell must recognise the exact string cosh-core emits (asserted by
        // the registry integration test) as the reset-required signal.
        assert!(matches!(
            classify_core_configure_error("credential_reset_required".to_string()),
            CoreAuthConfigureError::ResetRequired
        ));
        assert!(matches!(
            classify_core_configure_error("failed to persist config: disk full".to_string()),
            CoreAuthConfigureError::Other(_)
        ));
    }

    #[test]
    fn configure_params_carry_reset_flag() {
        let response = AuthResponse {
            request_id: "req-1".into(),
            provider_id: "home-provider".into(),
            provider_type: Some("dashscope".into()),
            values: HashMap::from([("api_key".to_string(), "sk-real".to_string())]),
            persist: true,
            reset_unavailable_credentials: true,
        };
        let params = configure_params(&response);
        assert_eq!(params["provider_id"], "home-provider");
        assert_eq!(params["reset_unavailable_credentials"], true);
    }

    #[test]
    fn ecs_ram_role_submission_writes_no_encryptable_credentials() {
        let ecs = HashMap::from([("auth_source".to_string(), "ecs_ram_role".to_string())]);
        assert!(!values_write_encryptable_credentials(&ecs));

        let api_key = HashMap::from([("api_key".to_string(), "sk-real".to_string())]);
        assert!(values_write_encryptable_credentials(&api_key));

        // A whitespace-only credential is semantically empty.
        let blank = HashMap::from([("api_key".to_string(), "   ".to_string())]);
        assert!(!values_write_encryptable_credentials(&blank));
    }

    #[test]
    fn reset_confirmation_only_for_active_run_writing_credentials() {
        // CoreRegistry never pre-confirms: it relies on the server's signal.
        assert!(!should_confirm_reset_before_submit(
            false, true, false, true
        ));
        // ActiveRun confirms when this submission writes encryptable credentials.
        assert!(should_confirm_reset_before_submit(true, true, false, true));
        // A credential-less submission (e.g. Aliyun ECS RAM role) must not force
        // a reset of unrelated unreadable credentials.
        assert!(!should_confirm_reset_before_submit(
            true, true, false, false
        ));
        // Nothing to reset when credentials are readable or already confirmed.
        assert!(!should_confirm_reset_before_submit(
            true, false, false, true
        ));
        assert!(!should_confirm_reset_before_submit(true, true, true, true));
    }
}
