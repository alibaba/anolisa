use asc_daemon_core::PolicyError;
use asc_daemon_protocol::{
    DeleteBindingParams, IdParams, ListParams, MAX_FRAME_BYTES, PolicyTemplateDto,
    PutBindingParams, PutPolicyParams, PutScopeParams, RevisionParams, ScopeSelectorDto,
    TrustedDestinationDto, method,
};
use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{PapError, ScopeSelector};
use asc_policy_engine::{PolicyTemplate, TrustedDestination};
use asc_policy_runtime::{PolicyAdapter, RuntimeError};
use uuid::Uuid;

use super::DispatchError;
use crate::state::AppState;

const LIST_ITEMS_BYTE_BUDGET: usize = MAX_FRAME_BYTES - (64 * 1024);

#[allow(clippy::too_many_lines)] // one exhaustive capability allowlist is easier to audit
pub(super) fn dispatch<A>(
    method_name: &str,
    params: serde_json::Value,
    state: &AppState<A>,
) -> Result<(serde_json::Value, bool), DispatchError>
where
    A: PolicyAdapter + 'static,
{
    match method_name {
        method::POLICY_TEMPLATES_PUT => {
            let input: PutPolicyParams = decode(params)?;
            let policy_id = input
                .policy_id
                .map(|id| resource_id(id.to_string()))
                .transpose()?;
            let template = policy_template(input.template);
            let stored =
                state
                    .policy()
                    .put_policy(policy_id.as_ref(), &input.policy_name, &template)?;
            Ok((
                serde_json::json!({"disposition": "STORED", "policy": stored}),
                false,
            ))
        }
        method::POLICY_TEMPLATES_GET => {
            let input: RevisionParams = decode(params)?;
            let value = state
                .policy()
                .get_policy(&uuid_resource_id(input.id)?, revision(input.revision)?)?;
            json(value, false)
        }
        method::POLICY_TEMPLATES_LIST => {
            let input = list_params(params)?;
            json(
                state
                    .policy()
                    .list_policies(input.limit, input.offset, LIST_ITEMS_BYTE_BUDGET)?,
                false,
            )
        }
        method::POLICY_TEMPLATES_DELETE => {
            let input: RevisionParams = decode(params)?;
            let value = state
                .policy()
                .delete_policy_revision(&uuid_resource_id(input.id)?, revision(input.revision)?)?;
            Ok((
                serde_json::json!({"disposition": "DELETED", "policy": value}),
                false,
            ))
        }
        method::POLICY_SCOPES_PUT => {
            let input: PutScopeParams = decode(params)?;
            let scope_id = input
                .scope_id
                .map(|id| resource_id(id.to_string()))
                .transpose()?;
            let selector = match input.selector {
                ScopeSelectorDto::Pid { pid } => ScopeSelector::Pid { pid },
                ScopeSelectorDto::CgroupId { cgroup_id } => ScopeSelector::CgroupId { cgroup_id },
            };
            let value = state.policy().put_scope(scope_id.as_ref(), &selector)?;
            Ok((
                serde_json::json!({"disposition": "STORED", "scope": value}),
                false,
            ))
        }
        method::POLICY_SCOPES_GET => {
            let input: RevisionParams = decode(params)?;
            let value = state
                .policy()
                .get_scope(&uuid_resource_id(input.id)?, revision(input.revision)?)?;
            json(value, false)
        }
        method::POLICY_SCOPES_LIST => {
            let input = list_params(params)?;
            json(
                state
                    .policy()
                    .list_scopes(input.limit, input.offset, LIST_ITEMS_BYTE_BUDGET)?,
                false,
            )
        }
        method::POLICY_SCOPES_DELETE => {
            let input: RevisionParams = decode(params)?;
            let value = state
                .policy()
                .delete_scope_revision(&uuid_resource_id(input.id)?, revision(input.revision)?)?;
            Ok((
                serde_json::json!({"disposition": "DELETED", "scope": value}),
                false,
            ))
        }
        method::POLICY_BINDINGS_PUT => {
            let input: PutBindingParams = decode(params)?;
            let binding_id = input
                .binding_id
                .map(|id| resource_id(id.to_string()))
                .transpose()?;
            let policy_id = uuid_resource_id(input.policy_ref.id.to_string())?;
            let scope_id = resource_id(input.scope_ref.id.to_string())?;
            let binding = state.policy().put_binding(
                binding_id.as_ref(),
                &policy_id,
                revision(input.policy_ref.revision)?,
                &scope_id,
                revision(input.scope_ref.revision)?,
            )?;
            Ok((
                serde_json::json!({
                    "disposition": "ACCEPTED",
                    "binding": binding
                }),
                true,
            ))
        }
        method::POLICY_BINDINGS_GET => {
            let input: IdParams = decode(params)?;
            json(
                state.policy().get_binding(&uuid_resource_id(input.id)?)?,
                false,
            )
        }
        method::POLICY_BINDINGS_LIST => {
            let input = list_params(params)?;
            json(
                state
                    .policy()
                    .list_bindings(input.limit, input.offset, LIST_ITEMS_BYTE_BUDGET)?,
                false,
            )
        }
        method::POLICY_BINDINGS_DELETE => {
            let input: DeleteBindingParams = decode(params)?;
            let binding = state
                .policy()
                .delete_binding(&resource_id(input.binding_id.to_string())?)?;
            Ok((
                serde_json::json!({
                    "disposition": "ACCEPTED",
                    "binding": binding
                }),
                true,
            ))
        }
        _ => Err(DispatchError::UnknownMethod),
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: serde_json::Value) -> Result<T, DispatchError> {
    serde_json::from_value(params).map_err(|_| DispatchError::BadRequest)
}

fn list_params(params: serde_json::Value) -> Result<ListParams, DispatchError> {
    let value: ListParams = decode(params)?;
    if value.limit == 0 || value.limit > 1000 {
        return Err(DispatchError::BadRequest);
    }
    Ok(value)
}

fn resource_id(value: String) -> Result<ResourceId, DispatchError> {
    ResourceId::new(value).map_err(|_| DispatchError::BadRequest)
}

fn uuid_resource_id(value: String) -> Result<ResourceId, DispatchError> {
    Uuid::parse_str(&value).map_err(|_| DispatchError::BadRequest)?;
    resource_id(value)
}

fn revision(value: u64) -> Result<Revision, DispatchError> {
    Revision::new(value).map_err(|_| DispatchError::BadRequest)
}

fn policy_template(value: PolicyTemplateDto) -> PolicyTemplate {
    match value {
        PolicyTemplateDto::HighSensitivityReadDeny { files } => {
            PolicyTemplate::HighSensitivityReadDeny { files }
        }
        PolicyTemplateDto::PreventFileDeletion { files } => {
            PolicyTemplate::PreventFileDeletion { files }
        }
        PolicyTemplateDto::LowSensitivityEgress {
            files,
            trusted_destinations,
        } => PolicyTemplate::LowSensitivityEgress {
            files,
            trusted_destinations: trusted_destinations
                .into_iter()
                .map(|destination| match destination {
                    TrustedDestinationDto::Host { pattern, ports } => {
                        TrustedDestination::Host { pattern, ports }
                    }
                    TrustedDestinationDto::Cidr { cidr, ports } => {
                        TrustedDestination::Cidr { cidr, ports }
                    }
                })
                .collect(),
        },
    }
}

fn json<T: serde::Serialize>(
    value: T,
    wake: bool,
) -> Result<(serde_json::Value, bool), DispatchError> {
    serde_json::to_value(value)
        .map(|value| (value, wake))
        .map_err(|_| DispatchError::Serialization)
}

pub(super) fn project_error(error: &PolicyError) -> (&'static str, &'static str) {
    match error {
        PolicyError::Pap(PapError::Conflict) => ("conflict", "immutable revision conflict"),
        PolicyError::Pap(PapError::NotFound) | PolicyError::Runtime(RuntimeError::NotFound) => {
            ("not_found", "requested policy resource was not found")
        }
        PolicyError::Pap(PapError::ResponseTooLarge)
        | PolicyError::Runtime(RuntimeError::ResponseTooLarge) => (
            "payload_too_large",
            "one policy resource exceeds the daemon response frame",
        ),
        PolicyError::Runtime(RuntimeError::Pap(PapError::NotFound)) => (
            "not_found",
            "referenced Policy or Scope revision was not found",
        ),
        PolicyError::Pap(
            PapError::InvalidScope(_)
            | PapError::InvalidIdentifier(_)
            | PapError::InvalidPolicyName(_)
            | PapError::Engine(_),
        ) => ("invalid_argument", "policy input failed domain validation"),
        PolicyError::Runtime(RuntimeError::IdempotencyConflict) => (
            "idempotency_conflict",
            "operation id was reused for a different request",
        ),
        PolicyError::Runtime(RuntimeError::PreconditionFailed) => (
            "precondition_failed",
            "Binding revision precondition failed",
        ),
        PolicyError::Pap(PapError::Serialization(_) | PapError::Persistence)
        | PolicyError::Runtime(
            RuntimeError::Serialization(_) | RuntimeError::Persistence | RuntimeError::Pap(_),
        ) => ("internal_error", "policy state could not be updated"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_projects_missing_reference_as_not_found() {
        let error = PolicyError::Runtime(RuntimeError::Pap(PapError::NotFound));
        let (code, message) = project_error(&error);
        assert_eq!(code, "not_found");
        assert_eq!(message, "referenced Policy or Scope revision was not found");
    }

    #[test]
    fn oversized_page_projects_a_stable_payload_error() {
        for error in [
            PolicyError::Pap(PapError::ResponseTooLarge),
            PolicyError::Runtime(RuntimeError::ResponseTooLarge),
        ] {
            let (code, message) = project_error(&error);
            assert_eq!(code, "payload_too_large");
            assert_eq!(
                message,
                "one policy resource exceeds the daemon response frame"
            );
        }
    }
}
