use super::retry::restore_after_failed_submission;
use super::runtime::*;
use super::validation::{record_field_edit, record_field_submission, FieldSubmission};
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
    governed_auth_required_with_error("req-1", providers, None)
}

fn governed_auth_required_with_error(
    request_id: &str,
    providers: Vec<AuthProviderInfo>,
    error_message: Option<&str>,
) -> GovernedEvent {
    GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::AuthRequired {
            run_id: "run-1".into(),
            request_id: request_id.into(),
            reason: "test".into(),
            error_message: error_message.map(str::to_string),
            providers,
        },
        reason: "test".into(),
        display_text: "test".into(),
        auto_execute: false,
    }
}

#[test]
fn retry_auth_request_surfaces_validation_error() {
    let mut state = InlineState::default();
    state.auth.completed_ids.insert("auth-req-1".to_string());
    let event = governed_auth_required_with_error(
        "req-1-retry-1",
        vec![provider("openai_compat", "OpenAI Compatible")],
        Some("invalid base_url"),
    );

    let ids = record_auth_required(&mut state, &[event]);
    let mut output = Vec::new();
    render_auth_panel(&mut state, &ids, &mut output).expect("render retry auth");

    assert_eq!(ids, vec!["auth-req-1-retry-1".to_string()]);
    assert!(String::from_utf8(output)
        .expect("utf8 auth panel")
        .contains("invalid base_url"));
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
        Some(RawInputCapture::TextQuestion { secret: true, .. })
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

    let RawInputCapture::TextQuestion {
        id: first_id,
        initial_text: first_input,
        ..
    } = pending_auth_capture(&state).unwrap()
    else {
        panic!("expected question capture");
    };
    assert!(first_input.is_empty());
    state.auth.state.as_mut().unwrap().current_field = 1;
    let RawInputCapture::TextQuestion { id: second_id, .. } = pending_auth_capture(&state).unwrap()
    else {
        panic!("expected question capture");
    };

    assert_ne!(first_id, second_id);
}

fn openai_compat_with_provider_id_field() -> AuthProviderInfo {
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
            name: "base_url".to_string(),
            label: "Base URL".to_string(),
            hint: None,
            secret: false,
            required: true,
            placeholder: None,
        },
    ];
    provider
}

fn filling_provider_id_state() -> InlineState {
    let mut state = InlineState::default();
    record_auth_required(
        &mut state,
        &[governed_auth_required(vec![
            openai_compat_with_provider_id_field(),
        ])],
    );
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::FillingField;
    auth.current_field = 0;
    state
}

#[test]
fn dotted_provider_id_submission_stays_on_the_provider_id_field() {
    let mut state = filling_provider_id_state();
    let auth = state.auth.state.as_mut().unwrap();
    let field = auth.providers[0].fields[0].clone();

    let outcome = record_field_submission(auth, Some(&field), "qwen3.7-max".to_string());

    assert_eq!(outcome, FieldSubmission::Rejected);
    // Still on Provider ID: Base URL / API Key / Model are never reached.
    assert_eq!(auth.current_field, 0);
    assert_eq!(auth.phase, AuthPhase::FillingField);
    assert_eq!(auth.field_input, "qwen3.7-max");
    assert!(!auth.collected_values.contains_key("provider_id"));
    let error = auth.field_error.as_deref().expect("inline error recorded");
    assert!(error.contains("letters, digits"), "{error}");
}

#[test]
fn empty_and_non_ascii_provider_id_submissions_are_rejected() {
    for value in ["", "\u{6a21}\u{578b}", "bad.provider"] {
        let mut state = filling_provider_id_state();
        let auth = state.auth.state.as_mut().unwrap();
        let field = auth.providers[0].fields[0].clone();

        let outcome = record_field_submission(auth, Some(&field), value.to_string());

        assert_eq!(outcome, FieldSubmission::Rejected, "value={value:?}");
        assert_eq!(auth.current_field, 0, "value={value:?}");
        assert!(auth.collected_values.is_empty(), "value={value:?}");
        assert!(auth.field_error.is_some(), "value={value:?}");
    }
}

