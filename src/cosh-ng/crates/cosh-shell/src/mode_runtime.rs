use super::runtime_state::RuntimeModePanel;
use super::slash_runtime::write_shell_prompt;
use super::*;

pub(super) fn render_mode_command<W: Write>(
    arg: Option<&str>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    match arg {
        None => {
            state.pending_mode_panel = Some(RuntimeModePanel {
                id: "mode".to_string(),
                selected_option: mode_index(state.approval_mode),
            });
            render_current_mode_panel(state, output)?;
            Ok(false)
        }
        Some("ask") => {
            state.approval_mode = ApprovalMode::Ask;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Approval mode",
                vec!["Mode set to ask.".to_string()],
                Some("Every Agent action/tool request requires confirmation."),
            )?;
            Ok(true)
        }
        Some("auto") => {
            state.approval_mode = ApprovalMode::Auto;
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Approval mode",
                vec!["Mode set to auto.".to_string()],
                Some(
                    "Only low-risk read-only Bash tools can skip approval; risky requests still ask.",
                ),
            )?;
            Ok(true)
        }
        Some(other) => RatatuiInlineRenderer::for_terminal()
            .write_notice(
                output,
                "Approval mode",
                vec![format!("Unknown mode: {other}")],
                Some("Use /mode ask or /mode auto."),
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
                let unchanged = mode == state.approval_mode;
                state.approval_mode = mode;
                let label = state.approval_mode.label();
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
                    "Approval mode",
                    body,
                    Some(mode_footer(state.approval_mode)),
                )?;
                write_shell_prompt(output)?;
            }
            ModeCardAction::Cancel { id } => {
                if !state
                    .pending_mode_panel
                    .as_ref()
                    .is_some_and(|panel| panel.id == id)
                {
                    continue;
                }
                let label = state.approval_mode.label();
                clear_active_mode_panel(state, output)?;
                state.pending_mode_panel = None;
                RatatuiInlineRenderer::for_terminal().write_notice(
                    output,
                    "Approval mode",
                    vec![format!("Mode unchanged: {label}.")],
                    Some("No shell command ran."),
                )?;
                write_shell_prompt(output)?;
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

    let ask_marker = if panel.selected_option == 0 {
        "> "
    } else {
        "  "
    };
    let auto_marker = if panel.selected_option == 1 {
        "> "
    } else {
        "  "
    };
    let body = vec![
        format!("Current: {}", state.approval_mode.label()),
        format!("{ask_marker}[ ask  ] Confirm every Agent action/tool request"),
        format!("{auto_marker}[ auto ] Auto-allow low-risk read-only Bash tools"),
    ];
    let footer = "Keys: Left/Right select | Enter apply | Esc cancel";
    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "Approval mode",
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

fn mode_index(mode: ApprovalMode) -> usize {
    match mode {
        ApprovalMode::Ask => 0,
        ApprovalMode::Auto => 1,
    }
}

fn mode_from_index(index: usize) -> ApprovalMode {
    match index {
        1 => ApprovalMode::Auto,
        _ => ApprovalMode::Ask,
    }
}

fn mode_footer(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Every Agent action/tool request requires confirmation.",
        ApprovalMode::Auto => {
            "Only low-risk read-only Bash tools can skip approval; risky requests still ask."
        }
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
