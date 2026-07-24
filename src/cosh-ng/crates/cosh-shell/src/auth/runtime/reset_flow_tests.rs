use std::collections::HashMap;

use super::{
    apply_registry_configure_outcome, apply_reset_confirmation, build_auth_response,
    handle_auth_focus, submit_or_confirm_auth_response, AuthBackend, AuthPhase, RuntimeAuthState,
};
use crate::auth::reset::{configure_params, CoreAuthConfigureError};
use crate::runtime::prelude::{AuthProviderInfo, InlineState};

fn provider(id: &str) -> AuthProviderInfo {
    AuthProviderInfo {
        id: id.into(),
        label: id.into(),
        fields: Vec::new(),
    }
}

fn registry_reset_auth_state(id: &str) -> RuntimeAuthState {
    RuntimeAuthState {
        id: id.to_string(),
        run_id: "registry".into(),
        request_id: "req-1".into(),
        phase: AuthPhase::FillingField,
        providers: vec![AuthProviderInfo {
            id: "dashscope".into(),
            label: "DashScope".into(),
            fields: Vec::new(),
        }],
        selected_provider: 0,
        current_field: 0,
        collected_values: HashMap::from([("api_key".to_string(), "new-secret".to_string())]),
        field_input: String::new(),
        existing_providers: Vec::new(),
        editing_provider_name: Some("home-provider".to_string()),
        reset_unavailable_credentials: false,
        reset_confirm_selection: 1,
        credentials_unavailable: true,
        backend: AuthBackend::CoreRegistry,
    }
}

#[test]
fn reset_confirmation_preserves_nonzero_provider_selection() {
    let id = "auth-run-1-req-1".to_string();
    let mut state = InlineState::default();
    state.auth.state = Some(RuntimeAuthState {
        id: id.clone(),
        run_id: "run-1".into(),
        request_id: "req-1".into(),
        phase: AuthPhase::FillingField,
        providers: vec![
            provider("dashscope"),
            provider("openai_compat"),
            provider("aliyun"),
        ],
        selected_provider: 2,
        current_field: 0,
        collected_values: HashMap::from([
            ("access_key_id".to_string(), "ak".to_string()),
            ("access_key_secret".to_string(), "sk".to_string()),
        ]),
        field_input: String::new(),
        existing_providers: Vec::new(),
        editing_provider_name: None,
        reset_unavailable_credentials: false,
        reset_confirm_selection: 1,
        credentials_unavailable: true,
        backend: AuthBackend::ActiveRun,
    });

    submit_or_confirm_auth_response(None, &mut state, &mut Vec::new()).unwrap();
    {
        let auth = state.auth.state.as_ref().expect("auth state present");
        assert_eq!(auth.phase, AuthPhase::ConfirmResetUnavailable);
        assert_eq!(auth.selected_provider, 2);
        assert_eq!(auth.reset_confirm_selection, 1);
    }

    handle_auth_focus(&mut state, &id, 0, &mut Vec::new()).unwrap();
    {
        let auth = state.auth.state.as_ref().unwrap();
        assert_eq!(auth.reset_confirm_selection, 0);
        assert_eq!(auth.selected_provider, 2);
        assert_eq!(auth.providers[auth.selected_provider].id, "aliyun");
    }

    handle_auth_focus(&mut state, &id, 1, &mut Vec::new()).unwrap();
    {
        let auth = state.auth.state.as_ref().unwrap();
        assert_eq!(auth.reset_confirm_selection, 1);
        assert_eq!(auth.selected_provider, 2);
        assert_eq!(auth.providers[auth.selected_provider].id, "aliyun");
    }
}

#[test]
fn registry_reset_required_outcome_prompts_confirmation_then_resubmits_with_reset() {
    let mut state = InlineState::default();
    state.auth.state = Some(registry_reset_auth_state("auth-registry-req-1"));

    apply_registry_configure_outcome(
        Err(CoreAuthConfigureError::ResetRequired),
        &mut state,
        &mut Vec::new(),
        "DashScope",
    )
    .unwrap();
    {
        let auth = state.auth.state.as_ref().expect("auth state present");
        assert_eq!(auth.phase, AuthPhase::ConfirmResetUnavailable);
        assert_eq!(auth.reset_confirm_selection, 1);
        assert!(!auth.reset_unavailable_credentials);
    }

    let auth = state.auth.state.as_mut().unwrap();
    auth.reset_confirm_selection = 0;
    assert!(apply_reset_confirmation(auth));
    assert!(auth.reset_unavailable_credentials);
    let response = build_auth_response(auth);
    assert!(response.reset_unavailable_credentials);
    assert_eq!(
        configure_params(&response)["reset_unavailable_credentials"],
        true
    );

    let mut output = Vec::new();
    apply_registry_configure_outcome(Ok(()), &mut state, &mut output, "DashScope").unwrap();
    assert!(state.auth.state.is_none(), "auth flow should complete");
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("credentials saved"));
}

#[test]
fn keeping_unavailable_credentials_does_not_set_reset_flag() {
    let mut auth = registry_reset_auth_state("auth-registry-req-1");
    auth.reset_confirm_selection = 1;
    assert!(!apply_reset_confirmation(&mut auth));
    assert!(!auth.reset_unavailable_credentials);
    let response = build_auth_response(&auth);
    assert!(!response.reset_unavailable_credentials);
    assert_eq!(
        configure_params(&response)["reset_unavailable_credentials"],
        false
    );
}