#[test]
fn valid_provider_id_submission_is_recorded_without_error() {
    for value in ["qwen-prod", "qwen_prod", "Qwen37"] {
        let mut state = filling_provider_id_state();
        let auth = state.auth.state.as_mut().unwrap();
        let field = auth.providers[0].fields[0].clone();

        let outcome = record_field_submission(auth, Some(&field), value.to_string());

        assert_eq!(outcome, FieldSubmission::Accepted, "value={value:?}");
        assert_eq!(
            auth.collected_values.get("provider_id").map(String::as_str),
            Some(value)
        );
        assert!(auth.field_error.is_none(), "value={value:?}");
    }
}

#[test]
fn editing_the_field_again_clears_the_previous_error() {
    let mut state = filling_provider_id_state();
    let auth = state.auth.state.as_mut().unwrap();
    let field = auth.providers[0].fields[0].clone();
    record_field_submission(auth, Some(&field), "qwen3.7-max".to_string());
    assert!(auth.field_error.is_some());

    record_field_edit(auth, "qwen3-7-max");

    assert!(auth.field_error.is_none());
    assert_eq!(auth.field_input, "qwen3-7-max");
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

fn auth_field(name: &str, label: &str, secret: bool) -> AuthFieldInfo {
    AuthFieldInfo {
        name: name.to_string(),
        label: label.to_string(),
        hint: None,
        secret,
        required: true,
        placeholder: None,
    }
}

/// An edit of `qwen-prod` that reached the end of the form: every field is collected, the API
/// key still carries the display mask, and the user retyped the Access Key Secret.
fn failed_edit_state() -> InlineState {
    let mut provider = provider("openai_compat", "OpenAI Compatible");
    provider.fields = vec![
        auth_field("provider_id", "Provider ID", false),
        auth_field("base_url", "Base URL", false),
        auth_field("model", "Model", false),
        auth_field("api_key", "API Key", true),
        auth_field("access_key_secret", "Access Key Secret", true),
    ];
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::FillingField;
    auth.current_field = 5;
    auth.editing_provider_name = Some("qwen-prod".to_string());
    auth.collected_values = [
        ("provider_id", "qwen-prod"),
        ("base_url", "https://example.invalid/v1"),
        ("model", "qwen3.7-plus"),
        ("api_key", "\u{2022}\u{2022}\u{2022}"),
        ("access_key_secret", "real-typed-secret"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    auth.field_input = "real-typed-secret".to_string();
    state
}

/// #1834: a rejected edit wiped the whole form, so the user had to retype every field.
#[test]
fn failed_edit_submission_keeps_the_values_the_user_can_reconfirm() {
    let mut state = failed_edit_state();
    let auth = state.auth.state.as_mut().unwrap();

    restore_after_failed_submission(auth);

    assert_eq!(
        auth.collected_values.get("provider_id").map(String::as_str),
        Some("qwen-prod")
    );
    assert_eq!(
        auth.collected_values.get("base_url").map(String::as_str),
        Some("https://example.invalid/v1")
    );
    assert_eq!(
        auth.collected_values.get("model").map(String::as_str),
        Some("qwen3.7-plus")
    );
    // A bullet mask is not a credential: cosh-core swaps it back for the stored secret, so
    // keeping it is what lets an unrelated field be fixed without retyping the API key.
    assert_eq!(
        auth.collected_values.get("api_key").map(String::as_str),
        Some("\u{2022}\u{2022}\u{2022}")
    );
    // The cursor returns to the first editable field with its value re-projected.
    assert_eq!(auth.current_field, 1);
    assert_eq!(auth.field_input, "https://example.invalid/v1");
    assert!(auth.field_error.is_none());
}

/// A real secret typed into the rejected attempt is dropped: a failed submission must not
/// extend how long a plaintext credential lives in the form.
#[test]
fn failed_edit_submission_drops_secrets_the_user_actually_typed() {
    let mut state = failed_edit_state();
    let auth = state.auth.state.as_mut().unwrap();

    restore_after_failed_submission(auth);

    assert!(!auth.collected_values.contains_key("access_key_secret"));
    assert!(
        !auth
            .collected_values
            .values()
            .any(|value| value == "real-typed-secret"),
        "plaintext secret survived the retry: {:?}",
        auth.collected_values
    );
}

/// Selective preservation is scoped to edits. A new provider must still retry from empty,
/// which is what #1769 hardened the retry path for.
#[test]
fn failed_new_provider_submission_still_clears_every_value() {
    let mut state = failed_edit_state();
    let auth = state.auth.state.as_mut().unwrap();
    auth.editing_provider_name = None;

    restore_after_failed_submission(auth);

    assert_eq!(auth.current_field, 0);
    assert!(auth.collected_values.is_empty());
    assert!(auth.field_input.is_empty());
}

/// The restored panel has to offer the keep-current-value shortcut again; without the
/// re-projected `field_input` the hint stays hidden and the value looks lost.
#[test]
fn restored_edit_panel_offers_to_keep_the_current_value() {
    let mut state = failed_edit_state();
    restore_after_failed_submission(state.auth.state.as_mut().unwrap());

    let mut output = Vec::new();
    crate::auth::prompt::render_current_auth_panel(&mut state, &mut output)
        .expect("render restored panel");
    let rendered = String::from_utf8(output).expect("utf8 panel");

    assert!(
        rendered.contains("keep current value"),
        "restored panel dropped the keep-current-value hint: {rendered}"
    );
}

/// An Aliyun edit that reached the ECS RAM-role challenge and had its configure rejected.
fn failed_ecs_challenge_edit_state() -> InlineState {
    let mut provider = provider("aliyun", "Aliyun Authentication");
    provider.fields = vec![
        auth_field("provider_id", "Provider ID", false),
        auth_field("access_key_id", "Access Key ID", true),
        auth_field("access_key_secret", "Access Key Secret", true),
        auth_field("model", "Model", false),
    ];
    let mut state = InlineState::default();
    record_auth_required(&mut state, &[governed_auth_required(vec![provider])]);
    let auth = state.auth.state.as_mut().unwrap();
    auth.phase = AuthPhase::AliyunEcsChallenge {
        instance_id: "i-test-1".to_string(),
        console_url: "https://example.invalid/authorize".to_string(),
    };
    auth.editing_provider_name = Some("sysom-trial".to_string());
    // What apply_aliyun_prepare leaves behind: the ECS marker in, AK/SK out.
    auth.collected_values = [
        ("provider_id", "sysom-trial"),
        ("auth_source", "ecs_ram_role"),
        ("model", "qwen3.7-plus"),
        ("security_token", "\u{2022}\u{2022}\u{2022}"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    state
}

/// The retry comes back to the manual AK/SK prompts, so the ECS marker cannot survive: cosh-core
/// reads it as "credentials come from the metadata service", skips the AK/SK required check and
/// stores `None` for them — the user would type an Access Key pair and have it silently dropped.
#[test]
fn failed_ecs_challenge_edit_retry_drops_the_ecs_auth_source() {
    let mut state = failed_ecs_challenge_edit_state();
    let auth = state.auth.state.as_mut().unwrap();

    restore_after_failed_submission(auth);

    assert!(
        !auth.collected_values.contains_key("auth_source"),
        "manual retry kept the ECS marker: {:?}",
        auth.collected_values
    );
    // The restored phase asks for Access Key ID, which cosh-core will now actually validate.
    assert_eq!(auth.phase, AuthPhase::FillingField);
    assert_eq!(auth.current_field, 1);
    assert_eq!(
        auth.current_field_info().map(|field| field.name.as_str()),
        Some("access_key_id")
    );
    // Values that describe the provider rather than contradict the phase still survive.
    assert_eq!(
        auth.collected_values.get("model").map(String::as_str),
        Some("qwen3.7-plus")
    );
    assert_eq!(
        auth.collected_values
            .get("security_token")
            .map(String::as_str),
        Some("\u{2022}\u{2022}\u{2022}")
    );
}
