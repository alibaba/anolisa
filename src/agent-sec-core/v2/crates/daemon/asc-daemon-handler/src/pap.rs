//! PAP protocol adapter invoked by the daemon dispatcher.

use std::sync::Arc;

use asc_daemon_core::{PolicyAdministration, PolicyAdministrationError, Principal, ResourcePage};
use asc_daemon_protocol::method::{BindingMethod, PapMethod, PolicyMethod, ScopeMethod};
use asc_daemon_protocol::{
    CreateBindingParams, CreatePolicyParams, CreateScopeParams, DaemonResponse, ListParams,
    ListResult, RequestId, ResourceParams, RevisionParams, UpdateBindingParams, UpdatePolicyParams,
    UpdateScopeParams, error_code,
};

const MAX_PUBLIC_ERROR_BYTES: usize = 256;
const ERROR_MESSAGE_SUFFIX: &str = "...";

/// PAP-specific protocol adapter with repository/compiler types erased.
pub(super) struct PapHandler {
    application: Arc<dyn PolicyAdministration>,
}

impl PapHandler {
    pub(super) fn new(application: impl PolicyAdministration + 'static) -> Self {
        Self {
            application: Arc::new(application),
        }
    }

    pub(super) fn handle(
        &self,
        request_id: RequestId,
        principal: &Principal,
        method: PapMethod,
        params: serde_json::Value,
    ) -> DaemonResponse {
        match dispatch(method, params, self.application.as_ref(), principal) {
            // TODO(policy-response-bounds): full PreparedPolicy and BindingView results can exceed
            // the transport response limit after a mutation has committed. Before a durable
            // Repository is exposed, bound public result shapes and verify every mutation result
            // against the response budget before commit.
            Ok(result) => DaemonResponse::success(request_id, result),
            Err(PapDispatchError::BadRequest(message)) => {
                DaemonResponse::error(request_id, error_code::INVALID_REQUEST, &message)
            }
            Err(PapDispatchError::Application(error)) => {
                let (code, message) = project_application_error(&error);
                DaemonResponse::error(request_id, code, &message)
            }
            Err(PapDispatchError::Projection) => DaemonResponse::error(
                request_id,
                error_code::INTERNAL,
                "policy state could not be encoded",
            ),
        }
    }
}

fn dispatch(
    method: PapMethod,
    params: serde_json::Value,
    application: &dyn PolicyAdministration,
    principal: &Principal,
) -> Result<serde_json::Value, PapDispatchError> {
    match method {
        PapMethod::Policy(method) => dispatch_policy(method, params, application, principal),
        PapMethod::Scope(method) => dispatch_scope(method, params, application, principal),
        PapMethod::Binding(method) => dispatch_binding(method, params, application, principal),
    }
}

fn dispatch_policy(
    method: PolicyMethod,
    params: serde_json::Value,
    application: &dyn PolicyAdministration,
    principal: &Principal,
) -> Result<serde_json::Value, PapDispatchError> {
    match method {
        PolicyMethod::Create => {
            let input: CreatePolicyParams = decode(params)?;
            encode(application.create_policy(principal, &input.policy_name, &input.template)?)
        }
        PolicyMethod::Update => {
            let input: UpdatePolicyParams = decode(params)?;
            encode(application.update_policy(
                principal,
                &input.policy_id,
                &input.policy_name,
                &input.template,
            )?)
        }
        PolicyMethod::Get => {
            let input: RevisionParams = decode(params)?;
            encode(application.get_policy(principal, &input.id, input.revision)?)
        }
        PolicyMethod::List => {
            let input: ListParams = decode(params)?;
            encode_page(application.list_policies(principal, input.limit, input.offset)?)
        }
        PolicyMethod::Delete => {
            let input: RevisionParams = decode(params)?;
            encode(application.delete_policy_revision(principal, &input.id, input.revision)?)
        }
    }
}

fn dispatch_scope(
    method: ScopeMethod,
    params: serde_json::Value,
    application: &dyn PolicyAdministration,
    principal: &Principal,
) -> Result<serde_json::Value, PapDispatchError> {
    match method {
        ScopeMethod::Create => {
            let input: CreateScopeParams = decode(params)?;
            encode(application.create_scope(principal, &input.selector)?)
        }
        ScopeMethod::Update => {
            let input: UpdateScopeParams = decode(params)?;
            encode(application.update_scope(principal, &input.scope_id, &input.selector)?)
        }
        ScopeMethod::Get => {
            let input: RevisionParams = decode(params)?;
            encode(application.get_scope(principal, &input.id, input.revision)?)
        }
        ScopeMethod::List => {
            let input: ListParams = decode(params)?;
            encode_page(application.list_scopes(principal, input.limit, input.offset)?)
        }
        ScopeMethod::Delete => {
            let input: RevisionParams = decode(params)?;
            encode(application.delete_scope_revision(principal, &input.id, input.revision)?)
        }
    }
}

