use super::retry::restore_after_failed_submission;
use super::runtime::*;
use crate::runtime::prelude::{
    AgentEvent, AuthFieldInfo, AuthProviderInfo, GovernanceDecision, GovernancePolicyDecision,
    GovernedEvent, InlineState, RawInputCapture,
};

fn provider(id: &str, label: &str) -> AuthProviderInfo {
    AuthProviderInfo {
        id: id.into(),
        label: label.into(),
        fields: Vec::new(),
    }
}

fn governed_auth_required(providers: Vec<AuthProviderInfo>) -> GovernedEvent {
    GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::AuthRequired {
            run_id: "run-1".into(),
            request_id: "req-1".into(),
            reason: "test".into(),
            error_message: None,
            providers,
        },
        reason: "test".into(),
        display_text: "test".into(),
        auto_execute: false,
    }
}

#[test]
fn record_auth_required_promotes_aliyun_from_legacy_order() {
    // cosh-core's control protocol still emits the legacy provider order.
    let legacy = vec![
        provider("dashscope", "DashScope (百炼)"),
        provider("openai_compat", "OpenAI Compatible"),
        provider("aliyun", "Aliyun Authentication"),
    ];
    let mut state = InlineState::default();
    let ids = record_auth_required(&mut state, &[governed_auth_required(legacy)]);
    assert_eq!(ids, vec!["auth-req-1".to_string()]);

    let stored = state.auth.state.expect("auth state recorded");
    let ids: Vec<&str> = stored.providers.iter().map(|p| p.id.as_str()).collect();
    // Aliyun promoted to front; other providers keep their original relative order.
    assert_eq!(ids, ["aliyun", "dashscope", "openai_compat"]);
    assert!(stored.providers[0].label.contains("免费可用"));
}

#[test]
fn pending_auth_capture_marks_secret_fields() {
    let mut provider = provider("openai_compat", "OpenAI Compatible");
    provider.fields.push(AuthFieldInfo {
        name: "api_key".to_string(),
        label: "API key".to_string(),
        hint: None,
        secret: true,
        required: true,
        placeholder: None,
    });
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    state.auth.state.as_mut().unwrap().phase = AuthPhase::FillingField;

    assert!(matches!(
        pending_auth_capture(&state),
        Some(RawInputCapture::Question { secret: true, .. })
    ));
}

#[test]
fn pending_auth_capture_isolates_each_auth_field() {
    let mut provider = provider("openai_compat", "OpenAI Compatible");
    provider.fields = vec![
        AuthFieldInfo {
            name: "base_url".to_string(),
            label: "Base URL".to_string(),
            hint: None,
            secret: false,
            required: true,
            placeholder: None,
        },
        AuthFieldInfo {
            name: "model".to_string(),
            label: "Model".to_string(),
            hint: None,
            secret: false,
            required: true,
            placeholder: None,
        },
    ];
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::FillingField;

    let RawInputCapture::Question { id: first_id, .. } = pending_auth_capture(&state).unwrap()
    else {
        panic!("expected question capture");
    };
    state.auth.state.as_mut().unwrap().current_field = 1;
    let RawInputCapture::Question { id: second_id, .. } = pending_auth_capture(&state).unwrap()
    else {
        panic!("expected question capture");
    };

    assert_ne!(first_id, second_id);
}

#[test]
fn failed_submission_restarts_with_clean_values() {
    let mut provider = provider("openai_compat", "OpenAI Compatible");
    provider.fields.push(AuthFieldInfo {
        name: "provider_id".to_string(),
        label: "Provider ID".to_string(),
        hint: None,
        secret: false,
        required: true,
        placeholder: None,
    });
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::FillingField;
    auth.current_field = 1;
    auth.collected_values
        .insert("provider_id".to_string(), "bad.provider".to_string());
    auth.field_input = "stale-model".to_string();

    restore_after_failed_submission(auth);

    assert_eq!(auth.phase, AuthPhase::FillingField);
    assert_eq!(auth.current_field, 0);
    assert!(auth.collected_values.is_empty());
    assert!(auth.field_input.is_empty());
}

#[test]
fn failed_edit_submission_preserves_provider_identity() {
    let mut provider = provider("openai_compat", "OpenAI Compatible");
    provider.fields = vec![
        AuthFieldInfo {
            name: "provider_id".to_string(),
            label: "Provider ID".to_string(),
            hint: None,
            secret: false,
            required: true,
            placeholder: None,
        },
        AuthFieldInfo {
            name: "api_key".to_string(),
            label: "API Key".to_string(),
            hint: None,
            secret: true,
            required: true,
            placeholder: None,
        },
    ];
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::FillingField;
    auth.current_field = 2;
    auth.editing_provider_name = Some("existing-provider".to_string());
    auth.collected_values
        .insert("api_key".to_string(), "stale-key".to_string());
    auth.field_input = "stale-value".to_string();

    restore_after_failed_submission(auth);

    assert_eq!(auth.phase, AuthPhase::FillingField);
    assert_eq!(auth.current_field, 1);
    assert_eq!(
        auth.collected_values.get("provider_id").map(String::as_str),
        Some("existing-provider")
    );
    assert!(!auth.collected_values.contains_key("api_key"));
    assert!(auth.field_input.is_empty());
}
