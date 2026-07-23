use std::sync::{Arc, Mutex};

use cosh_shell::adapter::{
    CoshCoreAdapter, SessionHealth, SessionRecoveryState, SessionRuntimeState,
};

#[test]
fn recovery_states_and_health_have_stable_user_labels() {
    assert_eq!(SessionRecoveryState::None.label(), "none");
    assert_eq!(SessionRecoveryState::Selected.label(), "selected");
    assert_eq!(SessionRecoveryState::Restoring.label(), "restoring");
    assert_eq!(SessionRecoveryState::Active.label(), "active");
    assert_eq!(SessionRecoveryState::Failed.label(), "failed");

    assert!(SessionHealth::Ready.can_resume());
    assert!(!SessionHealth::Corrupt.can_resume());
    assert!(!SessionHealth::Incompatible.can_resume());
    assert!(!SessionHealth::ScopeMismatch.can_resume());
}

#[test]
fn active_and_selected_provider_sessions_are_both_protected() {
    let active = "00000000-0000-4000-8000-000000000000".to_string();
    let selected = "11111111-1111-4111-8111-111111111111".to_string();
    let mut session = SessionRuntimeState::with_active(active.clone(), "/tmp");
    session.recovery.state = SessionRecoveryState::Selected;
    session.recovery.selected_session_id = Some(selected.clone());
    session.recovery.selected_workspace_scope = Some("/tmp".to_string());
    let adapter = CoshCoreAdapter {
        program: "unused".to_string(),
        allow_model_call: false,
        session: Arc::new(Mutex::new(session)),
    };

    assert_eq!(adapter.protected_session_ids(), vec![active, selected]);
}
