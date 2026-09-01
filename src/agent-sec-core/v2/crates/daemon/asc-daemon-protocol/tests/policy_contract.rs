use asc_daemon_protocol::{
    DaemonRequest, DaemonResponse, DeleteBindingParams, IdParams, ListParams, PolicyTemplateDto,
    PutBindingParams, PutPolicyParams, PutScopeParams, RevisionParams, method,
};
use serde_json::json;

#[test]
fn policy_template_is_strict_and_round_trips() {
    let value = json!({
        "policyName": "high-sensitivity-read",
        "template": {
            "kind": "high_sensitivity_read_deny",
            "files": ["/secrets/**"]
        }
    });
    let decoded: PutPolicyParams = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);

    let mut unknown = value.clone();
    unknown["template"]["targetDsl"] = json!("not-allowed");
    assert!(serde_json::from_value::<PutPolicyParams>(unknown).is_err());

    let mut caller_allocated_revision = json!({
        "policyName": "high-sensitivity-read",
        "template": {
            "kind": "high_sensitivity_read_deny",
            "files": ["/secrets/**"]
        }
    });
    caller_allocated_revision["revision"] = json!(1);
    assert!(serde_json::from_value::<PutPolicyParams>(caller_allocated_revision).is_err());

    let mut missing_name = value;
    missing_name.as_object_mut().unwrap().remove("policyName");
    assert!(serde_json::from_value::<PutPolicyParams>(missing_name).is_err());

    let mut update = json!({
        "policyName": "high-sensitivity-read",
        "template": {
            "kind": "high_sensitivity_read_deny",
            "files": ["/secrets/**"]
        }
    });
    update["policyId"] = json!("6efed5ea-47c9-4b14-8e86-888f2ad88fc7");
    let decoded: PutPolicyParams = serde_json::from_value(update.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), update);

    update["policyId"] = json!("prevent-file-deletion");
    assert!(serde_json::from_value::<PutPolicyParams>(update).is_err());
}

#[test]
fn policy_delete_has_no_implicit_target() {
    let valid = json!({
        "id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
        "revision": 1
    });
    serde_json::from_value::<RevisionParams>(valid.clone()).unwrap();

    for invalid in [
        json!({}),
        json!({"id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7"}),
        json!({"revision": 1}),
        json!({
            "id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
            "revision": 1,
            "all": true
        }),
    ] {
        assert!(serde_json::from_value::<RevisionParams>(invalid).is_err());
    }
}

#[test]
fn binding_dto_has_one_policy_and_no_kernel_or_adapter_fields() {
    let value = json!({
        "policyRef": {"id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7", "revision": 1},
        "scopeRef": {"id": "11111111-1111-4111-8111-111111111111", "revision": 1}
    });
    serde_json::from_value::<PutBindingParams>(value.clone()).unwrap();

    for forbidden in [
        "pid",
        "cgroupId",
        "namespaceId",
        "adapter",
        "policies",
        "bindingRevision",
        "operationId",
        "executionDomainId",
        "expectedBindingRevision",
    ] {
        let mut changed = value.clone();
        changed[forbidden] = json!(1);
        assert!(serde_json::from_value::<PutBindingParams>(changed).is_err());
    }
}

#[test]
fn scope_dto_accepts_one_simple_selector_and_no_caller_revision() {
    for value in [
        json!({"selector": {"kind": "pid", "pid": 4242}}),
        json!({
            "scopeId": "11111111-1111-4111-8111-111111111111",
            "selector": {"kind": "cgroup_id", "cgroupId": 99}
        }),
    ] {
        serde_json::from_value::<PutScopeParams>(value).unwrap();
    }

    let value = json!({"selector": {"kind": "pid", "pid": 4242}});
    for forbidden in ["revision", "template", "executionDomainId"] {
        let mut changed = value.clone();
        changed[forbidden] = json!(1);
        assert!(serde_json::from_value::<PutScopeParams>(changed).is_err());
    }
}

#[test]
fn envelope_and_domain_response_layers_are_distinct() {
    let envelope = json!({
        "method": "policy.templates.list",
        "params": {},
        "auth": {"scheme": "bearer", "token": "secret"}
    });
    serde_json::from_value::<DaemonRequest>(envelope.clone()).unwrap();
    let mut unknown = envelope;
    unknown["callerUid"] = json!(1000);
    assert!(serde_json::from_value::<DaemonRequest>(unknown).is_err());

    let rejected = DaemonResponse::rejected(
        "request-1".to_owned(),
        "conflict",
        "immutable revision conflict",
    );
    assert!(rejected.ok);
    assert_eq!(rejected.exit_code, 1);
    assert!(rejected.error.is_none());
}

#[test]
fn all_policy_variants_are_product_level() {
    let value = json!({
        "kind": "low_sensitivity_egress",
        "files": ["/data/**"],
        "trustedDestinations": [{"type": "host", "pattern": "api.example.com", "ports": [443]}]
    });
    let template: PolicyTemplateDto = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(template).unwrap(), value);
}

#[test]
fn every_public_method_has_a_strict_request_fixture() {
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../fixtures/daemon/policy-methods.json"
    ))
    .unwrap();
    assert_eq!(fixtures.len(), 13);
    for fixture in fixtures {
        let method_name = fixture["method"].as_str().unwrap();
        let params = fixture["params"].clone();
        match method_name {
            method::DAEMON_HEALTH => assert_eq!(params, json!({})),
            method::POLICY_TEMPLATES_PUT => {
                serde_json::from_value::<PutPolicyParams>(params).unwrap();
            }
            method::POLICY_SCOPES_PUT => {
                serde_json::from_value::<PutScopeParams>(params).unwrap();
            }
            method::POLICY_BINDINGS_PUT => {
                serde_json::from_value::<PutBindingParams>(params).unwrap();
            }
            method::POLICY_BINDINGS_DELETE => {
                serde_json::from_value::<DeleteBindingParams>(params).unwrap();
            }
            method::POLICY_TEMPLATES_GET
            | method::POLICY_TEMPLATES_DELETE
            | method::POLICY_SCOPES_GET
            | method::POLICY_SCOPES_DELETE => {
                serde_json::from_value::<RevisionParams>(params).unwrap();
            }
            method::POLICY_BINDINGS_GET => {
                serde_json::from_value::<IdParams>(params).unwrap();
            }
            method::POLICY_TEMPLATES_LIST
            | method::POLICY_SCOPES_LIST
            | method::POLICY_BINDINGS_LIST => {
                serde_json::from_value::<ListParams>(params).unwrap();
            }
            unexpected => panic!("unregistered fixture method {unexpected}"),
        }
        assert!(fixture["expectedDisposition"].is_string());
    }
}

#[test]
fn method_registry_uses_an_explicit_capability_and_access_policy() {
    let policy = method::metadata(method::POLICY_TEMPLATES_PUT).unwrap();
    assert_eq!(policy.capability, method::Capability::Policy);
    assert_eq!(policy.access, method::AccessPolicy::ManagementCredential);

    let health = method::metadata(method::DAEMON_HEALTH).unwrap();
    assert_eq!(health.capability, method::Capability::Health);
    assert_eq!(health.access, method::AccessPolicy::Public);

    assert!(method::metadata("policy.unregistered").is_none());
    assert!(!method::is_policy("policy.unregistered"));
    assert!(method::metadata("policy.operations.get").is_none());
    assert!(method::metadata("policy.operations.list").is_none());
}
