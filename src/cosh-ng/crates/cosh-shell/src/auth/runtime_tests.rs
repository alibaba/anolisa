use super::runtime::*;
use crate::runtime::prelude::{
    AgentEvent, AuthFieldInfo, AuthOutcome, AuthProviderInfo, GovernanceDecision,
    GovernancePolicyDecision, GovernedEvent, InlineState, RawInputCapture,
};

fn provider(id: &str, label: &str) -> AuthProviderInfo {
    AuthProviderInfo {
        id: id.into(),
        label: label.into(),
        fields: Vec::new(),
    }
}

fn governed_auth_required(providers: Vec<AuthProviderInfo>) -> GovernedEvent {
    governed_auth_required_for_run("run-1", providers)
}

fn governed_auth_required_for_run(run_id: &str, providers: Vec<AuthProviderInfo>) -> GovernedEvent {
    GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::AuthRequired {
            run_id: run_id.into(),
            request_id: "req-1".into(),
            reason: "test".into(),
            error_message: None,
            credentials_unavailable: false,
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
    assert_eq!(ids, vec!["auth-run-1-req-1".to_string()]);

    let stored = state.auth.state.expect("auth state recorded");
    let ids: Vec<&str> = stored.providers.iter().map(|p| p.id.as_str()).collect();
    // Aliyun promoted to front; other providers keep their original relative order.
    assert_eq!(ids, ["aliyun", "dashscope", "openai_compat"]);
    assert!(stored.providers[0].label.contains("免费可用"));
}

#[test]
fn record_auth_required_preserves_reset_requirement() {
    let mut event = governed_auth_required(vec![provider("dashscope", "DashScope")]);
    if let AgentEvent::AuthRequired {
        credentials_unavailable,
        ..
    } = &mut event.event
    {
        *credentials_unavailable = true;
    }
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[event]);

    assert!(state
        .auth
        .state
        .as_ref()
        .is_some_and(|auth| auth.credentials_unavailable));
}

fn governed_auth_result(run_id: &str, request_id: &str, outcome: AuthOutcome) -> GovernedEvent {
    GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::AuditOnly,
        event: AgentEvent::AuthResult {
            run_id: run_id.into(),
            request_id: request_id.into(),
            outcome,
        },
        reason: "test".into(),
        display_text: String::new(),
        auto_execute: false,
    }
}

#[test]
fn auth_result_waits_for_core_before_reporting_saved() {
    let mut state = InlineState::default();
    record_auth_required(
        &mut state,
        &[governed_auth_required(vec![provider(
            "dashscope",
            "DashScope",
        )])],
    );
    state.auth.state.as_mut().unwrap().phase = AuthPhase::AwaitingResult {
        provider_label: "DashScope".to_string(),
    };

    let mut failed_output = Vec::new();
    record_auth_results(
        &mut state,
        &[governed_auth_result("run-1", "req-1", AuthOutcome::Failed)],
        &mut failed_output,
    )
    .unwrap();
    assert!(state.auth.state.is_none());
    assert!(String::from_utf8(failed_output)
        .unwrap()
        .contains("Credentials were not saved"));

    record_auth_required(
        &mut state,
        &[governed_auth_required_for_run(
            "run-2",
            vec![provider("dashscope", "DashScope")],
        )],
    );
    state.auth.state.as_mut().unwrap().phase = AuthPhase::AwaitingResult {
        provider_label: "DashScope".to_string(),
    };
    let mut success_output = Vec::new();
    record_auth_results(
        &mut state,
        &[governed_auth_result("run-2", "req-1", AuthOutcome::Saved)],
        &mut success_output,
    )
    .unwrap();
    assert!(state.auth.state.is_none());
    assert!(String::from_utf8(success_output)
        .unwrap()
        .contains("credentials saved"));
}

#[test]
fn auth_result_applied_reports_session_only_not_saved() {
    let mut state = InlineState::default();
    record_auth_required(
        &mut state,
        &[governed_auth_required(vec![provider(
            "dashscope",
            "DashScope",
        )])],
    );
    state.auth.state.as_mut().unwrap().phase = AuthPhase::AwaitingResult {
        provider_label: "DashScope".to_string(),
    };

    let mut output = Vec::new();
    record_auth_results(
        &mut state,
        &[governed_auth_result("run-1", "req-1", AuthOutcome::Applied)],
        &mut output,
    )
    .unwrap();

    // Applied completes the flow (state cleared) but must not claim the config
    // was saved; it reports a session-only application instead.
    assert!(state.auth.state.is_none());
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("applied for this session (not saved)"),
        "unexpected notice: {text}"
    );
    assert!(!text.contains("credentials saved"));
    assert!(!text.contains("Credentials were not saved"));
}

#[test]
fn auth_handshake_isolated_by_run_id() {
    let mut state = InlineState::default();
    record_auth_required(
        &mut state,
        &[governed_auth_required_for_run(
            "run-1",
            vec![provider("dashscope", "DashScope")],
        )],
    );
    state.auth.state.as_mut().unwrap().phase = AuthPhase::AwaitingResult {
        provider_label: "DashScope".to_string(),
    };
    record_auth_results(
        &mut state,
        &[governed_auth_result("run-1", "req-1", AuthOutcome::Saved)],
        &mut Vec::new(),
    )
    .unwrap();

    let ids = record_auth_required(
        &mut state,
        &[governed_auth_required_for_run(
            "run-2",
            vec![provider("dashscope", "DashScope")],
        )],
    );
    assert_eq!(ids, vec!["auth-run-2-req-1".to_string()]);
    state.auth.state.as_mut().unwrap().phase = AuthPhase::AwaitingResult {
        provider_label: "DashScope".to_string(),
    };

    record_auth_results(
        &mut state,
        &[governed_auth_result("run-1", "req-1", AuthOutcome::Saved)],
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
        state.auth.state.as_ref().map(|auth| auth.run_id.as_str()),
        Some("run-2")
    );
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
