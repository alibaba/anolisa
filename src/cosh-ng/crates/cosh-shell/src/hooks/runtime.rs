use crate::agent::failed_command::FailedCommandAgentStartOptions;
use crate::runtime::prelude::*;
#[cfg(test)]
use crate::runtime::state::HookSuppressionRecord;
#[cfg(test)]
use crate::runtime::state::InterruptionBudgetRecord;
#[cfg(test)]
use crate::runtime::state::{hook_feedback_group_key, HookFeedback};
use crate::runtime::state::{
    PendingConsultation, PendingConsultationState, RuntimeHookDisplay, RuntimeHookDisplayAction,
    RuntimeHookDisplayEvent, RuntimeHookFinding,
};
use cosh_shell::hook_types::{FindingSeverity, HookFinding};

use super::detector::{aggregate_hook_findings, refresh_aggregate_metadata};
#[cfg(test)]
use super::feedback::{
    apply_session_interruption_policy, decide_session_interruption_policy_with_context,
};
use super::feedback::{decide_session_interruption_policy, display_for_aggregate};
use super::policy::{classify_command_intent, command_intent_key, CommandIntent};
use super::presentation::render_consultation_details;
use super::prompt::{
    finding_markdown_for_aggregate, format_runtime_hint, hook_analysis_user_input,
    prompt_hint_for_finding,
};
#[cfg(test)]
use super::queue::{queued_consultation_decision, topic_budget_key, QueuedConsultationDecision};
use super::queue::{render_next_queued_consultation, render_or_queue_consultation};
#[cfg(test)]
use super::slash::handle_command_hook_hint_action;

const MAX_HOOK_FINDINGS: usize = 32;
const MAX_HOOK_DISPLAY_EVENTS: usize = 128;
pub(crate) const INTERRUPTION_BUDGET_WINDOW_MS: u64 = 10 * 60 * 1000;
const PENDING_CONSULTATION_TTL_MS: u64 = INTERRUPTION_BUDGET_WINDOW_MS;
pub(crate) const SUCCESS_CONSULTATION_IDLE_GRACE: std::time::Duration =
    std::time::Duration::from_millis(250);

#[derive(Debug, Clone)]
pub(crate) struct AggregatedHookFinding {
    pub(crate) primary: HookFinding,
    pub(crate) related: Vec<HookFinding>,
    pub(crate) recommended_skill: Option<String>,
    pub(crate) topic: String,
    pub(crate) entity_key: String,
    pub(crate) effective_severity: FindingSeverity,
    pub(crate) confidence: String,
    pub(crate) suppression_key: String,
}

pub(crate) fn record_command_hook_findings(blocks: &[CommandBlock], state: &mut InlineState) {
    for block in blocks {
        if !state.hooks.handled_command_hooks.insert(block.id.clone()) {
            continue;
        }

        let findings = state
            .hooks
            .engine
            .evaluate_with_disabled(block, &state.hooks.disabled);
        for aggregate in aggregate_hook_findings(findings) {
            record_aggregated_hook_finding(block, aggregate, state);
        }
    }

    if state.hooks.findings.len() > MAX_HOOK_FINDINGS {
        let drop_count = state.hooks.findings.len() - MAX_HOOK_FINDINGS;
        state.hooks.findings.drain(0..drop_count);
    }
}

pub(crate) fn record_blocks_followed_by_user_input(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    state: &mut InlineState,
) {
    for block in blocks {
        let Some(end_index) = command_end_event_index(events, block) else {
            continue;
        };
        if events
            .iter()
            .skip(end_index + 1)
            .any(|event| is_followup_user_input_event(event, &block.id))
        {
            state
                .hooks
                .mark_block_followed_by_user_input(block.id.clone());
        }
    }
}

fn command_end_event_index(events: &[ShellEvent], block: &CommandBlock) -> Option<usize> {
    events.iter().position(|event| {
        matches!(
            event.kind,
            ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
        ) && event.command_id.as_deref() == Some(block.id.as_str())
    })
}

