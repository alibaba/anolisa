use std::io::Write;
use std::sync::Arc;

use asc_daemon_core::{
    PeerCredentials, PolicyAdministration, Principal, PrincipalPolicy, PrincipalRole,
};
use asc_daemon_protocol::method::{self, AccessPolicy, MethodId};
use asc_daemon_protocol::{DaemonRequest, DaemonResponse, RequestId, error_code};
use asc_daemon_service::{DispatchError, DispatchRequest, RequestDispatcher, ResponseDisposition};

use crate::pap::PapHandler;

/// Protocol router composed over daemon application use cases.
pub struct DaemonDispatcher {
    pap: PapHandler,
    principal_policy: Arc<dyn PrincipalPolicy>,
}

impl DaemonDispatcher {
    /// Composes PAP dispatch with trusted server authorization policy.
    ///
    /// The role is process-owned configuration. It is never decoded from the
    /// request or inferred from caller-supplied attribution.
    pub fn new(
        application: impl PolicyAdministration + 'static,
        principal_policy: Arc<dyn PrincipalPolicy>,
    ) -> Self {
        Self {
            pap: PapHandler::new(application),
            principal_policy,
        }
    }

    /// Handles one decoded request using transport-authenticated peer identity.
    pub fn handle(
        &self,
        request_id: RequestId,
        peer: PeerCredentials,
        request: DaemonRequest,
    ) -> DaemonResponse {
        let Some(method_id) = method::resolve(&request.method) else {
            return DaemonResponse::error(
                request_id,
                error_code::UNKNOWN_METHOD,
                "daemon method is not implemented",
            );
        };

        let role = self.principal_policy.role_for(peer);
        let principal = Principal::from_authenticated_peer(peer, role);
        if !is_authorized(&principal, method_id.metadata().access) {
            return DaemonResponse::error(
                request_id,
                error_code::PERMISSION_DENIED,
                "principal is not authorized to administer policy",
            );
        }
        let MethodId::Pap(method) = method_id;
        self.pap
            .handle(request_id, &principal, method, request.params)
    }
}

fn is_authorized(principal: &Principal, access: AccessPolicy) -> bool {
    match access {
        AccessPolicy::PolicyAdministrator => principal.role() == PrincipalRole::PolicyAdministrator,
    }
}

impl RequestDispatcher for DaemonDispatcher {
    fn dispatch(
        &self,
        request: DispatchRequest,
        response: &mut dyn Write,
    ) -> Result<ResponseDisposition, DispatchError> {
        let request_id = new_request_id();
        if request.control.is_cancelled() {
            return write_response(
                response,
                &DaemonResponse::<serde_json::Value>::error(
                    request_id,
                    error_code::DEADLINE_EXCEEDED,
                    "request dispatch deadline expired",
                ),
            );
        }

        let peer = PeerCredentials::new(request.peer.uid(), request.peer.gid(), request.peer.pid());
        let Ok(decoded) = serde_json::from_slice::<DaemonRequest>(&request.payload) else {
            return write_response(
                response,
                &DaemonResponse::<serde_json::Value>::error(
                    request_id,
                    error_code::INVALID_REQUEST,
                    "request envelope is invalid",
                ),
            );
        };
        write_response(response, &self.handle(request_id, peer, decoded))
    }
}

pub(crate) fn new_request_id() -> RequestId {
    RequestId::new(uuid::Uuid::new_v4().to_string())
        .expect("UUID request identities are always non-empty")
}

pub(crate) fn write_response<T: serde::Serialize>(
    response: &mut dyn Write,
    value: &DaemonResponse<T>,
) -> Result<ResponseDisposition, DispatchError> {
    serde_json::to_writer(response, value).map_err(|_| DispatchError)?;
    Ok(ResponseDisposition::Send)
}
