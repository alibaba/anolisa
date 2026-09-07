use asc_policy_types::Validate;
use asc_policy_types::binding::{BindingStatus, BindingView, PreparedBinding};
use asc_policy_types::identifiers::Revision;

const COMPLETE_BINDING: &str = include_str!("fixtures/prepared-binding.json");

fn prepared_binding() -> PreparedBinding {
    serde_json::from_str(COMPLETE_BINDING).expect("complete Binding fixture must deserialize")
}

#[test]
fn complete_binding_round_trips_and_validates_as_one_boundary_document() {
    let expected: serde_json::Value = serde_json::from_str(COMPLETE_BINDING).unwrap();
    let binding = prepared_binding();

    binding.validate().unwrap();
    assert_eq!(binding.binding_revision.get(), 7);
    assert_eq!(binding.policy.revision.get(), 1);
    assert_eq!(binding.scope.revision.get(), 3);
    assert_eq!(serde_json::to_value(binding).unwrap(), expected);
}

#[test]
fn binding_validation_rejects_inconsistent_embedded_policy_identity() {
    let mut binding = prepared_binding();
    binding.policy.canonical_policy.revision = Revision::new(2).unwrap();

    let error = binding.validate().unwrap_err();
    assert_eq!(error.path, "policy.canonicalPolicy.revision");
}

#[test]
fn binding_validation_addresses_invalid_scope_fields() {
    let mut binding: serde_json::Value = serde_json::from_str(COMPLETE_BINDING).unwrap();
    binding["scope"]["selector"]["pid"] = serde_json::json!(0);
    let binding: PreparedBinding = serde_json::from_value(binding).unwrap();

    let error = binding.validate().unwrap_err();
    assert_eq!(error.path, "scope.selector.pid");
}

#[test]
fn binding_view_exposes_status_without_duplicate_spec_identity() {
    let spec = prepared_binding();
    let view = BindingView {
        status: BindingStatus::PendingApply,
        spec,
    };

    view.validate().unwrap();
    let wire = serde_json::to_value(&view).unwrap();
    assert!(wire.get("lifecycle").is_none());
    assert_eq!(wire["status"], "PENDING_APPLY");
    assert!(wire["spec"].get("desiredState").is_none());
    assert_eq!(serde_json::from_value::<BindingView>(wire).unwrap(), view);
}

#[test]
fn binding_lifecycle_separates_new_requests_from_worker_transitions() {
    let spec = prepared_binding();
    let pending_apply = BindingStatus::PendingApply;
    assert_eq!(spec.binding_revision.get(), 7);
    assert!(!pending_apply.is_terminal());
    assert!(pending_apply.complete_reconcile().is_err());

    let applying = pending_apply.start_reconcile().unwrap();
    pending_apply.validate_successor(applying).unwrap();
    assert_eq!(applying, BindingStatus::Applying);

    let retry_apply = applying.retry_reconcile().unwrap();
    applying.validate_successor(retry_apply).unwrap();
    assert_eq!(retry_apply, BindingStatus::PendingApply);
    let applying = retry_apply.start_reconcile().unwrap();
    let apply_failed = applying.fail_reconcile().unwrap();
    assert_eq!(apply_failed, BindingStatus::ApplyFailed);
    assert!(apply_failed.is_terminal());

    let retried_apply = apply_failed.request_apply().unwrap();
    assert_eq!(retried_apply, BindingStatus::PendingApply);
    assert!(apply_failed.validate_successor(retried_apply).is_err());
    let applying = retried_apply.start_reconcile().unwrap();
    let ready = applying.complete_reconcile().unwrap();
    assert_eq!(ready, BindingStatus::Ready);
    assert!(ready.is_terminal());
    assert_eq!(
        ready.request_apply().unwrap(),
        ready,
        "an identical PUT after successful Apply is idempotent"
    );

    let pending_delete = ready.request_delete().unwrap();
    assert_eq!(pending_delete, BindingStatus::PendingDelete);
    assert!(ready.validate_successor(pending_delete).is_err());
    let deleting = pending_delete.start_reconcile().unwrap();
    assert_eq!(deleting, BindingStatus::Deleting);
    assert!(deleting.request_apply().is_err());
    assert!(applying.request_delete().is_err());

    let retry_delete = deleting.retry_reconcile().unwrap();
    assert_eq!(retry_delete, BindingStatus::PendingDelete);
    let deleting = retry_delete.start_reconcile().unwrap();
    let delete_failed = deleting.fail_reconcile().unwrap();
    assert_eq!(delete_failed, BindingStatus::DeleteFailed);
    assert!(delete_failed.is_terminal());

    let retried_delete = delete_failed.request_delete().unwrap();
    assert_eq!(retried_delete, BindingStatus::PendingDelete);
    assert!(delete_failed.validate_successor(retried_delete).is_err());
    let deleted = retried_delete
        .start_reconcile()
        .unwrap()
        .complete_reconcile()
        .unwrap();
    assert_eq!(deleted, BindingStatus::Deleted);
    assert!(deleted.is_terminal());
    assert_eq!(deleted.request_delete().unwrap(), deleted);
}

#[test]
fn legacy_read_fields_are_not_reemitted_and_unknown_fields_are_rejected() {
    let mut legacy: serde_json::Value = serde_json::from_str(COMPLETE_BINDING).unwrap();
    legacy["policy"]["retired"] = serde_json::json!(false);
    legacy["scope"]["retired"] = serde_json::json!(false);
    legacy["executionDomainId"] = serde_json::json!("legacy-domain");

    let binding: PreparedBinding = serde_json::from_value(legacy).unwrap();
    let current = serde_json::to_value(binding).unwrap();
    assert!(current["policy"].get("retired").is_none());
    assert!(current["scope"].get("retired").is_none());
    assert!(current.get("executionDomainId").is_none());

    let mut unknown: serde_json::Value = serde_json::from_str(COMPLETE_BINDING).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PreparedBinding>(unknown).is_err());

    let mut lifecycle_inside_spec: serde_json::Value =
        serde_json::from_str(COMPLETE_BINDING).unwrap();
    lifecycle_inside_spec["desiredState"] = serde_json::json!("READY");
    assert!(serde_json::from_value::<PreparedBinding>(lifecycle_inside_spec).is_err());
}