fn dispatch_binding(
    method: BindingMethod,
    params: serde_json::Value,
    application: &dyn PolicyAdministration,
    principal: &Principal,
) -> Result<serde_json::Value, PapDispatchError> {
    match method {
        BindingMethod::Create => {
            let input: CreateBindingParams = decode(params)?;
            encode(application.create_binding(
                principal,
                &input.policy_id,
                input.policy_revision,
                &input.scope_id,
                input.scope_revision,
            )?)
        }
        BindingMethod::Update => {
            let input: UpdateBindingParams = decode(params)?;
            encode(application.update_binding(
                principal,
                &input.binding_id,
                &input.policy_id,
                input.policy_revision,
                &input.scope_id,
                input.scope_revision,
            )?)
        }
        BindingMethod::Get => {
            let input: ResourceParams = decode(params)?;
            encode(application.get_binding(principal, &input.id)?)
        }
        BindingMethod::List => {
            let input: ListParams = decode(params)?;
            encode_page(application.list_bindings(principal, input.limit, input.offset)?)
        }
        BindingMethod::Delete => {
            let input: ResourceParams = decode(params)?;
            encode(application.delete_binding(principal, &input.id)?)
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, PapDispatchError> {
    serde_json::from_value(params)
        .map_err(|error| PapDispatchError::BadRequest(bounded_parameter_error(&error)))
}

fn bounded_parameter_error(error: &serde_json::Error) -> String {
    bounded_error_message(&error.to_string())
}

fn bounded_error_message(message: &str) -> String {
    if message.len() <= MAX_PUBLIC_ERROR_BYTES {
        return message.to_owned();
    }

    let mut end = MAX_PUBLIC_ERROR_BYTES - ERROR_MESSAGE_SUFFIX.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ERROR_MESSAGE_SUFFIX}", &message[..end])
}

fn encode<T: serde::Serialize>(value: T) -> Result<serde_json::Value, PapDispatchError> {
    serde_json::to_value(value).map_err(|_| PapDispatchError::Projection)
}

fn encode_page<T: serde::Serialize>(
    page: ResourcePage<T>,
) -> Result<serde_json::Value, PapDispatchError> {
    encode(ListResult {
        items: page.items,
        total: page.total,
    })
}

enum PapDispatchError {
    BadRequest(String),
    Application(PolicyAdministrationError),
    Projection,
}

impl From<PolicyAdministrationError> for PapDispatchError {
    fn from(value: PolicyAdministrationError) -> Self {
        Self::Application(value)
    }
}

fn project_application_error(error: &PolicyAdministrationError) -> (&'static str, String) {
    let code = match error {
        PolicyAdministrationError::Forbidden => error_code::PERMISSION_DENIED,
        PolicyAdministrationError::InvalidArgument(_) => error_code::INVALID_ARGUMENT,
        PolicyAdministrationError::Conflict | PolicyAdministrationError::OperationInProgress => {
            error_code::CONFLICT
        }
        PolicyAdministrationError::NotFound(_) => error_code::NOT_FOUND,
        PolicyAdministrationError::ResourceExhausted => error_code::RESOURCE_EXHAUSTED,
        PolicyAdministrationError::Internal => error_code::INTERNAL,
    };
    (code, bounded_error_message(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_errors_are_specific_but_bounded() {
        let invalid_limit = decode::<ListParams>(serde_json::json!({"limit": 0}));
        let Err(PapDispatchError::BadRequest(message)) = invalid_limit else {
            panic!("an invalid pagination limit must fail parameter decoding");
        };
        assert_eq!(message, "limit must be between 1 and 1000");

        let long_field = "x".repeat(MAX_PUBLIC_ERROR_BYTES * 2);
        let mut params = serde_json::Map::new();
        params.insert(long_field, serde_json::Value::Null);
        let oversized = decode::<ListParams>(serde_json::Value::Object(params));
        let Err(PapDispatchError::BadRequest(message)) = oversized else {
            panic!("an unknown parameter must fail decoding");
        };
        assert!(message.len() <= MAX_PUBLIC_ERROR_BYTES);
        assert!(message.ends_with("..."));
    }

    #[test]
    fn application_error_projection_is_complete_and_sanitized() {
        assert_eq!(
            project_application_error(&PolicyAdministrationError::Forbidden),
            (
                error_code::PERMISSION_DENIED,
                "principal is not authorized to administer policy".to_owned()
            )
        );
        assert_eq!(
            project_application_error(&PolicyAdministrationError::Internal),
            (
                error_code::INTERNAL,
                "policy state could not be processed".to_owned()
            )
        );
    }
}
