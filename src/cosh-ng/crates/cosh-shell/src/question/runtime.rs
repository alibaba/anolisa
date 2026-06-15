use crate::question_choices::{
    question_choice_count, question_custom_answer_index, toggle_question_option,
};
use crate::runtime::prelude::*;
use cosh_shell::parser::agent_request_from_intercepted_input;
use cosh_shell::raw_input::RawInputCapture;
use cosh_shell::{
    adapter::{ApprovalDecision, ApprovalResponse},
    agent_render::{QuestionAnswerPanelModel, QuestionPanelModel, RatatuiInlineRenderer},
    types::{
        AgentEvent, AgentRequest, GovernedEvent, QuestionSelectionMode, ShellEvent, ShellEventKind,
    },
    AdapterInstance,
};

#[derive(Debug, Clone)]
pub(crate) struct RuntimeUserQuestion {
    id: String,
    question: String,
    options: Vec<String>,
    selected_option: usize,
    selected_options: Vec<usize>,
    custom_answer: String,
    allow_free_text: bool,
    selection_mode: QuestionSelectionMode,
    provider_request_id: Option<String>,
    answer: Option<String>,
}

pub(crate) struct QuestionAnswerRun {
    question_id: String,
    question: String,
    answer: String,
    provider_request_id: Option<String>,
    pub(crate) request: AgentRequest,
}

pub(crate) fn pending_question_capture(state: &InlineState) -> Option<RawInputCapture> {
    if let Some(question_id) = state.questions.pending_id.as_ref() {
        if let Some(question) = state
            .questions
            .items
            .iter()
            .find(|question| question.id == *question_id && question.answer.is_none())
        {
            return Some(RawInputCapture::Question {
                id: question.id.clone(),
                option_count: question.options.len(),
                allow_free_text: question.allow_free_text,
                multiple: question.selection_mode == QuestionSelectionMode::Multiple,
            });
        }
    }

    None
}

pub(crate) fn has_pending_question(state: &InlineState) -> bool {
    state.questions.pending_id.is_some()
}

pub(crate) fn render_question_answer_actions<W: Write>(
    events: &[ShellEvent],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        if !is_question_answer_card_event(event) {
            continue;
        }

        let key = stable_event_key("question-answer", event_index, event);
        if !state.questions.handled_answers.insert(key) {
            continue;
        }

        let Some(answer_run) =
            agent_request_from_pending_question_answer(event, event_index, state)
        else {
            let i18n = state.i18n();
            RatatuiInlineRenderer::for_terminal()
                .with_language(state.language)
                .write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: i18n.t(cosh_shell::MessageId::QuestionNoPendingTitle),
                        body: vec![i18n
                            .t(cosh_shell::MessageId::QuestionNoPendingBody)
                            .to_string()],
                        footer: None,
                    },
                )?;
            output.flush()?;
            continue;
        };

        render_question_answer_notice(state, &answer_run, output)?;
        if respond_question_answer_to_provider(state, &answer_run) {
            output.flush()?;
            continue;
        }
        stop_active_agent_run_without_rendering(state, output)?;
        start_agent_run(
            &answer_run.request,
            adapter,
            state,
            output,
            Some(event_index),
        )?;
        output.flush()?;
    }

    Ok(())
}

fn respond_question_answer_to_provider(
    state: &InlineState,
    answer_run: &QuestionAnswerRun,
) -> bool {
    let Some(request_id) = answer_run.provider_request_id.as_ref() else {
        return false;
    };
    let Some(active_run) = state.agent_run.active.as_ref() else {
        return true;
    };
    active_run
        .handle
        .respond_approval(ApprovalResponse {
            request_id: request_id.clone(),
            tool_use_id: None,
            tool_input: None,
            decision: ApprovalDecision::Answer {
                answer: answer_run.answer.clone(),
            },
        })
        .is_ok()
}

pub(crate) fn render_question_cancel_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some(question_id) = question_cancel_from_event(event) else {
            continue;
        };

        let key = stable_event_key("question-cancel", event_index, event);
        if !state.questions.handled_cancellations.insert(key) {
            continue;
        }

        let Some(question_index) = state
            .questions
            .items
            .iter()
            .position(|question| question.id == question_id && question.answer.is_none())
        else {
            continue;
        };

        clear_active_question_panel(state, output)?;
        state.questions.items[question_index].answer = Some(String::new());
        if state.questions.pending_id.as_deref() == Some(question_id.as_str()) {
            state.questions.pending_id = None;
        }
        state.questions.active_panel_id = None;
        state.questions.active_panel_height = 0;
        stop_active_agent_run_without_rendering(state, output)?;
        state.agent_run.needs_prompt_after_run = true;
        output.flush()?;
    }

    Ok(())
}

