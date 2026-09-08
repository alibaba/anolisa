use aw_contracts::{canonical, registry::SCHEMAS, validation::AdoptionCheck, Registry};
use serde_json::{json, Value};
use std::sync::LazyLock;

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry::new().unwrap());
fn fixtures() -> Value {
    canonical::parse(include_bytes!("fixtures/contracts.json")).unwrap()
}
fn result(f: &Value) -> Result<(), aw_contracts::Error> {
    REGISTRY.validate_result(
        &f["capability-invocation-v1"],
        &f["provider-receipt-v1"],
        Some(&f["context-projection-prepare-output-v2"]),
    )
}
fn rebind_result(f: &mut Value) {
    let output = f["context-projection-prepare-output-v2"].clone();
    f["provider-receipt-v1"]["output"]["digest"] =
        json!(canonical::document_digest(&output).unwrap());
    f["provider-receipt-v1"]["output"]["bytes"] = json!(canonical::bytes(&output).unwrap().len());
    f["context-adoption-v1"]["candidate_digest"] =
        f["provider-receipt-v1"]["output"]["digest"].clone();
    f["context-adoption-v1"]["receipt_digest"] =
        json!(canonical::document_digest(&f["provider-receipt-v1"]).unwrap());
}
fn adoption(
    f: &Value,
    effective: Option<&str>,
    recovered: Option<&str>,
) -> Result<Option<u64>, aw_contracts::Error> {
    REGISTRY.validate_adoption(AdoptionCheck {
        invocation: &f["capability-invocation-v1"],
        receipt: &f["provider-receipt-v1"],
        output: f["provider-receipt-v1"]
            .get("output")
            .map(|_| &f["context-projection-prepare-output-v2"]),
        observation: &f["context-adoption-v1"],
        boundary: &f["boundary-descriptor-v1"],
        effective_text: effective,
        recovered_text: recovered,
        now_ms: 1500,
    })
}
fn preserve(f: &mut Value, reason: &str) {
    let obs = &mut f["context-adoption-v1"];
    obs["decision"] = json!("preserved");
    obs["reason"] = json!(reason);
    obs["effective_digest"] = json!(canonical::digest(b"alpha alpha alpha\n"));
    obs["effective_bytes"] = json!(18);
}

#[test]
fn every_payload_schema_has_a_valid_example_and_rejects_unknown_fields() {
    let f = fixtures();
    assert_eq!(f.as_object().unwrap().len(), SCHEMAS.len() - 1);
    for (name, _) in SCHEMAS {
        if *name == "common-v1" {
            continue;
        }
        REGISTRY
            .validate(name, &f[*name])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut bad = f[*name].clone();
        bad["unknown"] = json!(true);
        assert!(REGISTRY.validate(name, &bad).is_err(), "{name}");
    }
    assert!(REGISTRY.validate("unknown-v1", &json!({})).is_err());
}

