use cosh_shell::{
    agent_render::{RecommendationActionPanelModel, RecommendationPanelModel},
    recommendation_action_from_event, RecommendationActionKind,
};

use super::*;

pub(super) fn render_selection_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let Some(action) = recommendation_action_from_event(event) else {
            continue;
        };

        let key = format!("select-{idx}");
        if !state.handled_selections.insert(key) {
            continue;
        }

        if state
            .selectable_after_event_index
            .map(|available_after| idx <= available_after)
            .unwrap_or(true)
            || state.selectable_commands.is_empty()
        {
            render_recommendation_unavailable(
                "No selectable recommendation",
                vec!["No selectable recommendation is available yet".to_string()],
                output,
            )?;
            output.flush()?;
            continue;
        }

        let Some(command) = state.selectable_commands.get(action.index - 1) else {
            render_recommendation_unavailable(
                "Recommendation unavailable",
                vec![format!(
                    "Recommendation {} is not available; choose 1..{}",
                    action.index,
                    state.selectable_commands.len()
                )],
                output,
            )?;
            output.flush()?;
            continue;
        };

        render_recommendation_action(action.kind, action.index, command, output)?;
        output.flush()?;
    }

    Ok(())
}

fn render_recommendation_action<W: Write>(
    kind: RecommendationActionKind,
    index: usize,
    command: &str,
    output: &mut W,
) -> std::io::Result<()> {
    let renderer = RatatuiInlineRenderer::for_terminal();
    match kind {
        RecommendationActionKind::Select => {
            renderer.write_recommendation_action_panel(
                output,
                RecommendationActionPanelModel {
                    title: "Recommendation selected",
                    primary: format!("Selected recommendation {index}"),
                    command: Some(command),
                    message: "Display-only: command was not executed; copy or re-enter it to run",
                },
            )?;
        }
    };
    Ok(())
}

fn render_recommendation_unavailable<W: Write>(
    title: &str,
    body: Vec<String>,
    output: &mut W,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal().write_notice(output, title, body, None)
}

pub(super) fn record_selectable_recommendations(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    selectable_after_event_index: Option<usize>,
) {
    let commands = selectable_commands_from_events(governed_events);
    if commands.is_empty() {
        return;
    }

    state.selectable_commands = commands;
    state.selectable_after_event_index = selectable_after_event_index;
}

pub(super) fn render_selectable_recommendations<W: Write>(
    governed_events: &[GovernedEvent],
    output: &mut W,
) -> std::io::Result<()> {
    let commands = selectable_commands_from_events(governed_events);
    if commands.is_empty() {
        return Ok(());
    }

    RatatuiInlineRenderer::for_terminal().write_recommendation_panel(
        output,
        RecommendationPanelModel {
            commands: &commands,
        },
    )?;
    Ok(())
}

fn selectable_commands_from_events(governed_events: &[GovernedEvent]) -> Vec<String> {
    governed_events
        .iter()
        .filter_map(|event| match &event.event {
            AgentEvent::Recommendation { commands, .. } => Some(commands.as_slice()),
            _ => None,
        })
        .flatten()
        .filter(|command| !command.trim().is_empty())
        .cloned()
        .collect()
}