pub(crate) fn render_question_focus_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some((id, selected_option)) = question_focus_from_event(event) else {
            continue;
        };

        let key = stable_event_key("question-focus", event_index, event);
        if !state.questions.handled_focus.insert(key) {
            continue;
        }

        let Some(question) = state
            .questions
            .items
            .iter_mut()
            .find(|question| question.id == id && question.answer.is_none())
        else {
            continue;
        };

        let choice_count = question_choice_count(question.options.len(), question.allow_free_text);
        if choice_count == 0 {
            continue;
        }
        question.selected_option = selected_option.min(choice_count - 1);
        redraw_current_question(state, output)?;
        output.flush()?;
    }

    Ok(())
}

pub(crate) fn render_question_toggle_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some((id, selected_option)) = question_toggle_from_event(event) else {
            continue;
        };

        let key = stable_event_key("question-toggle", event_index, event);
        if !state.questions.handled_focus.insert(key) {
            continue;
        }

        let Some(question) = state
            .questions
            .items
            .iter_mut()
            .find(|question| question.id == id && question.answer.is_none())
        else {
            continue;
        };
        if question.selection_mode != QuestionSelectionMode::Multiple {
            continue;
        }
        if selected_option >= question.options.len() {
            continue;
        }
        toggle_question_option(&mut question.selected_options, selected_option);
        redraw_current_question(state, output)?;
        output.flush()?;
    }

    Ok(())
}

pub(crate) fn render_question_input_actions<W: Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some((id, text)) = question_input_from_event(event) else {
            continue;
        };

        let key = stable_event_key("question-input", event_index, event);
        if !state.questions.handled_focus.insert(key) {
            continue;
        }

        let Some(question) = state
            .questions
            .items
            .iter_mut()
            .find(|question| question.id == id && question.answer.is_none())
        else {
            continue;
        };
        if !question.allow_free_text {
            continue;
        }
        question.custom_answer = text;
        if let Some(custom_idx) =
            question_custom_answer_index(question.options.len(), question.allow_free_text)
        {
            question.selected_option = custom_idx;
        }
        redraw_current_question(state, output)?;
        output.flush()?;
    }

    Ok(())
}

fn question_focus_from_event(event: &ShellEvent) -> Option<(String, usize)> {
    question_card_event(event, "focus")
}

fn question_toggle_from_event(event: &ShellEvent) -> Option<(String, usize)> {
    question_card_event(event, "toggle")
}

fn question_input_from_event(event: &ShellEvent) -> Option<(String, String)> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
        || event.message.as_deref() != Some("input")
    {
        return None;
    }

    let (id, text) = event.input.as_deref()?.split_once(':')?;
    Some((id.trim().to_string(), text.to_string()))
}

fn question_cancel_from_event(event: &ShellEvent) -> Option<String> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
        || event.message.as_deref() != Some("question_cancel")
    {
        return None;
    }
    Some(event.input.as_deref()?.trim().to_string())
}

fn question_card_event(event: &ShellEvent, message: &str) -> Option<(String, usize)> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
        || event.message.as_deref() != Some(message)
    {
        return None;
    }

    let (id, selected) = event.input.as_deref()?.split_once(':')?;
    let selected = selected.trim().parse::<usize>().ok()?;
    Some((id.trim().to_string(), selected))
}

fn redraw_current_question<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_question_panel(state, output)?;
    let Some(question_id) = state.questions.pending_id.clone() else {
        return Ok(());
    };
    render_user_questions(state, &[question_id], output)
}

fn clear_active_question_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.questions.active_panel_height;
    if height == 0 {
        state.questions.active_panel_id = None;
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
    state.questions.active_panel_id = None;
    state.questions.active_panel_height = 0;
    Ok(())
}

pub(crate) fn agent_request_from_pending_question_answer(
    event: &ShellEvent,
    sequence: usize,
    state: &mut InlineState,
) -> Option<QuestionAnswerRun> {
    let question_id = state.questions.pending_id.clone()?;
    let question_index = state
        .questions
        .items
        .iter()
        .position(|question| question.id == question_id && question.answer.is_none())?;
    let raw_answer = question_answer_text_from_event(event)?;
    let answer = resolve_question_answer(&state.questions.items[question_index], &raw_answer)?;
    let question = state.questions.items[question_index].question.clone();
    let mut request = agent_request_from_intercepted_input(event, sequence, true)?;
    let user_input = format!("Answer to pending Agent question: {question}\nUser answer: {answer}");
    request.id = format!("agent-answer-{question_id}-{sequence}");
    request.command_block.id = format!("answer-{question_id}-{sequence}");
    request.command_block.command = user_input.clone();
    request.user_input = Some(user_input);
    state.questions.items[question_index].answer = Some(answer.clone());
    state.questions.pending_id = None;
    state.questions.active_panel_id = None;
    state.questions.active_panel_height = 0;

    Some(QuestionAnswerRun {
        question_id,
        question,
        answer,
        provider_request_id: state.questions.items[question_index]
            .provider_request_id
            .clone(),
        request,
    })
}

