use super::runtime_state::RuntimeModePanel;
use super::slash_runtime::write_shell_prompt;
use super::*;

pub(super) fn render_mode_command<W: Write>(
    arg: Option<&str>,
    sub: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    if arg == Some("analysis") {
        return render_analysis_mode_command(sub, state, output);
    }
    match arg {
        None => {
            state.pending_mode_panel = Some(RuntimeModePanel {
                id: format!("mode-{}", state.handled_mode_actions.len() + 1),
                selected_option: if state.approval_mode == CoshApprovalMode::Suggest {
                    0
                } else {
                    1
                },
            });
            render_current_mode_panel(state, output)?;
            Ok(false)
        }
        Some("recommend") => {
            state.approval_mode = CoshApprovalMode::Suggest;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "User mode",
                vec!["Mode set to recommend.".to_string()],
                Some(user_mode_footer(CoshApprovalMode::Suggest)),
            )?;
            Ok(true)
        }
        Some("agent") => {
            state.approval_mode = CoshApprovalMode::Auto;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "User mode",
                vec!["Mode set to agent.".to_string()],
                Some(user_mode_footer(CoshApprovalMode::Auto)),
            )?;
            Ok(true)
        }
        Some("suggest") => {
            state.approval_mode = CoshApprovalMode::Suggest;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Legacy approval strategy",
                vec!["Strategy set to suggest.".to_string()],
                Some(mode_footer(CoshApprovalMode::Suggest)),
            )?;
            Ok(true)
        }
        Some("ask") => {
            state.approval_mode = CoshApprovalMode::Ask;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Legacy approval strategy",
                vec!["Strategy set to ask.".to_string()],
                Some(mode_footer(CoshApprovalMode::Ask)),
            )?;
            Ok(true)
        }
        Some("auto") => {
            state.approval_mode = CoshApprovalMode::Auto;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Legacy approval strategy",
                vec!["Strategy set to auto.".to_string()],
                Some(mode_footer(CoshApprovalMode::Auto)),
            )?;
            Ok(true)
        }
        Some("trust") => {
            state.approval_mode = CoshApprovalMode::Trust;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Legacy approval strategy",
                vec!["Strategy set to trust.".to_string()],
                Some(mode_footer(CoshApprovalMode::Trust)),
            )?;
            Ok(true)
        }
        Some(other) => RatatuiInlineRenderer::for_terminal()
            .write_notice(
                output,
                "Mode",
                vec![format!("Unknown mode: {other}")],
                Some("Use /mode recommend|agent. Legacy: /approval-mode suggest|ask|auto|trust."),
            )
            .map(|_| true),
    }
}

fn render_analysis_mode_command<W: Write>(
    arg: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None => {
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Analysis mode",
                vec![format!("Current: {}", state.analysis_mode.label())],
                Some("Use /mode analysis smart|auto|manual."),
            )?;
            Ok(true)
        }
        Some("smart") => {
            state.analysis_mode = AnalysisMode::Smart;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Analysis mode",
                vec!["Mode set to smart.".to_string()],
                Some("Hooks evaluate on failure; findings shown for review."),
            )?;
            Ok(true)
        }
        Some("auto") => {
            state.analysis_mode = AnalysisMode::Auto;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Analysis mode",
                vec!["Mode set to auto.".to_string()],
                Some("Hooks evaluate on failure; Agent auto-triggered for failed commands."),
            )?;
            Ok(true)
        }
        Some("manual") => {
            state.analysis_mode = AnalysisMode::Manual;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Analysis mode",
                vec!["Mode set to manual.".to_string()],
                Some("Hooks and automatic analysis disabled; use slash commands to trigger."),
            )?;
            Ok(true)
        }
        Some(other) => RatatuiInlineRenderer::for_terminal()
            .write_notice(
                output,
                "Analysis mode",
                vec![format!("Unknown analysis mode: {other}")],
                Some("Use /mode analysis smart|auto|manual."),
            )
            .map(|_| true),
    }
}

