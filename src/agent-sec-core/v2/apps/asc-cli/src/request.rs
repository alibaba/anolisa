use std::fs::File;
use std::io::Read as _;

use asc_daemon_protocol::{
    DeleteBindingParams, IdParams, ListParams, MAX_FRAME_BYTES, PutBindingParams, PutPolicyParams,
    PutScopeParams, RevisionParams, RevisionRefDto, ScopeSelectorDto, method,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::args::{
    BindingOperation, Cli, Command, PolicyResource, ScopeOperation, TemplateOperation,
};
use crate::error::CliError;

pub(crate) struct RpcRequest {
    pub(crate) method: &'static str,
    pub(crate) params: serde_json::Value,
}

pub(crate) fn from_cli(cli: Cli) -> Result<RpcRequest, CliError> {
    let Command::Policy { resource } = cli.command;
    match resource {
        PolicyResource::Template { operation } => template_request(operation),
        PolicyResource::Scope { operation } => scope_request(operation),
        PolicyResource::Binding { operation } => binding_request(operation),
    }
}

fn template_request(operation: TemplateOperation) -> Result<RpcRequest, CliError> {
    match operation {
        TemplateOperation::Put { file } => Ok(RpcRequest {
            method: method::POLICY_TEMPLATES_PUT,
            params: read_dto::<PutPolicyParams>(&file)?,
        }),
        TemplateOperation::Get { id, revision } => serialize(
            method::POLICY_TEMPLATES_GET,
            RevisionParams {
                id: id.to_string(),
                revision,
            },
        ),
        TemplateOperation::List { limit, offset } => {
            serialize(method::POLICY_TEMPLATES_LIST, ListParams { limit, offset })
        }
        TemplateOperation::Delete { id, revision } => serialize(
            method::POLICY_TEMPLATES_DELETE,
            RevisionParams {
                id: id.to_string(),
                revision,
            },
        ),
    }
}

fn scope_request(operation: ScopeOperation) -> Result<RpcRequest, CliError> {
    match operation {
        ScopeOperation::Put {
            scope_id,
            pid,
            cgroup_id,
        } => serialize(
            method::POLICY_SCOPES_PUT,
            PutScopeParams {
                scope_id,
                selector: match (pid, cgroup_id) {
                    (Some(pid), None) => ScopeSelectorDto::Pid { pid },
                    (None, Some(cgroup_id)) => ScopeSelectorDto::CgroupId { cgroup_id },
                    _ => return Err(CliError::InvalidInput),
                },
            },
        ),
        ScopeOperation::Get { id, revision } => serialize(
            method::POLICY_SCOPES_GET,
            RevisionParams {
                id: id.to_string(),
                revision,
            },
        ),
        ScopeOperation::List { limit, offset } => {
            serialize(method::POLICY_SCOPES_LIST, ListParams { limit, offset })
        }
        ScopeOperation::Delete { id, revision } => serialize(
            method::POLICY_SCOPES_DELETE,
            RevisionParams {
                id: id.to_string(),
                revision,
            },
        ),
    }
}

fn binding_request(operation: BindingOperation) -> Result<RpcRequest, CliError> {
    match operation {
        BindingOperation::Put {
            binding_id,
            policy_id,
            policy_revision,
            scope_id,
            scope_revision,
        } => serialize(
            method::POLICY_BINDINGS_PUT,
            PutBindingParams {
                binding_id,
                policy_ref: RevisionRefDto {
                    id: policy_id,
                    revision: policy_revision,
                },
                scope_ref: RevisionRefDto {
                    id: scope_id,
                    revision: scope_revision,
                },
            },
        ),
        BindingOperation::Get { id } => {
            serialize(method::POLICY_BINDINGS_GET, IdParams { id: id.to_string() })
        }
        BindingOperation::List { limit, offset } => {
            serialize(method::POLICY_BINDINGS_LIST, ListParams { limit, offset })
        }
        BindingOperation::Delete { id } => serialize(
            method::POLICY_BINDINGS_DELETE,
            DeleteBindingParams { binding_id: id },
        ),
    }
}

fn read_dto<T: DeserializeOwned + Serialize>(
    path: &std::path::Path,
) -> Result<serde_json::Value, CliError> {
    let file = File::open(path).map_err(|_| CliError::InputUnavailable)?;
    let mut bytes = Vec::new();
    file.take((MAX_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::InputUnavailable)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(CliError::InputTooLarge);
    }
    let input: T = serde_json::from_slice(&bytes).map_err(|_| CliError::InvalidInput)?;
    serde_json::to_value(input).map_err(|_| CliError::InvalidInput)
}

fn serialize<T: Serialize>(method: &'static str, params: T) -> Result<RpcRequest, CliError> {
    Ok(RpcRequest {
        method,
        params: serde_json::to_value(params).map_err(|_| CliError::InvalidInput)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::SystemTime;

    use clap::Parser as _;
    use serde_json::json;

    use super::*;

    #[test]
    fn template_put_preserves_the_shared_dto_without_defaults_or_lowering() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "asc-cli-template-put-{}-{nonce}.json",
            std::process::id()
        ));
        let input = json!({
            "policyName": "protect-production-secrets",
            "template": {
                "kind": "high_sensitivity_read_deny",
                "files": ["/secrets/**"]
            }
        });
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        let cli = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "template",
            "put",
            "--file",
            path.to_str().unwrap(),
        ])
        .unwrap();

        let request = from_cli(cli).unwrap();
        assert_eq!(request.method, method::POLICY_TEMPLATES_PUT);
        assert_eq!(request.params, input);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn template_delete_forwards_the_exact_identity_and_revision() {
        let cli = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "template",
            "delete",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
            "--revision",
            "7",
        ])
        .unwrap();

        let request = from_cli(cli).unwrap();
        assert_eq!(request.method, method::POLICY_TEMPLATES_DELETE);
        assert_eq!(
            request.params,
            json!({
                "id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
                "revision": 7
            })
        );
    }

    #[test]
    fn scope_put_and_delete_forward_only_the_shared_dto() {
        let put = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "scope",
            "put",
            "--pid",
            "4242",
        ])
        .unwrap();
        let request = from_cli(put).unwrap();
        assert_eq!(request.method, method::POLICY_SCOPES_PUT);
        assert_eq!(
            request.params,
            json!({"selector": {"kind": "pid", "pid": 4242}})
        );

        let delete = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "scope",
            "delete",
            "11111111-1111-4111-8111-111111111111",
            "--revision",
            "1",
        ])
        .unwrap();
        let request = from_cli(delete).unwrap();
        assert_eq!(request.method, method::POLICY_SCOPES_DELETE);
        assert_eq!(
            request.params,
            json!({
                "id": "11111111-1111-4111-8111-111111111111",
                "revision": 1
            })
        );
    }

    #[test]
    fn binding_put_and_delete_forward_only_the_shared_dto() {
        let put_input = json!({
            "policyRef": {
                "id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
                "revision": 1
            },
            "scopeRef": {
                "id": "11111111-1111-4111-8111-111111111111",
                "revision": 1
            }
        });
        let put = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "binding",
            "put",
            "--policy-id",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
            "--policy-revision",
            "1",
            "--scope-id",
            "11111111-1111-4111-8111-111111111111",
            "--scope-revision",
            "1",
        ])
        .unwrap();
        let request = from_cli(put).unwrap();
        assert_eq!(request.method, method::POLICY_BINDINGS_PUT);
        assert_eq!(request.params, put_input);

        let delete_input = json!({
            "bindingId": "22222222-2222-4222-8222-222222222222"
        });
        let delete = Cli::try_parse_from([
            "asc-cli",
            "--socket",
            "/run/daemon.sock",
            "--token-file",
            "/run/token",
            "policy",
            "binding",
            "delete",
            "22222222-2222-4222-8222-222222222222",
        ])
        .unwrap();
        let request = from_cli(delete).unwrap();
        assert_eq!(request.method, method::POLICY_BINDINGS_DELETE);
        assert_eq!(request.params, delete_input);
    }

    #[test]
    fn list_commands_forward_the_shared_query_dto() {
        for (resource, expected_method) in [
            ("template", method::POLICY_TEMPLATES_LIST),
            ("scope", method::POLICY_SCOPES_LIST),
            ("binding", method::POLICY_BINDINGS_LIST),
        ] {
            let cli = Cli::try_parse_from([
                "asc-cli",
                "--socket",
                "/run/daemon.sock",
                "--token-file",
                "/run/token",
                "policy",
                resource,
                "list",
                "--limit",
                "25",
                "--offset",
                "50",
            ])
            .unwrap();
            let request = from_cli(cli).unwrap();
            assert_eq!(request.method, expected_method);
            assert_eq!(request.params, json!({"limit": 25, "offset": 50}));
        }
    }
}
