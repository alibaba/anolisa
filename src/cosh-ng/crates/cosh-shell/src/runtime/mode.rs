use crate::runtime::prelude::*;
use crate::slash::prompt::write_shell_prompt;

pub(crate) fn render_mode_command<W: Write>(
    arg: Option<&str>,
    sub: Option<&str>,
    confirm: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None => render_mode_summary(state, output),
        Some("approval") => render_approval_mode_command(sub, confirm, state, output),
        Some("analysis") => render_analysis_mode_command(sub, state, output),
        Some("recommend" | "auto" | "trust") => render_notice_panel(
            output,
            state.i18n().t(MessageId::ModeRemovedTitle),
            vec![state
                .i18n()
                .format(MessageId::ModeRemovedBody, &[("mode", arg.unwrap())])],
            Some(
                &state
                    .i18n()
                    .format(MessageId::ModeRemovedFooter, &[("mode", arg.unwrap())]),
            ),
        )
        .map(|_| true),
        Some("language") => render_notice_panel(
            output,
            state.i18n().t(MessageId::ModeTitle),
            vec![state.i18n().t(MessageId::ModeLanguageBody).to_string()],
            Some(state.i18n().t(MessageId::ModeLanguageFooter)),
        )
        .map(|_| true),
        Some(other) => render_notice_panel(
            output,
            state.i18n().t(MessageId::ModeTitle),
            vec![state
                .i18n()
                .format(MessageId::ModeUnknownBody, &[("mode", other)])],
            Some(state.i18n().t(MessageId::ModeUnknownFooter)),
        )
        .map(|_| true),
    }
}

fn render_mode_summary<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<bool> {
    render_notice_panel(
        output,
        state.i18n().t(MessageId::ModesTitle),
        vec![
            state.i18n().format(
                MessageId::ModeApprovalLine,
                &[("mode", state.approval_mode.label())],
            ),
            state.i18n().format(
                MessageId::ModeAnalysisLine,
                &[("mode", state.analysis_mode.label())],
            ),
        ],
        Some(state.i18n().t(MessageId::ModeSummaryFooter)),
    )?;
    Ok(true)
}

fn render_approval_mode_command<W: Write>(
    arg: Option<&str>,
    confirm: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None => {
            state
                .control
                .set_pending_mode_panel(match state.approval_mode {
                    CoshApprovalMode::Recommend => 0,
                    CoshApprovalMode::Auto => 1,
                    CoshApprovalMode::Trust => 2,
                });
            render_current_mode_panel(state, output)?;
            Ok(false)
        }
        Some("recommend") => {
            state.approval_mode = CoshApprovalMode::Recommend;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::ApprovalModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::ApprovalModeSetBody, &[("mode", "recommend")])],
                Some(mode_footer(state.i18n(), CoshApprovalMode::Recommend)),
            )?;
            Ok(true)
        }
        Some("auto") => {
            state.approval_mode = CoshApprovalMode::Auto;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::ApprovalModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::ApprovalModeSetBody, &[("mode", "auto")])],
                Some(mode_footer(state.i18n(), CoshApprovalMode::Auto)),
            )?;
            Ok(true)
        }
        Some("trust") if confirm != Some("confirm") => {
            render_trust_confirmation_required(state.i18n(), output)
        }
        Some("trust") => {
            state.approval_mode = CoshApprovalMode::Trust;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::ApprovalModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::ApprovalModeSetBody, &[("mode", "trust")])],
                Some(mode_footer(state.i18n(), CoshApprovalMode::Trust)),
            )?;
            Ok(true)
        }
        Some(other) => render_notice_panel(
            output,
            state.i18n().t(MessageId::ApprovalModeTitle),
            vec![state
                .i18n()
                .format(MessageId::ApprovalModeUnknownBody, &[("mode", other)])],
            Some(state.i18n().t(MessageId::ApprovalModeUsageFooter)),
        )
        .map(|_| true),
    }
}

fn render_trust_confirmation_required<W: Write>(
    i18n: I18n,
    output: &mut W,
) -> std::io::Result<bool> {
    render_notice_panel(
        output,
        i18n.t(MessageId::ApprovalModeTrustConfirmationTitle),
        vec![
            i18n.t(MessageId::ApprovalModeTrustConfirmationBody)
                .to_string(),
            i18n.t(MessageId::ApprovalModeTrustConfirmationCommandBody)
                .to_string(),
        ],
        Some(i18n.t(MessageId::ApprovalModeTrustConfirmationFooter)),
    )?;
    Ok(true)
}