pub(super) fn render_mode_card_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let Some(action) = mode_card_action_from_event(event) else {
            continue;
        };
        let key = format!(
            "{}:{}",
            stable_event_key("mode-card", idx, event),
            event.message.as_deref().unwrap_or_default()
        );
        if !state.handled_mode_actions.insert(key) {
            continue;
        }

        match action {
            ModeCardAction::Focus { id, selected } => {
                let Some(panel) = state
                    .pending_mode_panel
                    .as_mut()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                panel.selected_option = selected.min(1);
                redraw_current_mode_panel(state, output)?;
            }
            ModeCardAction::Set { id, selected } => {
                let Some(panel) = state
                    .pending_mode_panel
                    .as_ref()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                let mode = mode_from_index(selected.min(1));
                let unchanged = mode.user_mode_label() == state.approval_mode.user_mode_label();
                state.approval_mode = mode;
                let label = state.approval_mode.user_mode_label();
                let _ = panel;
                clear_active_mode_panel(state, output)?;
                state.pending_mode_panel = None;
                let body = if unchanged {
                    vec![format!("Mode remains {label}.")]
                } else {
                    vec![format!("Mode set to {label}.")]
                };
                RatatuiInlineRenderer::for_terminal().write_notice(
                    output,
                    "User mode",
                    body,
                    Some(user_mode_footer(state.approval_mode)),
                )?;
                write_shell_prompt(state, output)?;
            }
            ModeCardAction::Cancel { id } => {
                let Some(_panel) = state
                    .pending_mode_panel
                    .as_ref()
                    .filter(|panel| panel.id == id)
                else {
                    continue;
                };
                let label = state.approval_mode.user_mode_label();
                clear_active_mode_panel(state, output)?;
                state.pending_mode_panel = None;
                RatatuiInlineRenderer::for_terminal().write_notice(
                    output,
                    "User mode",
                    vec![format!("Mode unchanged: {label}.")],
                    Some("No shell command ran."),
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
    let Some(panel) = state.pending_mode_panel.as_ref() else {
        return Ok(());
    };
    if state.active_mode_panel_id.as_deref() == Some(panel.id.as_str()) {
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
        format!("Current: {}", state.approval_mode.user_mode_label()),
        format!("{}[ recommend ] Explain and suggest only", marker(0)),
        format!(
            "{}[ agent     ] Use tools with cosh-shell governance",
            marker(1)
        ),
    ];
    let footer = "Keys: Left/Right select | Enter apply | Esc cancel";
    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "User mode",
        body.clone(),
        Some(footer),
    )?;
    state.active_mode_panel_id = Some(panel.id.clone());
    state.active_mode_panel_height = notice_height(&body, Some(footer));
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
    let height = state.active_mode_panel_height;
    if height == 0 {
        state.active_mode_panel_id = None;
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
    state.active_mode_panel_id = None;
    state.active_mode_panel_height = 0;
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
        0 => CoshApprovalMode::Suggest,
        1 => CoshApprovalMode::Auto,
        _ => CoshApprovalMode::Auto,
    }
}

fn user_mode_footer(mode: CoshApprovalMode) -> &'static str {
    match mode.user_mode_label() {
        "recommend" => "Agent explains and suggests; no tool calls are emitted.",
        _ => "Agent can use tools; cosh-shell handles safe auto-approval and approval cards.",
    }
}

fn mode_footer(mode: CoshApprovalMode) -> &'static str {
    match mode {
        CoshApprovalMode::Suggest => "Agent suggests actions; no execution occurs.",
        CoshApprovalMode::Ask => "Every Agent action/tool request requires confirmation.",
        CoshApprovalMode::Auto => {
            "Only low-risk read-only Bash tools can skip approval; risky requests still ask."
        }
        CoshApprovalMode::Trust => "All Agent actions are auto-approved without confirmation.",
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
