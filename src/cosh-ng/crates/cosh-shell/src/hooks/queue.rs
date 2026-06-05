use std::io::Write;

use cosh_shell::hook_types::{FindingSeverity, HookFinding};
use cosh_shell::types::{CommandBlock, CommandStatus, OutputRefs};

use super::feedback::decide_session_interruption_policy;
use super::presentation::render_consultation_card;
use super::runtime::{
    consultation_from_hint, entity_key, finding_topic, record_hook_display_event_for_consultation,
    severity_rank, AggregatedHookFinding, INTERRUPTION_BUDGET_WINDOW_MS,
    SUCCESS_CONSULTATION_IDLE_GRACE,
};
use crate::runtime::state::{
    HookSuppressionRecord, InlineState, InterruptionBudgetRecord, PendingConsultation,
    PendingConsultationState, RuntimeHookDisplay, RuntimeHookDisplayAction, RuntimeHookFinding,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueuedConsultationDecision {
    Render,
    KeepQueued,
    Drop,
}

pub(crate) fn interruption_budget_exhausted(
    block: &CommandBlock,
    aggregate: &AggregatedHookFinding,
    state: &InlineState,
) -> bool {
    let topic = finding_topic(aggregate);
    let entity = entity_key(block, aggregate);
    let severity = aggregate.primary.severity;
    [topic_budget_key(topic), entity_budget_key(topic, &entity)]
        .iter()
        .filter_map(|key| state.hooks.interruption_budget.get(key))
        .any(|record| suppress_by_budget(record, block.ended_at_ms, severity))
}

pub(crate) fn record_interruption_budget(
    consultation: &PendingConsultation,
    state: &mut InlineState,
) {
    let Some(finding) = consultation.hook_finding.as_ref() else {
        return;
    };
    let record = InterruptionBudgetRecord {
        last_rendered_at_ms: consultation.ended_at_ms,
        severity: finding.severity,
    };
    state
        .hooks
        .interruption_budget
        .insert(topic_budget_key(&consultation.topic), record);
    state.hooks.interruption_budget.insert(
        entity_budget_key(&consultation.topic, &consultation.entity_key),
        record,
    );
}

fn suppress_by_budget(
    record: &InterruptionBudgetRecord,
    ended_at_ms: u64,
    severity: FindingSeverity,
) -> bool {
    ended_at_ms.saturating_sub(record.last_rendered_at_ms) < INTERRUPTION_BUDGET_WINDOW_MS
        && severity_rank(record.severity) >= severity_rank(severity)
}

pub(crate) fn topic_budget_key(topic: &str) -> String {
    format!("topic:{topic}")
}

fn entity_budget_key(topic: &str, entity_key: &str) -> String {
    format!("entity:{topic}:{entity_key}")
}

pub(crate) fn render_or_queue_consultation<W: Write>(
    hint: &RuntimeHookFinding,
    state: &mut InlineState,
    _output: &mut W,
) -> std::io::Result<()> {
    let Some(mut consultation) = consultation_from_hint(hint) else {
        return Ok(());
    };
    if state.agent_run.active.is_some() {
        consultation.state = PendingConsultationState::Deferred;
        consultation.display_reason = "active-agent-run-deferred".to_string();
        record_hook_display_event_for_consultation(
            &consultation,
            RuntimeHookDisplayAction::Deferred,
            state,
        );
        state
            .hooks
            .pending_consultation_queue
            .push_back(consultation);
        return Ok(());
    }
    if state.hooks.pending_consultation.is_some() {
        consultation.state = PendingConsultationState::Queued;
        state
            .hooks
            .pending_consultation_queue
            .push_back(consultation);
        return Ok(());
    }
    consultation.state = PendingConsultationState::Queued;
    state
        .hooks
        .pending_consultation_queue
        .push_back(consultation);
    Ok(())
}

pub(crate) fn render_next_queued_consultation<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.agent_run.active.is_some() || state.hooks.pending_consultation.is_some() {
        return Ok(());
    }
    let Some(mut consultation) = next_renderable_queued_consultation(state) else {
        return Ok(());
    };
    consultation.state = PendingConsultationState::Displayed;
    mark_consultation_rendered(&consultation, state);
    render_consultation_card(&consultation, state.language, output)?;
    state.hooks.pending_consultation = Some(consultation);
    Ok(())
}