fn is_followup_user_input_event(event: &ShellEvent, block_id: &str) -> bool {
    match event.kind {
        ShellEventKind::CommandStarted => event.command_id.as_deref() != Some(block_id),
        ShellEventKind::UserInputIntercepted => event.component.is_none(),
        _ => false,
    }
}

pub(crate) fn render_recorded_hook_findings<W: Write>(
    blocks: &[CommandBlock],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let block_ids = blocks
        .iter()
        .map(|block| block.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let renderer = RatatuiInlineRenderer::for_terminal();

    let hints = state.hooks.findings.clone();
    for hint in hints {
        if !block_ids.contains(hint.command_block_id.as_str())
            || !state.hooks.rendered_findings.insert(hint.id.clone())
        {
            continue;
        }

        match hint.display {
            RuntimeHookDisplay::Silent => {}
            RuntimeHookDisplay::Hint => {
                if state.analysis_mode != AnalysisMode::Manual {
                    let Some(markdown) = hint.finding_markdown.as_deref() else {
                        continue;
                    };
                    let i18n = state.i18n();
                    let footer = i18n.format(
                        cosh_shell::MessageId::HookFindingFooter,
                        &[("hint_id", hint.id.as_str())],
                    );
                    renderer.write_notice_panel(
                        output,
                        NoticePanelModel {
                            title: i18n.t(cosh_shell::MessageId::HookFindingTitle),
                            body: renderer.markdown_text_lines(markdown),
                            footer: Some(&footer),
                        },
                    )?;
                    record_hook_display_event_for_hint(
                        &hint,
                        RuntimeHookDisplayAction::Shown,
                        state,
                    );
                }
            }
            RuntimeHookDisplay::Consultation => {
                if state.analysis_mode != AnalysisMode::Manual {
                    render_or_queue_consultation(&hint, state, output)?;
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn render_queued_hook_consultation<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    render_next_queued_consultation(state, output)
}

pub(crate) fn hook_routing_hints_for_block(
    state: &InlineState,
    block: &CommandBlock,
) -> Vec<String> {
    state
        .hooks
        .findings
        .iter()
        .filter(|hint| hint.command_block_id == block.id)
        .map(format_runtime_hint)
        .collect()
}

pub(crate) fn handle_consultation_events<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let consultation = match state.hooks.pending_consultation.take() {
        Some(c) => c,
        None => return Ok(()),
    };

    for event in events {
        if event.kind != ShellEventKind::UserInputIntercepted {
            continue;
        }
        if event.component.as_deref() != Some("card") {
            continue;
        }
        let event_id = event.input.as_deref().unwrap_or("");
        if !event_id.contains(&consultation.card_id) {
            continue;
        }
        let action = event.message.as_deref().unwrap_or("");
        if action == "approve" {
            let mut consultation = consultation;
            consultation.state = PendingConsultationState::Analyzed;
            let block = blocks.iter().find(|b| b.id == consultation.block_id);
            if let Some(block) = block {
                if consultation.hook_finding.is_some() {
                    record_hook_display_event_for_consultation(
                        &consultation,
                        RuntimeHookDisplayAction::Analyzed,
                        state,
                    );
                    start_agent_for_hook_consultation(
                        block,
                        blocks,
                        &consultation,
                        adapter,
                        state,
                        output,
                    )?;
                } else {
                    let findings = findings_from_blocks(blocks);
                    start_agent_for_block(
                        block,
                        blocks,
                        &findings,
                        adapter,
                        state,
                        output,
                        FailedCommandAgentStartOptions {
                            selectable_after_event_index: None,
                            trigger: FailedCommandAnalysisTrigger::UserConfirmed,
                        },
                    )?;
                }
            }
            render_next_queued_consultation(state, output)?;
            return Ok(());
        } else if action == "details" {
            render_consultation_details(&consultation, state, output)?;
            state.hooks.pending_consultation = Some(consultation);
            return Ok(());
        } else if action == "cancel" || action == "deny" {
            let mut consultation = consultation;
            consultation.state = PendingConsultationState::Ignored;
            state
                .hooks
                .ignored_cards
                .insert(consultation.suppression_key.clone());
            record_hook_display_event_for_consultation(
                &consultation,
                RuntimeHookDisplayAction::Ignored,
                state,
            );
            render_next_queued_consultation(state, output)?;
            return Ok(());
        }
    }

    state.hooks.pending_consultation = Some(consultation);
    Ok(())
}

fn record_aggregated_hook_finding(
    block: &CommandBlock,
    mut aggregate: AggregatedHookFinding,
    state: &mut InlineState,
) {
    attach_recent_memory_pressure(block, &mut aggregate, state);
    apply_memory_pressure_severity_upgrade(&mut aggregate);
    refresh_aggregate_metadata(block, &mut aggregate);
    let base_display = display_for_aggregate(block, &aggregate, state.analysis_mode);
    let decision = decide_session_interruption_policy(
        block,
        &aggregate,
        base_display,
        &aggregate.suppression_key,
        state,
    );
    let recommended_skill = aggregate.recommended_skill.clone();
    let prompt_hint = prompt_hint_for_finding(block, &aggregate, recommended_skill.as_deref());
    let finding_markdown = finding_markdown_for_aggregate(block, &aggregate, state.i18n());
    let hook_id = aggregate.primary.hook_id.clone();
    let related_hook_ids = aggregate
        .related
        .iter()
        .map(|finding| finding.hook_id.clone())
        .collect::<Vec<_>>();
    state.hooks.findings.push(RuntimeHookFinding {
        id: format!("hook-{}-{hook_id}", block.id),
        command_block_id: block.id.clone(),
        command: block.command.clone(),
        output_ref: block.output.terminal_output_ref.clone(),
        ended_at_ms: block.ended_at_ms,
        prompt_hint,
        finding_markdown: Some(finding_markdown),
        hook_finding: Some(combined_hook_finding(
            aggregate.primary.clone(),
            &aggregate.related,
        )),
        recommended_skill,
        display: decision.display,
        display_reason: decision.reason.to_string(),
        related_hook_ids,
        topic: aggregate.topic,
        entity_key: aggregate.entity_key,
        effective_severity: aggregate.effective_severity,
        confidence: aggregate.confidence,
        suppression_key: aggregate.suppression_key,
    });
    if decision.reason == "muted" {
        if let Some(hint) = state.hooks.findings.last().cloned() {
            record_hook_display_event_for_hint(&hint, RuntimeHookDisplayAction::Muted, state);
        }
    }
}

pub(crate) fn consultation_from_hint(hint: &RuntimeHookFinding) -> Option<PendingConsultation> {
    let finding = hint.hook_finding.clone()?;
    Some(PendingConsultation {
        finding_id: hint.id.clone(),
        card_id: format!("consultation-{}", hint.id),
        block_id: hint.command_block_id.clone(),
        command: hint.command.clone(),
        output_ref: hint.output_ref.clone(),
        state: PendingConsultationState::Queued,
        created_at_ms: hint.ended_at_ms,
        expires_at_ms: hint.ended_at_ms.saturating_add(PENDING_CONSULTATION_TTL_MS),
        ended_at_ms: hint.ended_at_ms,
        queued_at: std::time::Instant::now(),
        prompt_hint: hint.prompt_hint.clone(),
        hook_finding: Some(finding),
        recommended_skill: hint.recommended_skill.clone(),
        context_hints: vec![format_runtime_hint(hint)],
        suppression_key: hint.suppression_key.clone(),
        topic: hint.topic.clone(),
        entity_key: hint.entity_key.clone(),
        confidence: hint.confidence.clone(),
        display_reason: hint.display_reason.clone(),
    })
}

fn record_hook_display_event_for_hint(
    hint: &RuntimeHookFinding,
    action: RuntimeHookDisplayAction,
    state: &mut InlineState,
) {
    let hook_id = hint
        .hook_finding
        .as_ref()
        .map(|finding| finding.hook_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    record_hook_display_event(
        RuntimeHookDisplayEvent {
            action,
            finding_id: hint.id.clone(),
            command_block_id: hint.command_block_id.clone(),
            hook_id,
            topic: hint.topic.clone(),
            entity_key: hint.entity_key.clone(),
            suppression_key: hint.suppression_key.clone(),
            display: hint.display,
            display_reason: hint.display_reason.clone(),
            confidence: hint.confidence.clone(),
            ended_at_ms: hint.ended_at_ms,
        },
        state,
    );
}

pub(crate) fn record_hook_display_event_for_consultation(
    consultation: &PendingConsultation,
    action: RuntimeHookDisplayAction,
    state: &mut InlineState,
) {
    let hook_id = consultation
        .hook_finding
        .as_ref()
        .map(|finding| finding.hook_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    record_hook_display_event(
        RuntimeHookDisplayEvent {
            action,
            finding_id: consultation.finding_id.clone(),
            command_block_id: consultation.block_id.clone(),
            hook_id,
            topic: consultation.topic.clone(),
            entity_key: consultation.entity_key.clone(),
            suppression_key: consultation.suppression_key.clone(),
            display: RuntimeHookDisplay::Consultation,
            display_reason: consultation.display_reason.clone(),
            confidence: consultation.confidence.clone(),
            ended_at_ms: consultation.ended_at_ms,
        },
        state,
    );
}

fn record_hook_display_event(event: RuntimeHookDisplayEvent, state: &mut InlineState) {
    state.hooks.display_events.push(event);
    if state.hooks.display_events.len() > MAX_HOOK_DISPLAY_EVENTS {
        let drop_count = state.hooks.display_events.len() - MAX_HOOK_DISPLAY_EVENTS;
        state.hooks.display_events.drain(0..drop_count);
    }
}

pub(crate) fn start_agent_for_hook_consultation<W: Write>(
    block: &CommandBlock,
    blocks: &[CommandBlock],
    consultation: &PendingConsultation,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let findings = findings_from_blocks(blocks);
    let Some(mut request) =
        agent_request_after_confirmation(&block.session_id, block, &findings, true)
    else {
        return Ok(());
    };

    let ctx_config = cosh_shell::context_window::RelatedHistoryConfig {
        related_command_ids: related_command_ids_for_consultation(state, consultation),
        ..Default::default()
    };
    let ctx_entries =
        cosh_shell::context_window::build_related_history_index(blocks, block, &ctx_config);
    request.id = format!("agent-request-{}", consultation.card_id);
    request.context_blocks = cosh_shell::context_window::context_blocks_from_entries(&ctx_entries);
    request.context_hints = if consultation.context_hints.is_empty() {
        hook_routing_hints_for_block(state, block)
    } else {
        consultation.context_hints.clone()
    };
    request.user_input = Some(hook_analysis_user_input(block, consultation));
    request.mode = AgentMode::RecommendOnly;
    request.user_confirmed = true;
    request.hook_finding = consultation.hook_finding.clone();
    request.recommended_skill = consultation.recommended_skill.clone();
    state.agent_run.needs_prompt_after_run = true;
    start_agent_run(&request, adapter, state, output, None)
}

fn related_command_ids_for_consultation(
    state: &InlineState,
    consultation: &PendingConsultation,
) -> Vec<String> {
    let Some(hint) = state
        .hooks
        .findings
        .iter()
        .find(|hint| hint.id == consultation.finding_id)
    else {
        return Vec::new();
    };

    hint.related_hook_ids
        .iter()
        .filter_map(|related_hook_id| {
            state
                .hooks
                .findings
                .iter()
                .rev()
                .find(|candidate| {
                    candidate.command_block_id != consultation.block_id
                        && candidate.ended_at_ms <= consultation.ended_at_ms
                        && candidate
                            .hook_finding
                            .as_ref()
                            .map(|finding| finding.hook_id == *related_hook_id)
                            .unwrap_or(false)
                })
                .map(|candidate| candidate.command_block_id.clone())
        })
        .collect()
}

fn combined_hook_finding(mut primary: HookFinding, related: &[HookFinding]) -> HookFinding {
    if related.is_empty() {
        return primary;
    }
    let related_summary = related
        .iter()
        .map(|finding| {
            format!(
                "{} [{}]: {}",
                finding.hook_id,
                severity_label(finding.severity),
                finding.title
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    primary.description = format!(
        "{} Related findings: {related_summary}",
        primary.description
    );
    primary
}

pub(crate) fn recommended_skill_from_findings(
    primary: &HookFinding,
    related: &[HookFinding],
) -> Option<String> {
    primary
        .skill
        .clone()
        .or_else(|| related.iter().find_map(|finding| finding.skill.clone()))
}

pub(crate) fn is_muted_hook_target(aggregate: &AggregatedHookFinding, state: &InlineState) -> bool {
    let topic = finding_topic(aggregate);
    state.hooks.muted_targets.contains(topic)
        || state
            .hooks
            .muted_targets
            .contains(&aggregate.primary.hook_id)
        || aggregate
            .related
            .iter()
            .any(|finding| state.hooks.muted_targets.contains(&finding.hook_id))
}

pub(crate) fn has_memory_pressure_with_process(aggregate: &AggregatedHookFinding) -> bool {
    let has_pressure = aggregate.primary.hook_id == "memory-pressure"
        || aggregate
            .related
            .iter()
            .any(|finding| finding.hook_id == "memory-pressure");
    let has_process = aggregate.primary.hook_id == "high-memory-process"
        || aggregate
            .related
            .iter()
            .any(|finding| finding.hook_id == "high-memory-process");
    has_pressure && has_process
}

fn attach_recent_memory_pressure(
    block: &CommandBlock,
    aggregate: &mut AggregatedHookFinding,
    state: &InlineState,
) {
    if aggregate.primary.hook_id != "high-memory-process"
        || aggregate
            .related
            .iter()
            .any(|finding| finding.hook_id == "memory-pressure")
    {
        return;
    }
    let Some(pressure) = state.hooks.findings.iter().rev().find(|hint| {
        hint.command_block_id != block.id
            && hint.ended_at_ms <= block.ended_at_ms
            && block.ended_at_ms.saturating_sub(hint.ended_at_ms) <= INTERRUPTION_BUDGET_WINDOW_MS
            && hint.display != RuntimeHookDisplay::Silent
            && severity_rank(hint.effective_severity) >= severity_rank(FindingSeverity::Warning)
            && hint
                .hook_finding
                .as_ref()
                .map(|finding| finding.hook_id == "memory-pressure")
                .unwrap_or(false)
    }) else {
        return;
    };
    if let Some(finding) = pressure.hook_finding.clone() {
        aggregate.related.push(finding);
    }
}

fn apply_memory_pressure_severity_upgrade(aggregate: &mut AggregatedHookFinding) {
    if aggregate.primary.hook_id != "high-memory-process" {
        return;
    }
    let Some(pressure_severity) = aggregate
        .related
        .iter()
        .find(|finding| finding.hook_id == "memory-pressure")
        .map(|finding| finding.severity)
    else {
        return;
    };
    let Some(mem_pct) = process_mem_pct(&aggregate.primary.title) else {
        return;
    };

    let target_severity = if pressure_severity == FindingSeverity::Critical && mem_pct >= 35.0 {
        Some(FindingSeverity::Critical)
    } else if severity_rank(pressure_severity) >= severity_rank(FindingSeverity::Warning)
        && mem_pct >= 20.0
    {
        Some(FindingSeverity::Warning)
    } else {
        None
    };

    if let Some(severity) = target_severity {
        if severity_rank(severity) > severity_rank(aggregate.primary.severity) {
            aggregate.primary.severity = severity;
        }
    }
}

pub(crate) fn is_memory_hook(hook_id: &str) -> bool {
    hook_id == "memory-pressure"
        || hook_id == "high-memory-process"
        || hook_id == "interactive-top-guidance"
}

pub(crate) fn finding_topic(aggregate: &AggregatedHookFinding) -> &str {
    if !aggregate.topic.is_empty() {
        return &aggregate.topic;
    }
    finding_topic_from_findings(&aggregate.primary, &aggregate.related)
}

pub(crate) fn finding_topic_from_findings(
    primary: &HookFinding,
    related: &[HookFinding],
) -> &'static str {
    if is_memory_hook(&primary.hook_id)
        || related
            .iter()
            .any(|finding| is_memory_hook(&finding.hook_id))
    {
        "memory"
    } else if primary.hook_id == "test-failure" {
        "test"
    } else if primary.hook_id == "failed-command" {
        "command-failure"
    } else {
        "external"
    }
}

pub(crate) fn finding_confidence<'a>(
    block: &CommandBlock,
    aggregate: &'a AggregatedHookFinding,
) -> &'a str {
    if !aggregate.confidence.is_empty() {
        return &aggregate.confidence;
    }
    computed_finding_confidence(block, aggregate)
}

pub(crate) fn computed_finding_confidence(
    block: &CommandBlock,
    aggregate: &AggregatedHookFinding,
) -> &'static str {
    if aggregate
        .primary
        .description
        .contains("Confidence is lower")
        || is_low_confidence_command_intent(&block.command)
    {
        "low"
    } else if aggregate.related.is_empty() {
        "medium"
    } else {
        "high"
    }
}

fn is_low_confidence_command_intent(command: &str) -> bool {
    matches!(
        classify_command_intent(command),
        CommandIntent::Lookup
            | CommandIntent::Pipeline
            | CommandIntent::Script
            | CommandIntent::Wrapper
            | CommandIntent::Interactive
    )
}

#[cfg(test)]
fn suppression_key(block: &CommandBlock, aggregate: &AggregatedHookFinding) -> String {
    if !aggregate.suppression_key.is_empty() {
        return aggregate.suppression_key.clone();
    }
    computed_suppression_key(block, aggregate)
}

pub(crate) fn computed_suppression_key(
    block: &CommandBlock,
    aggregate: &AggregatedHookFinding,
) -> String {
    if aggregate.primary.hook_id == "high-memory-process" {
        return format!(
            "{}:{}:{}:{}",
            finding_topic(aggregate),
            aggregate.primary.hook_id,
            entity_key(block, aggregate),
            command_intent_key(&block.command)
        );
    }
    format!(
        "{}:{}:{}",
        finding_topic(aggregate),
        aggregate.primary.hook_id,
        command_intent_key(&block.command)
    )
}

pub(crate) fn entity_key(block: &CommandBlock, aggregate: &AggregatedHookFinding) -> String {
    if !aggregate.entity_key.is_empty() {
        return aggregate.entity_key.clone();
    }
    computed_entity_key(block, aggregate)
}

pub(crate) fn computed_entity_key(
    block: &CommandBlock,
    aggregate: &AggregatedHookFinding,
) -> String {
    match aggregate.primary.hook_id.as_str() {
        "memory-pressure" => "system-memory".to_string(),
        "high-memory-process" => process_entity_key(&aggregate.primary.title),
        _ => command_intent_key(&block.command).to_string(),
    }
}

fn process_entity_key(title: &str) -> String {
    if let Some(pid) = extract_pid_from_process_title(title) {
        return format!("process:pid:{pid}");
    }
    format!("process:title:{}", title.trim())
}

fn process_mem_pct(title: &str) -> Option<f64> {
    let before_marker = title.rsplit_once("% MEM")?.0;
    let pct = before_marker.split_whitespace().last()?;
    pct.parse().ok()
}

fn extract_pid_from_process_title(title: &str) -> Option<&str> {
    let marker = "(PID ";
    let start = title.find(marker)? + marker.len();
    let rest = &title[start..];
    let end = rest.find(')')?;
    let pid = rest[..end].trim();
    if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) {
        Some(pid)
    } else {
        None
    }
}

pub(crate) fn memory_hook_preference(hook_id: &str) -> u8 {
    match hook_id {
        "memory-pressure" => 1,
        _ => 0,
    }
}

pub(crate) fn severity_rank(severity: FindingSeverity) -> u8 {
    match severity {
        FindingSeverity::Info => 0,
        FindingSeverity::Warning => 1,
        FindingSeverity::Critical => 2,
    }
}

pub(crate) fn severity_label(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::Info => "info",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Critical => "critical",
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