fn render_analysis_mode_command<W: Write>(
    arg: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None => {
            render_notice_panel(
                output,
                state.i18n().t(MessageId::AnalysisModeTitle),
                vec![state.i18n().format(
                    MessageId::AnalysisModeCurrentBody,
                    &[("mode", state.analysis_mode.label())],
                )],
                Some(state.i18n().t(MessageId::AnalysisModeUsageFooter)),
            )?;
            Ok(true)
        }
        Some("smart") => {
            state.analysis_mode = AnalysisMode::Smart;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::AnalysisModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::AnalysisModeSetBody, &[("mode", "smart")])],
                Some(state.i18n().t(MessageId::AnalysisModeSmartFooter)),
            )?;
            Ok(true)
        }
        Some("auto") => {
            state.analysis_mode = AnalysisMode::Auto;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::AnalysisModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::AnalysisModeSetBody, &[("mode", "auto")])],
                Some(state.i18n().t(MessageId::AnalysisModeAutoFooter)),
            )?;
            Ok(true)
        }
        Some("manual") => {
            state.analysis_mode = AnalysisMode::Manual;
            render_notice_panel(
                output,
                state.i18n().t(MessageId::AnalysisModeTitle),
                vec![state
                    .i18n()
                    .format(MessageId::AnalysisModeSetBody, &[("mode", "manual")])],
                Some(state.i18n().t(MessageId::AnalysisModeManualFooter)),
            )?;
            Ok(true)
        }
        Some(other) => render_notice_panel(
            output,
            state.i18n().t(MessageId::AnalysisModeTitle),
            vec![state
                .i18n()
                .format(MessageId::AnalysisModeUnknownBody, &[("mode", other)])],
            Some(state.i18n().t(MessageId::AnalysisModeUsageFooter)),
        )
        .map(|_| true),
    }
}

pub(crate) fn render_mode_card_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some(action) = mode_card_action_from_event(event) else {
            continue;
        };
        let key = format!(
            "{}:{}",
            stable_event_key("mode-card", event_index, event),
            event.message.as_deref().unwrap_or_default()
        );
        if !state.control.claim_mode_action(key) {
            continue;
        }

        match action {
            ModeCardAction::Focus { id, selected } => {
                let Some(panel) = state
                    .control
                    .pending_mode_panel_mut()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                panel.selected_option = selected.min(2);
                redraw_current_mode_panel(state, output)?;
            }
            ModeCardAction::Set { id, selected } => {
                let Some(panel) = state
                    .control
                    .pending_mode_panel()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                let mode = mode_from_index(selected.min(2));
                if mode == CoshApprovalMode::Trust && state.approval_mode != CoshApprovalMode::Trust
                {
                    let _ = panel;
                    clear_active_mode_panel(state, output)?;
                    state.control.clear_pending_mode_panel();
                    render_trust_confirmation_required(state.i18n(), output)?;
                    write_shell_prompt(state, output)?;
                    output.flush()?;
                    continue;
                }
                let unchanged = mode == state.approval_mode;
                state.approval_mode = mode;
                let label = state.approval_mode.label();
                let _ = panel;
                clear_active_mode_panel(state, output)?;
                state.control.clear_pending_mode_panel();
                let body = if unchanged {
                    vec![state
                        .i18n()
                        .format(MessageId::ApprovalModeRemainsBody, &[("mode", label)])]
                } else {
                    vec![state
                        .i18n()
                        .format(MessageId::ApprovalModeSetBody, &[("mode", label)])]
                };
                render_notice_panel(
                    output,
                    state.i18n().t(MessageId::ApprovalModeCardTitle),
                    body,
                    Some(mode_footer(state.i18n(), state.approval_mode)),
                )?;
                write_shell_prompt(state, output)?;
            }
            ModeCardAction::Cancel { id } => {
                let Some(_panel) = state
                    .control
                    .pending_mode_panel()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                let label = state.approval_mode.label();
                clear_active_mode_panel(state, output)?;
                state.control.clear_pending_mode_panel();
                render_notice_panel(
                    output,
                    state.i18n().t(MessageId::ApprovalModeCardTitle),
                    vec![state
                        .i18n()
                        .format(MessageId::ApprovalModeCancelBody, &[("mode", label)])],
                    Some(state.i18n().t(MessageId::ApprovalModeCancelFooter)),
                )?;
                write_shell_prompt(state, output)?;
            }
        }
        output.flush()?;
    }
    Ok(())
}