fn question_answer_text_from_event(event: &ShellEvent) -> Option<String> {
    let input = event.input.as_deref()?.trim();
    if input.is_empty() {
        return None;
    }

    if event.component.as_deref() == Some("card") && event.message.as_deref() == Some("answer") {
        return Some(input.to_string());
    }

    None
}

fn resolve_question_answer(question: &RuntimeUserQuestion, raw_answer: &str) -> Option<String> {
    if question.selection_mode == QuestionSelectionMode::Multiple {
        if let Some(answer) = resolve_multi_question_answer(question, raw_answer) {
            return Some(answer);
        }
    }

    if let Ok(index) = raw_answer.trim().parse::<usize>() {
        return question.options.get(index.saturating_sub(1)).cloned();
    }

    if let Some(option) = question
        .options
        .iter()
        .find(|option| option.eq_ignore_ascii_case(raw_answer.trim()))
    {
        return Some(option.clone());
    }

    if question.allow_free_text {
        Some(raw_answer.trim().to_string())
    } else {
        None
    }
}

fn resolve_multi_question_answer(
    question: &RuntimeUserQuestion,
    raw_answer: &str,
) -> Option<String> {
    let (indices_text, custom_answer) = raw_answer
        .split_once('\n')
        .map(|(indices, custom)| (indices.trim(), custom.trim()))
        .unwrap_or((raw_answer.trim(), ""));
    let indices = indices_text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;

    let mut answers = indices
        .into_iter()
        .filter_map(|index| question.options.get(index.saturating_sub(1)).cloned())
        .collect::<Vec<_>>();
    if !custom_answer.is_empty() && question.allow_free_text {
        answers.push(custom_answer.to_string());
    }
    if answers.is_empty() {
        None
    } else {
        Some(answers.join(", "))
    }
}

pub(crate) fn render_question_answer_notice<W: Write>(
    state: &mut InlineState,
    answer_run: &QuestionAnswerRun,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_question_panel(state, output)?;
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_question_answer_panel(
            output,
            QuestionAnswerPanelModel {
                id: &answer_run.question_id,
                question: &answer_run.question,
                answer: &answer_run.answer,
                message: "",
            },
        )?;
    Ok(())
}

fn is_question_answer_card_event(event: &ShellEvent) -> bool {
    if event.component.as_deref() == Some("card") {
        return event.message.as_deref() == Some("answer");
    }
    false
}

pub(crate) fn record_user_questions(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
) -> Vec<String> {
    let mut ids = Vec::new();
    for event in governed_events {
        let AgentEvent::UserQuestion {
            run_id: _,
            provider_request_id,
            question,
            options,
            allow_free_text,
            selection_mode,
        } = &event.event
        else {
            continue;
        };
        let id = next_question_id(state);
        state.questions.items.push(RuntimeUserQuestion {
            id: id.clone(),
            question: question.clone(),
            options: options.clone(),
            selected_option: 0,
            selected_options: Vec::new(),
            custom_answer: String::new(),
            allow_free_text: *allow_free_text,
            selection_mode: *selection_mode,
            provider_request_id: provider_request_id.clone(),
            answer: None,
        });
        state.questions.pending_id = Some(id.clone());
        ids.push(id);
    }
    ids
}

fn next_question_id(state: &InlineState) -> String {
    format!("q-{}", state.questions.items.len() + 1)
}

pub(crate) fn render_user_questions<W: Write>(
    state: &mut InlineState,
    question_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    for question_id in question_ids {
        let Some(question) = state
            .questions
            .items
            .iter()
            .find(|question| question.id == *question_id)
        else {
            continue;
        };

        let height = RatatuiInlineRenderer::for_terminal()
            .with_language(state.language)
            .write_question_panel(
                output,
                QuestionPanelModel {
                    id: &question.id,
                    question: &question.question,
                    options: &question.options,
                    selected_option: question.selected_option,
                    selected_options: &question.selected_options,
                    custom_answer: &question.custom_answer,
                    allow_free_text: question.allow_free_text,
                    selection_mode: question.selection_mode,
                },
            )?;
        state.questions.active_panel_id = Some(question.id.clone());
        state.questions.active_panel_height = height;
    }
    Ok(())
}