fn next_renderable_queued_consultation(state: &mut InlineState) -> Option<PendingConsultation> {
    let now_ms = pending_consultation_now_ms(state);
    while let Some(mut consultation) = state.hooks.pending_consultation_queue.pop_front() {
        match queued_consultation_decision(&mut consultation, state, now_ms) {
            QueuedConsultationDecision::Render => return Some(consultation),
            QueuedConsultationDecision::KeepQueued => {
                state
                    .hooks
                    .pending_consultation_queue
                    .push_front(consultation);
                return None;
            }
            QueuedConsultationDecision::Drop => {}
        }
    }
    None
}

pub(crate) fn queued_consultation_decision(
    consultation: &mut PendingConsultation,
    state: &mut InlineState,
    now_ms: u64,
) -> QueuedConsultationDecision {
    if now_ms > consultation.expires_at_ms {
        consultation.state = PendingConsultationState::Expired;
        record_hook_display_event_for_consultation(
            consultation,
            RuntimeHookDisplayAction::Expired,
            state,
        );
        return QueuedConsultationDecision::Drop;
    }
    if consultation.queued_at.elapsed() < SUCCESS_CONSULTATION_IDLE_GRACE {
        return QueuedConsultationDecision::KeepQueued;
    }
    let Some(finding) = consultation.hook_finding.as_ref() else {
        consultation.state = PendingConsultationState::Expired;
        record_hook_display_event_for_consultation(
            consultation,
            RuntimeHookDisplayAction::Expired,
            state,
        );
        return QueuedConsultationDecision::Drop;
    };

    let block = block_for_pending_consultation(consultation);
    let aggregate = aggregate_for_pending_consultation(consultation, finding.clone());
    let decision = decide_session_interruption_policy(
        &block,
        &aggregate,
        RuntimeHookDisplay::Consultation,
        &consultation.suppression_key,
        state,
    );
    if decision.display != RuntimeHookDisplay::Consultation {
        consultation.display_reason = decision.reason.to_string();
        consultation.state = if decision.reason == "ignored-same-finding" {
            PendingConsultationState::Ignored
        } else {
            PendingConsultationState::Deferred
        };
        let action = if consultation.state == PendingConsultationState::Ignored {
            RuntimeHookDisplayAction::Ignored
        } else {
            RuntimeHookDisplayAction::Deferred
        };
        record_hook_display_event_for_consultation(consultation, action, state);
        return QueuedConsultationDecision::Drop;
    }
    consultation.display_reason = decision.reason.to_string();
    QueuedConsultationDecision::Render
}

fn block_for_pending_consultation(consultation: &PendingConsultation) -> CommandBlock {
    CommandBlock {
        id: consultation.block_id.clone(),
        session_id: "session".to_string(),
        command: consultation.command.clone(),
        cwd: String::new(),
        end_cwd: String::new(),
        started_at_ms: consultation.created_at_ms,
        ended_at_ms: consultation.ended_at_ms,
        duration_ms: consultation
            .ended_at_ms
            .saturating_sub(consultation.created_at_ms),
        exit_code: 0,
        status: CommandStatus::Completed,
        output: OutputRefs {
            terminal_output_ref: consultation.output_ref.clone(),
            terminal_output_bytes: 0,
        },
    }
}

fn aggregate_for_pending_consultation(
    consultation: &PendingConsultation,
    finding: HookFinding,
) -> AggregatedHookFinding {
    let effective_severity = finding.severity;
    AggregatedHookFinding {
        primary: finding,
        related: Vec::new(),
        recommended_skill: consultation.recommended_skill.clone(),
        topic: consultation.topic.clone(),
        entity_key: consultation.entity_key.clone(),
        effective_severity,
        confidence: consultation.confidence.clone(),
        suppression_key: consultation.suppression_key.clone(),
    }
}

fn pending_consultation_now_ms(state: &InlineState) -> u64 {
    state
        .hooks
        .findings
        .iter()
        .map(|hint| hint.ended_at_ms)
        .chain(
            state
                .hooks
                .pending_consultation
                .iter()
                .map(|consultation| consultation.ended_at_ms),
        )
        .chain(
            state
                .hooks
                .pending_consultation_queue
                .iter()
                .map(|consultation| consultation.ended_at_ms),
        )
        .max()
        .unwrap_or(0)
}

fn mark_consultation_rendered(consultation: &PendingConsultation, state: &mut InlineState) {
    let Some(finding) = consultation.hook_finding.as_ref() else {
        return;
    };
    state.hooks.rendered_cards.insert(
        consultation.suppression_key.clone(),
        HookSuppressionRecord {
            severity: finding.severity,
        },
    );
    record_interruption_budget(consultation, state);
    record_hook_display_event_for_consultation(
        consultation,
        RuntimeHookDisplayAction::Shown,
        state,
    );
}