#[test]
fn canonical_wire_vectors_and_ambiguous_inputs() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/canonical-vectors.json")).unwrap();
    for v in vectors.as_array().unwrap() {
        assert_eq!(
            canonical::bytes(&v["input"]).unwrap(),
            v["canonical"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            canonical::document_digest(&v["input"]).unwrap(),
            v["digest"]
        );
    }
    for bad in [
        r#"{"a":1,"a":2}"#,
        r#"{"x":{"a":1,"\u0061":2}}"#,
        "1.0",
        "1e0",
        "-0",
        "9007199254740992",
        "-9007199254740992",
        r#"{"中":1}"#,
        "{}{}",
        r#""\ud800""#,
    ] {
        assert!(canonical::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    let deep = format!("{}0{}", "[".repeat(34), "]".repeat(34));
    assert!(canonical::parse(deep.as_bytes()).is_err());
    assert!(canonical::parse(&vec![b' '; canonical::MAX_DOCUMENT_BYTES + 1]).is_err());
}

#[test]
fn admits_current_binding_and_rejects_stale_or_unsupported_descriptors() {
    let f = fixtures();
    let check = |f: &Value| {
        REGISTRY.validate_invocation(
            &f["capability-invocation-v1"],
            &f["provider-descriptor-v1"],
            &f["boundary-descriptor-v1"],
            &f["runtime-binding-v1"],
            1000,
        )
    };
    check(&f).unwrap();
    for (doc, field, value) in [
        ("runtime-binding-v1", "generation", json!(2)),
        ("runtime-binding-v1", "session_id", json!("old-session")),
        ("runtime-binding-v1", "state", json!("exited")),
        ("boundary-descriptor-v1", "revision", json!(2)),
        (
            "boundary-descriptor-v1",
            "invocation_mode",
            json!("observe_only"),
        ),
        ("capability-invocation-v1", "deadline_at_ms", json!(999)),
        (
            "provider-descriptor-v1",
            "manifest_digest",
            json!("0".repeat(64)),
        ),
    ] {
        let mut bad = f.clone();
        bad[doc][field] = value;
        assert!(check(&bad).is_err(), "{doc}.{field}");
    }
    let mut bad = f.clone();
    bad["provider-descriptor-v1"]["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(f["provider-descriptor-v1"]["capabilities"][0].clone());
    assert!(check(&bad).is_err());
    let mut bad = f.clone();
    bad["capability-invocation-v1"]["input_schema"]["digest"] = json!("0".repeat(64));
    assert!(check(&bad).is_err());
}

#[test]
fn result_correlation_and_budgets_reject_false_success() {
    let f = fixtures();
    result(&f).unwrap();
    for field in [
        "invocation_id",
        "provider_id",
        "provider_version",
        "input_digest",
        "manifest_digest",
    ] {
        let mut bad = f.clone();
        bad["provider-receipt-v1"][field] = json!("0".repeat(64));
        assert!(result(&bad).is_err(), "{field}");
    }
    let mut bad = f.clone();
    bad["provider-receipt-v1"]["scope"]["session_id"] = json!("another");
    assert!(result(&bad).is_err());
    let mut bad = f.clone();
    bad["capability-invocation-v1"]["budget"]["output_bytes"] = json!(1);
    assert!(result(&bad).is_err());
    let mut bad = f.clone();
    bad["context-projection-prepare-output-v2"]["candidate"]["source_digest"] =
        json!("0".repeat(64));
    rebind_result(&mut bad);
    assert!(result(&bad).is_err());
    let mut bad = f.clone();
    bad["provider-receipt-v1"]["completed_at_ms"] = json!(3000);
    assert!(result(&bad).is_err());
    let mut bad = f.clone();
    let meter = json!({"meter_id":"bytes", "unit":"bytes", "measurement_kind":"observed", "method":"utf8/v1", "value":18});
    bad["provider-receipt-v1"]["meters"] = json!([meter, meter]);
    assert!(result(&bad).is_err());
}

#[test]
fn adoption_requires_effective_text_recovery_and_supported_proof() {
    let f = fixtures();
    assert_eq!(
        adoption(&f, Some("alpha*3"), Some("alpha alpha alpha\n")).unwrap(),
        Some(11)
    );
    assert!(adoption(&f, Some("alpha*3"), None).is_err());
    assert!(adoption(&f, Some("alpha*3"), Some("wrong source")).is_err());
    assert!(adoption(
        &f,
        Some("different final text"),
        Some("alpha alpha alpha\n")
    )
    .is_err());
    let mut bad = f.clone();
    bad["context-adoption-v1"]["proof"]["boundary"] = json!("model_request");
    assert!(adoption(&bad, Some("alpha*3"), Some("alpha alpha alpha\n")).is_err());
    let mut bad = f.clone();
    bad["context-adoption-v1"]["ledger_status"] = json!("unavailable");
    bad["context-adoption-v1"]
        .as_object_mut()
        .unwrap()
        .remove("ledger_evidence");
    assert!(adoption(&bad, Some("alpha*3"), Some("alpha alpha alpha\n")).is_err());
    bad["boundary-descriptor-v1"]["ledger_policy"] = json!("best_effort");
    assert_eq!(
        adoption(&bad, Some("alpha*3"), Some("alpha alpha alpha\n")).unwrap(),
        None
    );
}

#[test]
fn recovery_reference_must_match_source_and_survive_until_adoption() {
    let mut f = fixtures();
    let digest = f["context-projection-prepare-output-v2"]["candidate"]["source_digest"].clone();
    f["context-projection-prepare-output-v2"]["candidate"]["reversibility"] = json!("retrievable");
    f["context-projection-prepare-output-v2"]["candidate"]["recovery"] = json!({"mode":"external", "resolver_id":"fixture/v1", "reference":"ref-1", "source_digest":digest, "expires_at_ms":1400});
    rebind_result(&mut f);
    assert_eq!(
        adoption(&f, Some("alpha*3"), Some("alpha alpha alpha\n")).unwrap(),
        Some(11)
    );
    f["context-projection-prepare-output-v2"]["candidate"]["recovery"]["expires_at_ms"] =
        json!(1300);
    rebind_result(&mut f);
    assert!(adoption(&f, Some("alpha*3"), Some("alpha alpha alpha\n")).is_err());
    preserve(&mut f, "recovery_unavailable");
    assert_eq!(
        adoption(&f, Some("alpha alpha alpha\n"), None).unwrap(),
        Some(0)
    );
}

#[test]
fn preservation_and_later_transform_do_not_count_candidate_savings() {
    let mut f = fixtures();
    preserve(&mut f, "environment_rejected");
    assert_eq!(
        adoption(&f, Some("alpha alpha alpha\n"), None).unwrap(),
        Some(0)
    );
    f["context-adoption-v1"]["reason"] = json!("no_savings");
    assert!(adoption(&f, Some("alpha alpha alpha\n"), None).is_err());
    f["context-projection-prepare-output-v2"]["candidate"]["content"] =
        json!("alpha alpha alpha\n");
    rebind_result(&mut f);
    assert_eq!(
        adoption(&f, Some("alpha alpha alpha\n"), None).unwrap(),
        Some(0)
    );
    f["context-adoption-v1"]["decision"] = json!("overridden");
    f["context-adoption-v1"]["reason"] = json!("later_transform");
    f["context-adoption-v1"]["effective_digest"] = json!(canonical::digest(b"later"));
    f["context-adoption-v1"]["effective_bytes"] = json!(5);
    assert_eq!(adoption(&f, Some("later"), None).unwrap(), Some(0));
}

#[test]
fn unverified_and_failed_provider_records_cannot_create_savings() {
    let mut f = fixtures();
    let obs = &mut f["context-adoption-v1"];
    obs["decision"] = json!("unverified");
    obs["reason"] = json!("proof_unavailable");
    for key in ["proof", "effective_digest", "effective_bytes"] {
        obs.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(adoption(&f, None, None).unwrap(), None);
    assert!(adoption(&f, Some("alpha*3"), None).is_err());
    for disposition in ["failed", "bypassed"] {
        let mut f = fixtures();
        let receipt = &mut f["provider-receipt-v1"];
        receipt["disposition"] = json!(disposition);
        receipt.as_object_mut().unwrap().remove("output");
        if disposition == "failed" {
            receipt["error_code"] = json!("provider-unavailable");
            receipt["completed_at_ms"] = json!(1400);
        }
        f["context-adoption-v1"]["receipt_digest"] =
            json!(canonical::document_digest(&f["provider-receipt-v1"]).unwrap());
        f["context-adoption-v1"]
            .as_object_mut()
            .unwrap()
            .remove("candidate_digest");
        f["context-adoption-v1"]["proof"]["observed_at_ms"] = json!(1450);
        preserve(&mut f, "no_candidate");
        assert_eq!(
            adoption(&f, Some("alpha alpha alpha\n"), None).unwrap(),
            Some(0)
        );
    }
}

fn security_invocation(f: &Value, kind: &str) -> Value {
    let mut inv = f["capability-invocation-v1"].clone();
    inv["capability"] = json!(format!("security.{kind}.inspect/v2"));
    inv["input"] = f[format!("security-{kind}-inspect-input-v2")].clone();
    inv["input_digest"] = json!(canonical::document_digest(&inv["input"]).unwrap());
    for dir in ["input", "output"] {
        inv[format!("{dir}_schema")] = REGISTRY
            .reference(&format!("security-{kind}-inspect-{dir}-v2"))
            .unwrap();
    }
    inv
}
fn security_result(f: &Value, kind: &str, output: &Value) -> Result<(), aw_contracts::Error> {
    let inv = security_invocation(f, kind);
    let mut receipt = f["provider-receipt-v1"].clone();
    for key in ["capability", "input_schema", "input_digest"] {
        receipt[key] = inv[key].clone();
    }
    receipt["output"] = json!({"schema":inv["output_schema"], "digest":canonical::document_digest(output).unwrap(), "bytes":canonical::bytes(output).unwrap().len()});
    REGISTRY.validate_result(&inv, &receipt, Some(output))
}

#[test]
fn scanners_report_requested_languages_complete_bytes_and_rulesets() {
    let f = fixtures();
    for kind in ["content", "code", "command"] {
        let output = f[format!("security-{kind}-inspect-output-v2")].clone();
        security_result(&f, kind, &output).unwrap();
        let key = if kind == "command" {
            "decision"
        } else {
            "inspection"
        };
        for field in ["scanned_bytes", "input_bytes"] {
            let mut bad = output.clone();
            bad[key]["coverage"][field] = json!(0);
            assert!(security_result(&f, kind, &bad).is_err());
        }
        let mut bad = output.clone();
        bad[key]["coverage"]["ruleset_ids"] = json!([]);
        assert!(security_result(&f, kind, &bad).is_err());
        let mut bad = output.clone();
        bad[key]["coverage"]["languages"] = json!(["python"]);
        assert!(security_result(&f, kind, &bad).is_err());
        let mut bad = output.clone();
        bad[key]["coverage"]["complete"] = json!(false);
        assert!(security_result(&f, kind, &bad).is_err());
    }
}

#[test]
fn command_gate_binds_final_intent_scope_arguments_and_expiry() {
    let f = fixtures();
    let inv = security_invocation(&f, "command");
    let intent = &f["execution-intent-v1"];
    let output = &f["security-command-inspect-output-v2"];
    REGISTRY
        .validate_execution_gate(&inv, output, intent, intent, 1500)
        .unwrap();
    for field in [
        "working_directory_ref",
        "environment_revision",
        "executor_revision",
        "target_id",
    ] {
        let mut changed = intent.clone();
        changed[field] = json!("changed");
        assert!(REGISTRY
            .validate_execution_gate(&inv, output, intent, &changed, 1500)
            .is_err());
    }
    let mut changed = intent.clone();
    changed["arguments"]["content"] = json!("different");
    assert!(REGISTRY
        .validate_execution_gate(&inv, output, intent, &changed, 1500)
        .is_err());
    assert!(REGISTRY
        .validate_execution_gate(&inv, output, intent, intent, 2000)
        .is_err());
    let mut changed = inv.clone();
    changed["scope"]["actor_id"] = json!("another");
    assert!(REGISTRY
        .validate_execution_gate(&changed, output, intent, intent, 1500)
        .is_err());
}

#[test]
fn control_observation_does_not_grant_authority() {
    let f = fixtures();
    let grant = &f["control-grant-v1"];
    let runtime = &f["runtime-binding-v1"];
    REGISTRY
        .validate_control(grant, runtime, "controller-1", "stop", 1500)
        .unwrap();
    assert!(REGISTRY
        .validate_control(grant, runtime, "observer-1", "stop", 1500)
        .is_err());
    assert!(REGISTRY
        .validate_control(grant, runtime, "controller-1", "stop", 2000)
        .is_err());
    for field in ["generation", "binding_revision"] {
        let mut changed = runtime.clone();
        changed[field] = json!(2);
        assert!(REGISTRY
            .validate_control(grant, &changed, "controller-1", "stop", 1500)
            .is_err());
    }
    let mut changed = runtime.clone();
    changed["owner_id"] = json!("another");
    assert!(REGISTRY
        .validate_control(grant, &changed, "controller-1", "stop", 1500)
        .is_err());
    changed = runtime.clone();
    changed["state"] = json!("exited");
    assert!(REGISTRY
        .validate_control(grant, &changed, "controller-1", "resume", 1500)
        .is_err());
    changed["state"] = json!("suspended");
    REGISTRY
        .validate_control(grant, &changed, "controller-1", "resume", 1500)
        .unwrap();
}

#[test]
fn uncertain_operations_require_query_and_terminal_records_are_immutable() {
    let f = fixtures();
    let mut previous = f["operation-record-v1"].clone();
    for (seq, state) in [
        (2, "approved"),
        (3, "started"),
        (4, "uncertain"),
        (5, "succeeded"),
    ] {
        let mut next = previous.clone();
        next["sequence"] = json!(seq);
        next["state"] = json!(state);
        next["approval_ref"] = json!("approval-1");
        if state == "succeeded" {
            next["evidence"] = json!([f["context-adoption-v1"]["proof"]["evidence"]]);
        }
        REGISTRY
            .validate_operation_transition(&previous, &next)
            .unwrap();
        if previous["state"] == "uncertain" {
            let mut bad = next.clone();
            bad["state"] = json!("started");
            assert!(REGISTRY
                .validate_operation_transition(&previous, &bad)
                .is_err());
        }
        let mut bad = next.clone();
        bad["resource_generation"] = json!("2");
        assert!(REGISTRY
            .validate_operation_transition(&previous, &bad)
            .is_err());
        previous = next;
    }
    REGISTRY
        .validate_operation_transition(&previous, &previous)
        .unwrap();
    let mut bad = previous.clone();
    bad["sequence"] = json!(6);
    assert!(REGISTRY
        .validate_operation_transition(&previous, &bad)
        .is_err());
    let prepared = &f["operation-record-v1"];
    let mut bad = prepared.clone();
    bad["state"] = json!("started");
    bad["sequence"] = json!(2);
    bad["approval_ref"] = json!("approval-1");
    assert!(REGISTRY
        .validate_operation_transition(prepared, &bad)
        .is_err());
}

#[test]
fn identifiers_reject_line_terminators_in_every_regex_engine() {
    let f = fixtures();
    for suffix in ["\n", "\r", "\u{2028}", "\u{2029}"] {
        let mut bad = f["runtime-binding-v1"].clone();
        bad["owner_id"] = json!(format!("owner{suffix}"));
        assert!(REGISTRY.validate("runtime-binding-v1", &bad).is_err());
        let mut bad = f["provider-receipt-v1"].clone();
        bad["input_digest"] = json!(format!("{}{suffix}", "0".repeat(64)));
        assert!(REGISTRY.validate("provider-receipt-v1", &bad).is_err());
    }
}
