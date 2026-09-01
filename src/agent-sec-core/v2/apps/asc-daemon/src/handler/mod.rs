mod policy;

use asc_daemon_core::PolicyError;
use asc_daemon_protocol::{DaemonRequest, DaemonResponse, method};
use asc_policy_runtime::PolicyAdapter;
use tracing::warn;

use crate::state::AppState;

pub(crate) fn dispatch<A>(
    request_id: String,
    request: DaemonRequest,
    state: &AppState<A>,
) -> DaemonResponse
where
    A: PolicyAdapter + 'static,
{
    let requires_management_credential = method::metadata(&request.method)
        .is_some_and(|value| value.access == method::AccessPolicy::ManagementCredential);
    if requires_management_credential && !state.auth().verify(request.auth.as_ref()) {
        warn!(rpc_method = %request.method, "unauthenticated policy request rejected");
        return DaemonResponse::daemon_error(
            request_id,
            "unauthenticated",
            "policy management authentication required",
        );
    }
    if request.method == method::DAEMON_HEALTH {
        return DaemonResponse::success(request_id, serde_json::json!({"status": "ready"}));
    }
    let result = policy::dispatch(&request.method, request.params, state);
    match result {
        Ok((data, wake_worker)) => {
            if wake_worker {
                state.wake_policy_worker();
            }
            DaemonResponse::success(request_id, data)
        }
        Err(DispatchError::BadRequest) => {
            DaemonResponse::daemon_error(request_id, "bad_request", "method parameters are invalid")
        }
        Err(DispatchError::UnknownMethod) => DaemonResponse::daemon_error(
            request_id,
            "unknown_method",
            "daemon method is not implemented by this slice",
        ),
        Err(DispatchError::Domain(error)) => {
            let (code, message) = policy::project_error(&error);
            DaemonResponse::rejected(request_id, code, message)
        }
        Err(DispatchError::Serialization) => DaemonResponse::daemon_error(
            request_id,
            "internal_error",
            "daemon response projection failed",
        ),
    }
}

enum DispatchError {
    BadRequest,
    UnknownMethod,
    Domain(PolicyError),
    Serialization,
}

impl From<PolicyError> for DispatchError {
    fn from(value: PolicyError) -> Self {
        Self::Domain(value)
    }
}
