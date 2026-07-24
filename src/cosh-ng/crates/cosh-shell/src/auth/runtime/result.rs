//! Auth submission result handling and terminal-state feedback.

use super::{load_current_field_input, render_current_auth_panel, AuthPhase};
use crate::auth::completion::finish_auth_configuration;
use crate::auth::reset::CoreAuthConfigureError;
use crate::runtime::prelude::{
    AgentEvent, AuthOutcome, GovernedEvent, NoticePanelModel, RatatuiInlineRenderer,
};
use crate::runtime::state::InlineState;

pub(crate) fn record_auth_results<W: std::io::Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    output: &mut W,
) -> std::io::Result<()> {
    for event in governed_events {
        let AgentEvent::AuthResult {
            run_id,
            request_id,
            outcome,
            ..
        } = &event.event
        else {
            continue;
        };
        let Some(auth) = state.auth.state.as_ref() else {
            continue;
        };
        if auth.run_id != *run_id || auth.request_id != *request_id {
            continue;
        }
        let AuthPhase::AwaitingResult { provider_label } = &auth.phase else {
            continue;
        };
        let provider_label = provider_label.clone();
        match outcome {
            AuthOutcome::Saved => complete(state, output, &provider_label, true)?,
            AuthOutcome::Applied => complete(state, output, &provider_label, false)?,
            AuthOutcome::Failed => close_failed_active_run(state, output)?,
        }
    }
    Ok(())
}

pub(super) fn apply_registry_configure_outcome<W: std::io::Write>(
    outcome: Result<(), CoreAuthConfigureError>,
    state: &mut InlineState,
    output: &mut W,
    provider_label: &str,
) -> std::io::Result<()> {
    match outcome {
        // Registry configuration always persists on success.
        Ok(()) => complete(state, output, provider_label, true),
        Err(CoreAuthConfigureError::ResetRequired) => {
            if let Some(auth) = state.auth.state.as_mut() {
                auth.phase = AuthPhase::ConfirmResetUnavailable;
                auth.reset_confirm_selection = 1;
            }
            render_current_auth_panel(state, output)
        }
        Err(CoreAuthConfigureError::Other(error)) => {
            restore_after_failed_submission(state, output, &error)
        }
    }
}

fn complete<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
    provider_label: &str,
    persisted: bool,
) -> std::io::Result<()> {
    if let Some(auth) = state.auth.state.take() {
        state.auth.completed_ids.insert(auth.completion_key());
    }
    finish_auth_configuration(state, output, provider_label, persisted)
}

fn restore_after_failed_submission<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
    detail: &str,
) -> std::io::Result<()> {
    if let Some(auth) = state.auth.state.as_mut() {
        auth.phase = AuthPhase::FillingField;
        auth.current_field = 0;
        load_current_field_input(auth);
    }
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: "Credentials were not saved",
            body: vec![
                detail.to_string(),
                "Review the values and try again.".to_string(),
            ],
            footer: None,
        },
    )?;
    render_current_auth_panel(state, output)
}

pub(super) fn close_failed_active_run<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if let Some(auth) = state.auth.state.take() {
        state.auth.completed_ids.insert(auth.completion_key());
    }
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: "Credentials were not saved",
            body: vec![
                "cosh-core could not save or use the submitted credentials.".to_string(),
                "Start a new agent request, or run /auth after the current run finishes."
                    .to_string(),
            ],
            footer: None,
        },
    )?;
    output.flush()
}