fn render_current_mode_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(panel) = state.control.pending_mode_panel() else {
        return Ok(());
    };
    if state.control.active_mode_panel_id() == Some(panel.id.as_str()) {
        return Ok(());
    }

    let marker = |i: usize| {
        if panel.selected_option == i {
            "> "
        } else {
            "  "
        }
    };
    let body = vec![
        state.i18n().format(
            MessageId::ApprovalModeCardCurrentLine,
            &[("mode", state.approval_mode.label())],
        ),
        state.i18n().format(
            MessageId::ApprovalModeCardRecommendLine,
            &[("marker", marker(0))],
        ),
        state.i18n().format(
            MessageId::ApprovalModeCardAutoLine,
            &[("marker", marker(1))],
        ),
        state.i18n().format(
            MessageId::ApprovalModeCardTrustLine,
            &[("marker", marker(2))],
        ),
    ];
    let footer = state.i18n().t(MessageId::ApprovalModeCardFooter);
    render_notice_panel(
        output,
        state.i18n().t(MessageId::ApprovalModeCardTitle),
        body.clone(),
        Some(footer),
    )?;
    state
        .control
        .set_active_mode_panel(panel.id.clone(), notice_height(&body, Some(footer)));
    Ok(())
}

fn redraw_current_mode_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_mode_panel(state, output)?;
    render_current_mode_panel(state, output)
}

fn clear_active_mode_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.control.active_mode_panel_height();
    if height == 0 {
        state.control.clear_active_mode_panel_id();
        return Ok(());
    }

    write!(output, "\x1b[{height}A")?;
    for row in 0..height {
        write!(output, "\r\x1b[2K")?;
        if row + 1 < height {
            write!(output, "\x1b[1B")?;
        }
    }
    if height > 1 {
        write!(output, "\x1b[{}A", height - 1)?;
    }
    write!(output, "\r")?;
    state.control.clear_active_mode_panel();
    Ok(())
}

enum ModeCardAction {
    Focus { id: String, selected: usize },
    Set { id: String, selected: usize },
    Cancel { id: String },
}

fn mode_card_action_from_event(event: &ShellEvent) -> Option<ModeCardAction> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
    {
        return None;
    }

    match event.message.as_deref()? {
        "mode_focus" => {
            let (id, selected) = split_mode_value(event.input.as_deref()?)?;
            Some(ModeCardAction::Focus { id, selected })
        }
        "mode_set" => {
            let (id, selected) = split_mode_value(event.input.as_deref()?)?;
            Some(ModeCardAction::Set { id, selected })
        }
        "mode_cancel" => Some(ModeCardAction::Cancel {
            id: event.input.as_deref()?.to_string(),
        }),
        _ => None,
    }
}

fn split_mode_value(value: &str) -> Option<(String, usize)> {
    let (id, selected) = value.split_once(':')?;
    Some((id.to_string(), selected.parse().ok()?))
}

fn mode_from_index(index: usize) -> CoshApprovalMode {
    match index {
        0 => CoshApprovalMode::Recommend,
        1 => CoshApprovalMode::Auto,
        2 => CoshApprovalMode::Trust,
        _ => CoshApprovalMode::Auto,
    }
}

fn mode_footer(i18n: I18n, mode: CoshApprovalMode) -> &'static str {
    match mode {
        CoshApprovalMode::Recommend => i18n.t(MessageId::ApprovalModeRecommendFooter),
        CoshApprovalMode::Auto => i18n.t(MessageId::ApprovalModeAutoFooter),
        CoshApprovalMode::Trust => i18n.t(MessageId::ApprovalModeTrustFooter),
    }
}

fn notice_height(body: &[String], footer: Option<&str>) -> usize {
    let renderer = RatatuiInlineRenderer::for_terminal();
    let mut lines = body
        .iter()
        .flat_map(|line| renderer.markdown_text_lines(line))
        .collect::<Vec<_>>();
    if let Some(footer) = footer {
        lines.extend(renderer.markdown_text_lines(footer));
    }
    lines.len().max(1) + 2
}

fn render_notice_panel<W: Write>(
    output: &mut W,
    title: &str,
    body: Vec<String>,
    footer: Option<&str>,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal().write_notice_panel(
        output,
        NoticePanelModel {
            title,
            body,
            footer,
        },
    )
}
