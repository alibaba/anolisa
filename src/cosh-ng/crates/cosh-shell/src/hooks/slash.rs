use std::io::Write;

use cosh_shell::{
    agent_render::{NoticePanelModel, RatatuiInlineRenderer},
    types::CommandBlock,
    AdapterInstance,
};

use super::presentation::render_consultation_details;
use super::runtime::{
    consultation_from_hint, record_hook_display_event_for_consultation,
    start_agent_for_hook_consultation,
};
use crate::runtime::state::{InlineState, RuntimeHookDisplayAction};

pub(crate) fn handle_command_hook_hint_action<W: Write>(
    action: &str,
    hint_id: &str,
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let i18n = state.i18n();
    let Some(hint) = state
        .hooks
        .findings
        .iter()
        .find(|hint| hint.id == hint_id)
        .cloned()
    else {
        return RatatuiInlineRenderer::for_terminal().write_notice_panel(
            output,
            NoticePanelModel {
                title: i18n.t(cosh_shell::MessageId::HookHintTitle),
                body: vec![i18n.format(
                    cosh_shell::MessageId::HookHintNotFoundBody,
                    &[("hint_id", hint_id)],
                )],
                footer: Some(i18n.t(cosh_shell::MessageId::HookHintNotFoundFooter)),
            },
        );
    };
    let Some(consultation) = consultation_from_hint(&hint) else {
        return RatatuiInlineRenderer::for_terminal().write_notice_panel(
            output,
            NoticePanelModel {
                title: i18n.t(cosh_shell::MessageId::HookHintTitle),
                body: vec![i18n.format(
                    cosh_shell::MessageId::HookHintNoFindingBody,
                    &[("hint_id", hint_id)],
                )],
                footer: None,
            },
        );
    };

    match action {
        "analyze" => {
            let Some(block) = blocks
                .iter()
                .find(|block| block.id == consultation.block_id)
            else {
                return RatatuiInlineRenderer::for_terminal().write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: i18n.t(cosh_shell::MessageId::HookHintTitle),
                        body: vec![i18n.format(
                            cosh_shell::MessageId::HookHintBlockUnavailableBody,
                            &[("block_id", consultation.block_id.as_str())],
                        )],
                        footer: None,
                    },
                );
            };
            record_hook_display_event_for_consultation(
                &consultation,
                RuntimeHookDisplayAction::Analyzed,
                state,
            );
            start_agent_for_hook_consultation(block, blocks, &consultation, adapter, state, output)
        }
        "details" => render_consultation_details(&consultation, state, output),
        "ignore" => {
            state
                .hooks
                .ignored_cards
                .insert(consultation.suppression_key.clone());
            record_hook_display_event_for_consultation(
                &consultation,
                RuntimeHookDisplayAction::Ignored,
                state,
            );
            RatatuiInlineRenderer::for_terminal().write_notice_panel(
                output,
                NoticePanelModel {
                    title: i18n.t(cosh_shell::MessageId::HookHintIgnoredTitle),
                    body: vec![i18n.format(
                        cosh_shell::MessageId::HookHintIgnoredBody,
                        &[("hint_id", hint_id)],
                    )],
                    footer: Some(i18n.t(cosh_shell::MessageId::HookHintIgnoredFooter)),
                },
            )
        }
        _ => RatatuiInlineRenderer::for_terminal().write_notice_panel(
            output,
            NoticePanelModel {
                title: i18n.t(cosh_shell::MessageId::HookHintUsageTitle),
                body: vec![i18n.t(cosh_shell::MessageId::HookHintUsageBody).to_string()],
                footer: None,
            },
        ),
    }
}
