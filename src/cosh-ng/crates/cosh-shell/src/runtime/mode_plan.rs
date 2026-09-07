use crate::runtime::mode::render_notice_panel;
use crate::runtime::prelude::*;

pub(crate) fn plan_mode_label(active: bool) -> &'static str {
    if active {
        "on"
    } else {
        "off"
    }
}

/// `/plan [on|off|status]` — with no argument it toggles plan mode; `/mode
/// plan [on|off|status]` shares this handler but shows the status when no
/// sub-argument is given (`toggle_when_unset = false`).
pub(crate) fn render_plan_mode_command<W: Write>(
    arg: Option<&str>,
    toggle_when_unset: bool,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None if toggle_when_unset => set_plan_mode(!state.plan_mode, state, output),
        None | Some("status") => render_plan_mode_status(state, output),
        Some("on") => {
            if state.plan_mode {
                return render_plan_mode_unchanged(MessageId::PlanModeAlreadyOnBody, state, output);
            }
            set_plan_mode(true, state, output)
        }
        Some("off") => {
            if !state.plan_mode {
                return render_plan_mode_unchanged(
                    MessageId::PlanModeAlreadyOffBody,
                    state,
                    output,
                );
            }
            set_plan_mode(false, state, output)
        }
        Some(other) => render_notice_panel(
            output,
            state.i18n().t(MessageId::PlanModeTitle),
            vec![state
                .i18n()
                .format(MessageId::PlanModeUnknownBody, &[("mode", other)])],
            Some(state.i18n().t(MessageId::PlanModeUsageFooter)),
        )
        .map(|_| true),
    }
}

fn set_plan_mode<W: Write>(
    active: bool,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    state.plan_mode = active;
    let (body, footer) = if active {
        (
            MessageId::PlanModeEnabledBody,
            MessageId::PlanModeEnabledFooter,
        )
    } else {
        (
            MessageId::PlanModeDisabledBody,
            MessageId::PlanModeDisabledFooter,
        )
    };
    render_notice_panel(
        output,
        state.i18n().t(MessageId::PlanModeTitle),
        vec![state.i18n().t(body).to_string()],
        Some(state.i18n().t(footer)),
    )?;
    Ok(true)
}

fn render_plan_mode_status<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<bool> {
    let body = if state.plan_mode {
        MessageId::PlanModeStatusOnBody
    } else {
        MessageId::PlanModeStatusOffBody
    };
    render_notice_panel(
        output,
        state.i18n().t(MessageId::PlanModeTitle),
        vec![state
            .i18n()
            .format(body, &[("mode", state.approval_mode.label())])],
        Some(state.i18n().t(MessageId::PlanModeUsageFooter)),
    )?;
    Ok(true)
}

fn render_plan_mode_unchanged<W: Write>(
    body: MessageId,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    render_notice_panel(
        output,
        state.i18n().t(MessageId::PlanModeTitle),
        vec![state.i18n().t(body).to_string()],
        Some(state.i18n().t(MessageId::PlanModeUsageFooter)),
    )?;
    Ok(true)
}
