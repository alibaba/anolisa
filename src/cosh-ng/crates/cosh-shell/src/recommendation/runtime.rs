use cosh_shell::agent_render::{RecommendationActionPanelModel, RecommendationPanelModel};
use cosh_shell::parser::{recommendation_action_from_event, RecommendationActionKind};

use crate::runtime::prelude::*;

pub(crate) fn render_selection_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some(action) = recommendation_action_from_event(event) else {
            continue;
        };

        let key = format!("select-{event_index}");
        if !state.handled_selections.insert(key) {
            continue;
        }

        if state
            .control
            .selectable_commands_available_after()
            .map(|available_after| event_index <= available_after)
            .unwrap_or(true)
            || !state.control.has_selectable_commands()
        {
            let i18n = state.i18n();
            render_recommendation_unavailable(
                state.language,
                i18n.t(cosh_shell::MessageId::RecommendationNoSelectableTitle),
                vec![i18n
                    .t(cosh_shell::MessageId::RecommendationNoSelectableBody)
                    .to_string()],
                output,
            )?;
            output.flush()?;
            continue;
        }

        let Some(command) = state.control.selectable_command(action.index - 1) else {
            let i18n = state.i18n();
            let index = action.index.to_string();
            let total = state.control.selectable_command_count().to_string();
            render_recommendation_unavailable(
                state.language,
                i18n.t(cosh_shell::MessageId::RecommendationUnavailableTitle),
                vec![i18n.format(
                    cosh_shell::MessageId::RecommendationUnavailableBody,
                    &[("index", index.as_str()), ("total", total.as_str())],
                )],
                output,
            )?;
            output.flush()?;
            continue;
        };

        render_recommendation_action(state.language, action.kind, action.index, command, output)?;
        output.flush()?;
    }

    Ok(())
}

fn render_recommendation_action<W: Write>(
    language: cosh_shell::Language,
    kind: RecommendationActionKind,
    index: usize,
    command: &str,
    output: &mut W,
) -> std::io::Result<()> {
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(language);
    let i18n = cosh_shell::I18n::new(language);
    let index = index.to_string();
    let (title, primary_id, message_id) = match kind {
        RecommendationActionKind::Select => (
            cosh_shell::MessageId::RecommendationSelectedTitle,
            cosh_shell::MessageId::RecommendationSelectedBody,
            cosh_shell::MessageId::RecommendationDisplayOnlyBody,
        ),
        RecommendationActionKind::Copy => (
            cosh_shell::MessageId::RecommendationCopiedTitle,
            cosh_shell::MessageId::RecommendationCopiedBody,
            cosh_shell::MessageId::RecommendationCopyOnlyBody,
        ),
        RecommendationActionKind::Insert => (
            cosh_shell::MessageId::RecommendationInsertTitle,
            cosh_shell::MessageId::RecommendationInsertBody,
            cosh_shell::MessageId::RecommendationInsertOnlyBody,
        ),
        RecommendationActionKind::Details => (
            cosh_shell::MessageId::RecommendationDetailsTitle,
            cosh_shell::MessageId::RecommendationDetailsBody,
            cosh_shell::MessageId::RecommendationDetailsOnlyBody,
        ),
    };
    renderer.write_recommendation_action_panel(
        output,
        RecommendationActionPanelModel {
            title: i18n.t(title),
            primary: i18n.format(primary_id, &[("index", index.as_str())]),
            command: Some(command),
            message: i18n.t(message_id),
        },
    )?;
    Ok(())
}

fn render_recommendation_unavailable<W: Write>(
    language: cosh_shell::Language,
    title: &str,
    body: Vec<String>,
    output: &mut W,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title,
                body,
                footer: None,
            },
        )
}

pub(crate) fn record_selectable_recommendations(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    selectable_after_event_index: Option<usize>,
) {
    let commands = selectable_commands_from_events(governed_events);
    if commands.is_empty() {
        return;
    }

    state
        .control
        .remember_selectable_commands(commands, selectable_after_event_index);
}

pub(crate) fn render_selectable_recommendations<W: Write>(
    governed_events: &[GovernedEvent],
    language: cosh_shell::Language,
    output: &mut W,
) -> std::io::Result<()> {
    let commands = selectable_commands_from_events(governed_events);
    if commands.is_empty() {
        return Ok(());
    }

    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_recommendation_panel(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn recommendation_card_event(index: usize, message: &str) -> ShellEvent {
        let mut event = ShellEvent::user_input_intercepted("session-1", index.to_string());
        event.component = Some("card".to_string());
        event.message = Some(message.to_string());
        event
    }

    #[test]
    fn card_insert_renders_pending_prompt_guidance_without_executing() {
        let mut state = InlineState::default();
        state
            .control
            .remember_selectable_commands(vec!["echo SHOULD_NOT_RUN".to_string()], Some(0));
        let event = recommendation_card_event(1, "recommendation_insert");
        let mut output = Vec::new();

        render_selection_actions(&[event], &mut state, &mut output, 1)
            .expect("render insert action");

        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("Recommendation insert"), "{output}");
        assert!(
            output.contains("Prepared recommendation 1 for manual input"),
            "{output}"
        );
        assert!(output.contains("echo SHOULD_NOT_RUN"), "{output}");
        assert!(
            output.contains(
                "Insert is pending editable input only; nothing was submitted or written to the child shell."
            ),
            "{output}"
        );
        assert!(!output.contains("$ echo SHOULD_NOT_RUN"), "{output}");
    }

    #[test]
    fn card_details_renders_recommendation_details_without_executing() {
        let mut state = InlineState::default();
        state
            .control
            .remember_selectable_commands(vec!["pwd".to_string()], Some(0));
        let event = recommendation_card_event(1, "recommendation_details");
        let mut output = Vec::new();

        render_selection_actions(&[event], &mut state, &mut output, 1)
            .expect("render details action");

        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("Recommendation details"), "{output}");
        assert!(output.contains("Details for recommendation 1"), "{output}");
        assert!(output.contains("pwd"), "{output}");
        assert!(output.contains("Details-only"), "{output}");
        assert!(!output.contains("$ pwd"), "{output}");
    }
}
