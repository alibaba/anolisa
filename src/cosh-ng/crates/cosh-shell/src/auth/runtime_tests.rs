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
