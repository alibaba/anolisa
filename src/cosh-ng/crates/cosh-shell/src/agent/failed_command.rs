use crate::hooks::interrupt::command_should_skip_failure_analysis;
use crate::runtime::prelude::*;

use crate::command::{classify_failure, FailureClass, FailureConfidence, FailureReason};
use crate::evidence::model::{EvidenceExcerpt, OutputExcerptDirection};
use crate::evidence::output_policy::{
    bounded_output_excerpt_for_block, bounded_output_head_tail_excerpt_for_block,
};
use crate::insight::evidence::{
    build_provider_evidence_payload, provider_target_facts, take_bound_insight_metadata,
    trim_optional_context_hints, EvidenceBundleInput,
};
use crate::insight::failed_command::{
    decide_failure_intervention, map_failure_semantics, FailureInsightKind,
};
use crate::insight::model::{
    InsightBinding, InsightCandidate, InsightConfidence, InsightEvidence, InsightSeverity,
    InsightSource, InsightTarget, InterventionDecision, OutputExcerptStatus, PromptSuggestion,
    SuppressionTopic,
};
use crate::insight::policy::{failure_suppression_key, AnalysisPolicyMode, InterventionGates};
use crate::insight::scope::resolve_execution_scope;
use crate::runtime::controller::{pending_card_capture, shell_has_active_foreground_command};

const FAILURE_OUTPUT_EXCERPT_MAX_BYTES: usize = 8192;
const FAILURE_OUTPUT_EXCERPT_MAX_LINES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailedCommandAnalysisTrigger {
    Auto,
    UserConfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureAnalysisDisposition {
    SilentRecord,
    ActionCard,
    AutoAnalyze,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailedCommandAgentStartOptions {
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) trigger: FailedCommandAnalysisTrigger,
}

pub(crate) fn collect_failed_command_insights<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    state: &mut InlineState,
    _output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for block in blocks {
        if state.analyzed_blocks.contains(&block.id) || state.canceled_blocks.contains(&block.id) {
            continue;
        }
        if state.evaluated_failed_command_insights.contains(&block.id) {
            continue;
        }
        let end_event_index = block_end_event_index(events, block);
        if end_event_index.is_none_or(|idx| idx < event_index_base) {
            continue;
        }
        state
            .evaluated_failed_command_insights
            .insert(block.id.clone());

        let excerpt = failure_output_evidence(block);
        let semantics = classify_failure(block, events, excerpt.text.as_deref());
        let command_not_found = semantics.class == FailureClass::CommandNotFound;
        let rewrite = if state.analysis_mode != AnalysisMode::Manual && command_not_found {
            let diagnostic_tail = command_not_found_diagnostic_tail(block);
            state
                .shell_rewrite
                .resolve_for_block(block, diagnostic_tail.as_deref())
        } else {
            None
        };
        let candidate = rewrite
            .map(|text| shell_rewrite_candidate(block, text))
            .or_else(|| failed_command_candidate(events, block));
        let Some(candidate) = candidate else {
            continue;
        };
        let user_has_not_continued = !state.hooks.block_followed_by_user_input(&block.id);
        let gates = InterventionGates {
            same_dispatch_batch: end_event_index.is_some_and(|idx| idx >= event_index_base),
            input_empty: user_has_not_continued,
            foreground_idle: !shell_has_active_foreground_command(events),
            active_runtime_idle: state.agent_run.active.is_none()
                && pending_card_capture(state).is_none(),
            user_has_not_continued,
            user_interactive_origin: block.origin == CommandOrigin::UserInteractive,
            budget_available: !state.insight_budget.is_suppressed(
                &candidate.suppression_key,
                candidate.severity,
                block.ended_at_ms,
            ),
        };
        let Some(kind) = map_failure_semantics(&semantics) else {
            continue;
        };
        if !matches!(
            decide_failure_intervention(
                kind,
                semantics.confidence,
                semantics.auto_eligibility,
                failure_output_status(block, &excerpt).is_usable(excerpt.text.as_deref()),
                &candidate,
                analysis_policy_mode(state.analysis_mode),
                gates,
            ),
            InterventionDecision::Suggest { .. }
        ) {
            continue;
        }
        let replace = state
            .pending_command_insight
            .as_ref()
            .is_none_or(|pending| candidate.severity > pending.severity);
        if replace {
            state.pending_command_insight = Some(candidate);
        }
    }

    Ok(())
}

fn analysis_policy_mode(mode: AnalysisMode) -> AnalysisPolicyMode {
    match mode {
        AnalysisMode::Smart => AnalysisPolicyMode::Smart,
        AnalysisMode::Auto => AnalysisPolicyMode::Auto,
        AnalysisMode::Manual => AnalysisPolicyMode::Manual,
    }
}

pub(crate) fn failed_command_intervention(
    events: &[ShellEvent],
    block: &CommandBlock,
    candidate: &InsightCandidate,
    mode: AnalysisMode,
    gates: InterventionGates,
) -> InterventionDecision {
    let excerpt = failure_output_evidence(block);
    let semantics = classify_failure(block, events, excerpt.text.as_deref());
    let Some(kind) = map_failure_semantics(&semantics) else {
        return InterventionDecision::Silent;
    };
    decide_failure_intervention(
        kind,
        semantics.confidence,
        semantics.auto_eligibility,
        failure_output_status(block, &excerpt).is_usable(excerpt.text.as_deref()),
        candidate,
        analysis_policy_mode(mode),
        gates,
    )
}

fn shell_rewrite_candidate(block: &CommandBlock, text: String) -> InsightCandidate {
    let scope = resolve_execution_scope(&block.session_id, &block.command);
    let suppression_key = failure_suppression_key(
        SuppressionTopic::CommandNotFound,
        &block.command,
        scope.clone(),
    );
    InsightCandidate {
        source: InsightSource::FailedCommand,
        topic: SuppressionTopic::CommandNotFound,
        entity: suppression_key.entity.clone(),
        severity: InsightSeverity::Warning,
        confidence: InsightConfidence::High,
        evidence: Vec::new(),
        suggestion: Some(PromptSuggestion::ShellRewrite { text }),
        scope,
        suppression_key,
    }
}

pub(crate) fn failed_command_candidate(
    events: &[ShellEvent],
    block: &CommandBlock,
) -> Option<InsightCandidate> {
    let excerpt = failure_output_evidence(block);
    let semantics = classify_failure(block, events, excerpt.text.as_deref());
    let kind = map_failure_semantics(&semantics)?;
    if kind == FailureInsightKind::CommandNotFound {
        return None;
    }
    let scope = resolve_execution_scope(&block.session_id, &block.command);
    let topic = match kind {
        FailureInsightKind::PermissionDenied => SuppressionTopic::PermissionDenied,
        FailureInsightKind::BuildOrTestFailure => SuppressionTopic::BuildOrTestFailure,
        FailureInsightKind::RuntimeException => SuppressionTopic::RuntimeException,
        FailureInsightKind::AbnormalSignal => SuppressionTopic::AbnormalSignal,
        FailureInsightKind::CommandNotFound => unreachable!(),
    };
    let evidence_status = failure_output_status(block, &excerpt);
    let severity = if kind == FailureInsightKind::AbnormalSignal {
        InsightSeverity::Critical
    } else {
        InsightSeverity::Warning
    };
    let confidence = match semantics.confidence {
        FailureConfidence::High => InsightConfidence::High,
        FailureConfidence::Medium => InsightConfidence::Medium,
        FailureConfidence::Low => InsightConfidence::Low,
    };
    let mut evidence = vec![
        InsightEvidence {
            key: "failure_class".to_string(),
            value: format!("{:?}", semantics.class),
        },
        InsightEvidence {
            key: "failure_auto_eligibility".to_string(),
            value: format!("{:?}", semantics.auto_eligibility),
        },
    ];
    evidence.extend(
        semantics
            .reasons
            .iter()
            .filter(|reason| {
                matches!(
                    reason,
                    FailureReason::ExitCode(_)
                        | FailureReason::CommandFamily(_)
                        | FailureReason::TerminalSignature(_)
                        | FailureReason::ExcerptDirection(_)
                )
            })
            .enumerate()
            .map(|(index, reason)| InsightEvidence {
                key: format!("failure_reason_{index}"),
                value: format!("{reason:?}"),
            }),
    );
    let target = InsightTarget {
        insight_id: format!("failure-{}", block.id),
        source_session_id: block.session_id.clone(),
        source_command_block_id: block.id.clone(),
        scope: scope.clone(),
        evidence_handle: Some(crate::evidence::terminal_output_id(
            &block.session_id,
            &block.id,
        )),
        evidence_status,
        severity,
        confidence,
        evidence: evidence.clone(),
        created_at_ms: block.ended_at_ms,
    };
    let suppression_key = failure_suppression_key(topic.clone(), &block.command, scope.clone());
    Some(InsightCandidate {
        source: InsightSource::FailedCommand,
        topic,
        entity: suppression_key.entity.clone(),
        severity,
        confidence,
        evidence,
        suggestion: Some(PromptSuggestion::AgentPrompt {
            binding: Box::new(InsightBinding {
                suggestion_id: format!("failure-suggestion-{}", block.id),
                target,
            }),
        }),
        scope,
        suppression_key,
    })
}

fn failure_output_status(_block: &CommandBlock, excerpt: &EvidenceExcerpt) -> OutputExcerptStatus {
    match excerpt.capture_status {
        crate::evidence::EvidenceCaptureStatus::Expired => OutputExcerptStatus::Expired,
        crate::evidence::EvidenceCaptureStatus::Unavailable => OutputExcerptStatus::Unavailable,
        crate::evidence::EvidenceCaptureStatus::ReadFailed => OutputExcerptStatus::ReadFailed,
        crate::evidence::EvidenceCaptureStatus::Truncated => OutputExcerptStatus::Truncated,
        crate::evidence::EvidenceCaptureStatus::Available if excerpt.text.is_none() => {
            OutputExcerptStatus::ReadFailed
        }
        _ if excerpt
            .text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty()) =>
        {
            OutputExcerptStatus::Empty
        }
        _ if excerpt.truncated => OutputExcerptStatus::Truncated,
        _ => OutputExcerptStatus::Available,
    }
}

pub(crate) fn render_post_failure_actions<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    findings: &[Finding],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let key = format!("cancel-{event_index}");
        if event_cancels_failed_command_analysis(event)
            && !state.handled_cancellations.contains(&key)
        {
            let Some(block) = pending_failed_block_for_event(blocks, state, event) else {
                continue;
            };

            state.handled_cancellations.insert(key);
            state.canceled_blocks.insert(block.id.clone());
            RatatuiInlineRenderer::for_terminal().write_notice_panel(
                output,
                NoticePanelModel {
                    title: state.i18n().t(MessageId::FailedAnalysisCancelledTitle),
                    body: vec![state.i18n().format(
                        MessageId::FailedAnalysisCancelledBody,
                        &[("command", block.command.as_str())],
                    )],
                    footer: Some(state.i18n().t(MessageId::FailedAnalysisCancelledFooter)),
                },
            )?;
            output.flush()?;
            continue;
        }

        let key = format!("confirm-{event_index}");
        if !event_confirms_failed_command_analysis(event)
            || state.handled_confirmations.contains(&key)
        {
            continue;
        }

        let Some(block) = pending_failed_block_for_event(blocks, state, event) else {
            continue;
        };

        state.handled_confirmations.insert(key);
        start_agent_for_block(
            block,
            blocks,
            findings,
            adapter,
            state,
            output,
            FailedCommandAgentStartOptions {
                selectable_after_event_index: Some(event_index),
                trigger: FailedCommandAnalysisTrigger::UserConfirmed,
            },
        )?;
        output.flush()?;
    }

    Ok(())
}

pub(crate) fn latest_pending_failed_block_before_event<'a>(
    blocks: &'a [CommandBlock],
    state: &InlineState,
    event: &ShellEvent,
) -> Option<&'a CommandBlock> {
    blocks.iter().rev().find(|block| {
        can_user_confirm_failure_analysis(block)
            && !state.analyzed_blocks.contains(&block.id)
            && !state.canceled_blocks.contains(&block.id)
            && event_happened_after_block_end(event, block)
    })
}

fn pending_failed_block_for_event<'a>(
    blocks: &'a [CommandBlock],
    state: &InlineState,
    event: &ShellEvent,
) -> Option<&'a CommandBlock> {
    latest_pending_failed_block_before_event(blocks, state, event)
}

#[allow(dead_code)]
pub(crate) fn should_analyze_failed_block(block: &CommandBlock, mode: AnalysisMode) -> bool {
    should_auto_analyze_failed_block(&[], block, mode)
}

pub(crate) fn should_auto_analyze_failed_block(
    events: &[ShellEvent],
    block: &CommandBlock,
    mode: AnalysisMode,
) -> bool {
    failure_analysis_disposition_for_block(events, block, mode)
        == FailureAnalysisDisposition::AutoAnalyze
}

pub(crate) fn failure_analysis_disposition_for_block(
    events: &[ShellEvent],
    block: &CommandBlock,
    mode: AnalysisMode,
) -> FailureAnalysisDisposition {
    let excerpt = failure_output_excerpt(block);
    failure_analysis_disposition(events, block, mode, excerpt.as_deref())
}

fn failure_analysis_disposition(
    events: &[ShellEvent],
    block: &CommandBlock,
    mode: AnalysisMode,
    output_excerpt: Option<&str>,
) -> FailureAnalysisDisposition {
    if block.command.trim().is_empty() || command_should_skip_failure_analysis(events, block) {
        return FailureAnalysisDisposition::SilentRecord;
    }

    let semantics = classify_failure(block, events, output_excerpt);
    match semantics.class {
        FailureClass::Success
        | FailureClass::ExpectedNoResult
        | FailureClass::UsageOrHelp
        | FailureClass::InteractiveCancel
        | FailureClass::UserInterrupt
        | FailureClass::PipelineNormal
        | FailureClass::CommandNotFound
        | FailureClass::GenericRuntimeFailure
        | FailureClass::ProviderOrInternalArtifact
        | FailureClass::UnknownFailure => FailureAnalysisDisposition::SilentRecord,
        FailureClass::PermissionDenied if semantics.confidence == FailureConfidence::High => {
            match mode {
                AnalysisMode::Auto
                    if semantics.auto_eligibility
                        == crate::command::FailureAutoEligibility::LegacyAllowlisted =>
                {
                    FailureAnalysisDisposition::AutoAnalyze
                }
                AnalysisMode::Auto => FailureAnalysisDisposition::ActionCard,
                AnalysisMode::Smart => FailureAnalysisDisposition::ActionCard,
                AnalysisMode::Manual => FailureAnalysisDisposition::SilentRecord,
            }
        }
        FailureClass::AbnormalSignal
        | FailureClass::BuildOrTestFailure
        | FailureClass::RuntimeException
            if semantics.confidence == FailureConfidence::High =>
        {
            match mode {
                AnalysisMode::Auto
                    if semantics.auto_eligibility
                        == crate::command::FailureAutoEligibility::LegacyAllowlisted
                        && output_excerpt.is_some_and(usable_failure_excerpt) =>
                {
                    FailureAnalysisDisposition::AutoAnalyze
                }
                AnalysisMode::Auto | AnalysisMode::Smart => FailureAnalysisDisposition::ActionCard,
                AnalysisMode::Manual => FailureAnalysisDisposition::SilentRecord,
            }
        }
        _ => FailureAnalysisDisposition::SilentRecord,
    }
}

fn usable_failure_excerpt(excerpt: &str) -> bool {
    let text = excerpt.trim();
    !text.is_empty() && text != "... <truncated>"
}

fn can_user_confirm_failure_analysis(block: &CommandBlock) -> bool {
    block.exit_code != 0
        && !block.command.trim().is_empty()
        && !matches!(
            block.origin,
            CommandOrigin::ProviderTool | CommandOrigin::ShellInternal
        )
}

fn failure_output_excerpt(block: &CommandBlock) -> Option<String> {
    failure_output_evidence(block).text
}

fn failure_output_evidence(block: &CommandBlock) -> EvidenceExcerpt {
    bounded_output_head_tail_excerpt_for_block(
        block,
        FAILURE_OUTPUT_EXCERPT_MAX_LINES,
        FAILURE_OUTPUT_EXCERPT_MAX_BYTES,
    )
}

fn command_not_found_diagnostic_tail(block: &CommandBlock) -> Option<String> {
    bounded_output_excerpt_for_block(
        block,
        OutputExcerptDirection::Tail,
        FAILURE_OUTPUT_EXCERPT_MAX_LINES,
        FAILURE_OUTPUT_EXCERPT_MAX_BYTES,
    )
    .text
}

fn event_happened_after_block_end(event: &ShellEvent, block: &CommandBlock) -> bool {
    event
        .started_at_ms
        .map(|timestamp| timestamp >= block.ended_at_ms)
        .unwrap_or(true)
}

pub(crate) fn block_end_event_index(events: &[ShellEvent], block: &CommandBlock) -> Option<usize> {
    events.iter().enumerate().find_map(|(idx, event)| {
        if event.command_id.as_deref() == Some(block.id.as_str())
            && matches!(
                event.kind,
                ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
            )
        {
            Some(idx)
        } else {
            None
        }
    })
}

pub(crate) fn start_agent_for_block<W: Write>(
    block: &CommandBlock,
    blocks: &[CommandBlock],
    findings: &[Finding],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    options: FailedCommandAgentStartOptions,
) -> std::io::Result<()> {
    let should_start = match options.trigger {
        FailedCommandAnalysisTrigger::Auto => true,
        FailedCommandAnalysisTrigger::UserConfirmed => can_user_confirm_failure_analysis(block),
    };
    if !should_start {
        return Ok(());
    }

    if state.canceled_blocks.contains(&block.id) {
        return Ok(());
    }

    if !state.analyzed_blocks.insert(block.id.clone()) {
        return Ok(());
    }

    if state.analysis_throttle.should_throttle(&block.command) {
        let throttle_key = format!("throttle:{}", first_program_token(&block.command));
        if state.queued_analysis_notices.insert(throttle_key) {
            RatatuiInlineRenderer::for_terminal().write_notice_panel(
                output,
                NoticePanelModel {
                    title: state.i18n().t(MessageId::AnalysisSkippedTitle),
                    body: vec![state.i18n().format(
                        MessageId::AnalysisSkippedBody,
                        &[("command", block.command.as_str())],
                    )],
                    footer: Some(state.i18n().t(MessageId::AnalysisSkippedFooter)),
                },
            )?;
            output.flush()?;
        }
        return Ok(());
    }

    let request = match options.trigger {
        FailedCommandAnalysisTrigger::Auto => Some(agent_request_for_auto_failure(
            &block.session_id,
            block,
            findings,
        )),
        FailedCommandAnalysisTrigger::UserConfirmed => {
            agent_request_after_confirmation(&block.session_id, block, findings, true)
        }
    };
    match request {
        Some(mut request) => {
            let ctx_config = RelatedHistoryConfig::default();
            let ctx_entries = build_related_history_index(blocks, block, &ctx_config);
            let target_scope = resolve_execution_scope(&block.session_id, &block.command);
            request.context_blocks = if target_scope.allows_correlation() {
                context_blocks_from_entries(&ctx_entries)
                    .into_iter()
                    .filter(|related| {
                        resolve_execution_scope(&related.session_id, &related.command)
                            == target_scope
                    })
                    .collect()
            } else {
                Vec::new()
            };
            request
                .context_hints
                .extend(hook_routing_hints_for_block(state, block));
            attach_failure_evidence_bundle(&mut request);
            if options.trigger == FailedCommandAnalysisTrigger::Auto
                && !request.context_hints.is_empty()
                && state.agent_run.active.is_none()
            {
                writeln!(
                    output,
                    "{} {}",
                    state.i18n().format(
                        MessageId::HookAutoAnalyzedBody,
                        &[
                            ("command", block.command.as_str()),
                            ("exit_code", &block.exit_code.to_string()),
                        ],
                    ),
                    state.i18n().t(MessageId::HookAutoAnalyzedFooter),
                )?;
            }
            if state.agent_run.active.is_some()
                && state.queued_analysis_notices.insert(block.id.clone())
            {
                RatatuiInlineRenderer::for_terminal().write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: state.i18n().t(MessageId::AgentQueuedTitle),
                        body: vec![
                            state.i18n().format(
                                MessageId::AgentQueuedBodyCommand,
                                &[("command", block.command.as_str())],
                            ),
                            state.i18n().t(MessageId::AgentQueuedBodyActive).to_string(),
                        ],
                        footer: Some(state.i18n().t(MessageId::AgentQueuedFooter)),
                    },
                )?;
            }
            state.agent_run.needs_prompt_after_run = true;
            start_agent_run_with_origin(
                &request,
                match options.trigger {
                    FailedCommandAnalysisTrigger::Auto => AgentRunOrigin::AutoFailure,
                    FailedCommandAnalysisTrigger::UserConfirmed => AgentRunOrigin::Standard,
                },
                adapter,
                state,
                output,
                options.selectable_after_event_index,
            )
        }
        None => Ok(()),
    }
}

pub(crate) fn attach_failure_evidence_bundle(request: &mut AgentRequest) {
    let classifier_excerpt = bounded_output_head_tail_excerpt_for_block(
        &request.command_block,
        FAILURE_OUTPUT_EXCERPT_MAX_LINES,
        FAILURE_OUTPUT_EXCERPT_MAX_BYTES,
    );
    let semantics = classify_failure(
        &request.command_block,
        &[],
        classifier_excerpt.text.as_deref(),
    );
    let target_excerpt = match semantics.class {
        FailureClass::BuildOrTestFailure | FailureClass::RuntimeException => {
            bounded_output_head_tail_excerpt_for_block(&request.command_block, 120, 12 * 1024)
        }
        FailureClass::PermissionDenied | FailureClass::AbnormalSignal => {
            bounded_output_excerpt_for_block(
                &request.command_block,
                OutputExcerptDirection::Tail,
                120,
                12 * 1024,
            )
        }
        _ => classifier_excerpt,
    };
    let related_facts = request
        .context_blocks
        .iter()
        .map(crate::evidence::provider_safe_command_fact_line)
        .collect::<Vec<_>>();
    let severity = if semantics.class == FailureClass::AbnormalSignal {
        "Critical"
    } else {
        "Warning"
    };
    let metadata = take_bound_insight_metadata(
        &mut request.context_hints,
        severity,
        &format!("{:?}", semantics.confidence),
        failure_structured_evidence(&semantics),
    );
    let evidence_status = target_excerpt.evidence_status();
    let target_facts = provider_target_facts(
        &request.command_block,
        &format!(
            "{:?}",
            resolve_execution_scope(
                &request.command_block.session_id,
                &request.command_block.command
            )
        ),
        &format!("{:?}", request.command_block.origin),
        evidence_status,
        target_excerpt.redaction_status,
        target_excerpt.truncation_status(),
        &metadata,
    );
    trim_optional_context_hints(&mut request.context_hints);
    request.context_blocks.clear();
    let other_context_bytes = request
        .context_hints
        .iter()
        .map(|hint| hint.len() + 1)
        .sum();
    request.context_hints.push(build_provider_evidence_payload(
        EvidenceBundleInput {
            target_facts,
            target_excerpt: target_excerpt.text.unwrap_or_default(),
            related_facts,
        },
        other_context_bytes,
    ));
}

fn failure_structured_evidence(semantics: &crate::command::FailureSemantics) -> Vec<String> {
    let mut evidence = vec![
        format!("failure_class={:?}", semantics.class),
        format!("failure_auto_eligibility={:?}", semantics.auto_eligibility),
    ];
    let profile = match semantics.class {
        FailureClass::PermissionDenied => Some("permission"),
        FailureClass::BuildOrTestFailure => Some("build_or_test"),
        FailureClass::RuntimeException => Some("runtime_exception"),
        FailureClass::AbnormalSignal => Some("abnormal_signal"),
        _ => None,
    };
    if let Some(profile) = profile {
        evidence.push(format!("failure_profile={profile}"));
    }
    if semantics.class == FailureClass::RuntimeException {
        evidence.push(
            "failure_objectives=first_failing_frame,direct_cause,minimal_reproduction,smallest_safe_fix"
                .to_string(),
        );
    }
    evidence.extend(
        semantics
            .reasons
            .iter()
            .filter(|reason| {
                matches!(
                    reason,
                    FailureReason::ExitCode(_)
                        | FailureReason::CommandFamily(_)
                        | FailureReason::TerminalSignature(_)
                        | FailureReason::ExcerptDirection(_)
                )
            })
            .enumerate()
            .map(|(index, reason)| format!("failure_reason_{index}={reason:?}")),
    );
    evidence
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;
    use crate::agent::run::ActiveAgentRun;
    use crate::command::{FailureAutoEligibility, FailureSemantics};

    fn failed_block(exit_code: i32, command: &str) -> CommandBlock {
        CommandBlock {
            id: format!("cmd-{exit_code}"),
            session_id: "session-1".to_string(),
            command: command.to_string(),
            origin: Default::default(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
        }
    }

    fn test_active_run() -> ActiveAgentRun {
        let request = AgentRequest {
            id: "active-request".to_string(),
            session_id: "session-1".to_string(),
            command_block: failed_block(1, "active command"),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("active command".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };
        let (approval_sender, _approval_receiver) = mpsc::channel();
        let handle = AgentRunHandle::test_with_approval_sender(approval_sender);
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            origin: AgentRunOrigin::Standard,
            handle,
            provider_name: "fake",
            language: Language::EnUs,
            renderer: renderer.clone(),
            status_animation: renderer.status_animation(),
            markdown_stream: renderer.stream_markdown_agent(),
            governed_events: Vec::new(),
            deferred_events: Vec::new(),
            held_events: Vec::new(),
            cosh_request_filter: crate::evidence::stream::CoshRequestStreamFilter::default(),
            pending_cosh_requests: Vec::new(),
            pending_cosh_request_audits: Vec::new(),
            pending_hook_notifications: Vec::new(),
            rendered_governed_event_count: 0,
            selectable_after_event_index: None,
            started_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            current_phase: String::new(),
            current_message: String::new(),
            has_visible_text_delta: false,
            completed: false,
            host_completed_tool_ids: Vec::new(),
        }
    }

    fn write_output(content: &[u8]) -> String {
        let path = std::env::temp_dir().join(format!(
            "cosh-failure-output-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::write(&path, content).expect("write output");
        path.to_string_lossy().into_owned()
    }

    fn failed_event(block: &CommandBlock) -> ShellEvent {
        ShellEvent {
            kind: ShellEventKind::CommandFailed,
            session_id: block.session_id.clone(),
            command_id: Some(block.id.clone()),
            command: Some(block.command.clone()),
            cwd: Some(block.cwd.clone()),
            end_cwd: Some(block.end_cwd.clone()),
            exit_code: Some(block.exit_code),
            started_at_ms: Some(block.started_at_ms),
            ended_at_ms: Some(block.ended_at_ms),
            duration_ms: Some(block.duration_ms),
            terminal_output_ref: block.output.terminal_output_ref.clone(),
            terminal_output_bytes: Some(block.output.terminal_output_bytes),
            input: None,
            component: None,
            message: None,
            command_origin: Some(block.origin),
            shell_environment_generation: block.shell_environment_generation,
        }
    }

    #[test]
    fn failed_command_analysis_skips_user_interrupts_and_sigpipe() {
        for block in [
            failed_block(130, "sleep 100"),
            failed_block(143, "tail -f /var/log/system.log"),
            failed_block(141, "yes | head -1"),
        ] {
            assert!(!should_analyze_failed_block(&block, AnalysisMode::Auto));
        }
    }

    #[test]
    fn failed_command_analysis_keeps_real_failures() {
        let block = failed_block(2, "cargo test");

        assert_eq!(
            failure_analysis_disposition(
                &[],
                &block,
                AnalysisMode::Auto,
                Some("test result: FAILED. 1 failed")
            ),
            FailureAnalysisDisposition::AutoAnalyze
        );
        assert_eq!(
            failure_analysis_disposition(
                &[],
                &block,
                AnalysisMode::Smart,
                Some("test result: FAILED. 1 failed")
            ),
            FailureAnalysisDisposition::ActionCard
        );
        assert_eq!(
            failure_analysis_disposition(
                &[],
                &block,
                AnalysisMode::Manual,
                Some("test result: FAILED. 1 failed")
            ),
            FailureAnalysisDisposition::SilentRecord
        );
    }

    #[test]
    fn failure_disposition_quiets_usage_help() {
        let block = failed_block(2, "demo --bad");
        let output = "error: unexpected argument '--bad'\nUsage: demo [OPTIONS]\n";

        assert_eq!(
            failure_analysis_disposition(&[], &block, AnalysisMode::Auto, Some(output)),
            FailureAnalysisDisposition::SilentRecord
        );
        assert_eq!(
            failure_analysis_disposition(&[], &block, AnalysisMode::Smart, Some(output)),
            FailureAnalysisDisposition::SilentRecord
        );
        assert_eq!(
            failure_analysis_disposition(&[], &block, AnalysisMode::Manual, Some(output)),
            FailureAnalysisDisposition::SilentRecord
        );
    }

    #[test]
    fn auto_downgrades_build_failure_without_usable_excerpt() {
        let block = failed_block(2, "cargo test");

        assert_eq!(
            failure_analysis_disposition(&[], &block, AnalysisMode::Auto, None),
            FailureAnalysisDisposition::SilentRecord
        );
        assert_eq!(
            failure_analysis_disposition(
                &[],
                &block,
                AnalysisMode::Auto,
                Some("test result: FAILED. 1 failed")
            ),
            FailureAnalysisDisposition::AutoAnalyze
        );
    }

    #[test]
    fn generic_failure_is_silent_in_every_mode() {
        let block = failed_block(1, "demo");
        for mode in [
            AnalysisMode::Smart,
            AnalysisMode::Auto,
            AnalysisMode::Manual,
        ] {
            assert_eq!(
                failure_analysis_disposition(&[], &block, mode, Some("runtime error")),
                FailureAnalysisDisposition::SilentRecord
            );
        }
    }

    #[test]
    fn auto_keeps_new_failure_inputs_as_user_confirmed_actions() {
        for (exit_code, command, output) in [
            (1, "ninja", "ninja: build stopped: subcommand failed.\n"),
            (1, "./deploy", "permission denied\n"),
            (132, "./crash", "illegal instruction\n"),
            (
                1,
                "python app.py",
                "Traceback (most recent call last):\nValueError: boom\n",
            ),
        ] {
            let block = failed_block(exit_code, command);
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Auto, Some(output)),
                FailureAnalysisDisposition::ActionCard,
                "{command} exit={exit_code}"
            );
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Smart, Some(output)),
                FailureAnalysisDisposition::ActionCard,
                "{command} exit={exit_code}"
            );
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Manual, Some(output)),
                FailureAnalysisDisposition::SilentRecord,
                "{command} exit={exit_code}"
            );
        }
    }

    #[test]
    fn phase12_failure_fixtures_cover_semantics_and_all_modes() {
        let silent = FailureAnalysisDisposition::SilentRecord;
        let suggest = FailureAnalysisDisposition::ActionCard;
        let auto = FailureAnalysisDisposition::AutoAnalyze;
        for (
            name,
            exit_code,
            command,
            output,
            expected_class,
            expected_confidence,
            expected_eligibility,
            expected_smart,
            expected_auto,
        ) in [
            (
                "cargo-test-legacy",
                101,
                "cargo test",
                Some("test result: FAILED. 1 failed\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "cargo-build-legacy",
                101,
                "cargo build",
                Some("error: could not compile `demo`\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "make-legacy",
                2,
                "make all",
                Some("make: *** [all] Error 2\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "npm-legacy",
                1,
                "npm test",
                Some("npm ERR! Test failed\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "pytest-legacy",
                1,
                "pytest",
                Some("= 1 failed in 0.02s =\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "exit-126-legacy",
                126,
                "./script",
                Some("permission denied\n"),
                FailureClass::PermissionDenied,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "fatal-134-legacy",
                134,
                "./crash",
                Some("aborted (core dumped)\n"),
                FailureClass::AbnormalSignal,
                FailureConfidence::High,
                FailureAutoEligibility::LegacyAllowlisted,
                suggest,
                auto,
            ),
            (
                "cargo-rerun-suggest-only",
                101,
                "cargo test",
                Some("error: test failed, to rerun pass `--lib`\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "ninja-suggest-only",
                1,
                "ninja",
                Some("ninja: build stopped: subcommand failed.\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "maven-suggest-only",
                1,
                "mvn test",
                Some("[INFO] BUILD FAILURE\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "gradle-suggest-only",
                1,
                "./gradlew test",
                Some("BUILD FAILED in 2s\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "go-test-suggest-only",
                1,
                "go test ./...",
                Some("FAIL\texample.com/project\t0.02s\n"),
                FailureClass::BuildOrTestFailure,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "runtime-exception-suggest-only",
                1,
                "python app.py",
                Some("Traceback (most recent call last):\nValueError: boom\n"),
                FailureClass::RuntimeException,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "output-permission-suggest-only",
                1,
                "./deploy",
                Some("deploy: EACCES: permission denied\n"),
                FailureClass::PermissionDenied,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "fatal-132-suggest-only",
                132,
                "./crash",
                Some("illegal instruction\n"),
                FailureClass::AbnormalSignal,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "fatal-135-suggest-only",
                135,
                "./crash",
                Some("bus error\n"),
                FailureClass::AbnormalSignal,
                FailureConfidence::High,
                FailureAutoEligibility::SuggestOnly,
                suggest,
                suggest,
            ),
            (
                "summary-without-family",
                1,
                "printf fixture",
                Some("ninja: build stopped: subcommand failed.\n"),
                FailureClass::GenericRuntimeFailure,
                FailureConfidence::Medium,
                FailureAutoEligibility::SuggestOnly,
                silent,
                silent,
            ),
            (
                "unsupported-localized-summary",
                1,
                "mvn test",
                Some("构建失败\n"),
                FailureClass::GenericRuntimeFailure,
                FailureConfidence::Medium,
                FailureAutoEligibility::SuggestOnly,
                silent,
                silent,
            ),
            (
                "unknown-signal",
                142,
                "./unknown",
                None,
                FailureClass::UnknownFailure,
                FailureConfidence::Low,
                FailureAutoEligibility::SuggestOnly,
                silent,
                silent,
            ),
        ] {
            let block = failed_block(exit_code, command);
            let semantics = classify_failure(&block, &[], output);
            assert_eq!(semantics.class, expected_class, "{name}: class");
            assert_eq!(
                semantics.confidence, expected_confidence,
                "{name}: confidence"
            );
            assert_eq!(
                semantics.auto_eligibility, expected_eligibility,
                "{name}: eligibility"
            );
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Smart, output),
                expected_smart,
                "{name}: smart"
            );
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Auto, output),
                expected_auto,
                "{name}: auto"
            );
            assert_eq!(
                failure_analysis_disposition(&[], &block, AnalysisMode::Manual, output),
                silent,
                "{name}: manual"
            );
        }
    }

    #[test]
    fn failure_output_excerpt_is_bounded() {
        let mut block = failed_block(2, "demo --bad");
        let path = write_output(&vec![b'a'; FAILURE_OUTPUT_EXCERPT_MAX_BYTES + 1024]);
        block.output.terminal_output_ref = Some(path.clone());

        let excerpt = failure_output_excerpt(&block).expect("excerpt");
        let _ = std::fs::remove_file(path);

        assert!(excerpt.len() <= FAILURE_OUTPUT_EXCERPT_MAX_BYTES);
    }

    #[test]
    fn failure_output_excerpt_keeps_real_head_and_tail() {
        let mut block = failed_block(1, "cargo test");
        let output = format!(
            "error[E0308]: mismatched types\n{}test result: FAILED. 1 failed\n",
            "middle output\n".repeat(300)
        );
        let path = write_output(output.as_bytes());
        block.output.terminal_output_ref = Some(path.clone());
        block.output.terminal_output_bytes = output.len() as u64;

        let excerpt = failure_output_excerpt(&block).expect("bounded excerpt");
        assert!(excerpt.starts_with("error[E0308]"), "{excerpt}");
        assert!(
            excerpt.ends_with("test result: FAILED. 1 failed"),
            "{excerpt}"
        );
        assert!(excerpt.lines().count() <= FAILURE_OUTPUT_EXCERPT_MAX_LINES);
        assert!(excerpt.len() <= FAILURE_OUTPUT_EXCERPT_MAX_BYTES);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failure_output_status_uses_typed_head_tail_truncation() {
        let mut block = failed_block(1, "cargo test");
        let output = (0..200)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let path = write_output(output.as_bytes());
        block.output.terminal_output_ref = Some(path.clone());
        block.output.terminal_output_bytes = output.len() as u64;

        let excerpt = failure_output_evidence(&block);
        let status = failure_output_status(&block, &excerpt);

        std::fs::remove_file(path).expect("remove output");
        assert_eq!(status, OutputExcerptStatus::Truncated);
    }

    #[test]
    fn invalid_utf8_existing_output_has_explicit_read_failed_status() {
        let mut block = failed_block(2, "demo --bad");
        let path = write_output(&[0xff]);

        block.output.terminal_output_ref = Some(path.clone());
        let excerpt = failure_output_evidence(&block);
        let status = failure_output_status(&block, &excerpt);

        std::fs::remove_file(path).expect("remove output");
        assert_eq!(status, OutputExcerptStatus::ReadFailed);
    }

    #[test]
    fn explicit_failed_command_target_skips_internal_origins() {
        let mut user_block = failed_block(2, "demo --bad");
        user_block.id = "user".to_string();
        user_block.origin = CommandOrigin::UserInteractive;
        user_block.ended_at_ms = 10;
        let mut provider_block = failed_block(1, "provider helper");
        provider_block.id = "provider".to_string();
        provider_block.origin = CommandOrigin::ProviderTool;
        provider_block.ended_at_ms = 20;
        let mut internal_block = failed_block(1, "validation helper");
        internal_block.id = "internal".to_string();
        internal_block.origin = CommandOrigin::ShellInternal;
        internal_block.ended_at_ms = 30;
        let blocks = vec![user_block, provider_block, internal_block];
        let event = ShellEvent::user_input_intercepted("session-1", "/explain last error");
        let state = InlineState::default();

        let target =
            latest_pending_failed_block_before_event(&blocks, &state, &event).expect("target");

        assert_eq!(target.id, "user");
    }

    #[test]
    fn oom_like_failed_command_has_no_sysom_hint_before_finalizer() {
        let mut block = failed_block(137, "/tmp/run-worker");
        block.output.terminal_output_ref = Some(write_output(b"Killed\n"));

        let excerpt = failure_output_excerpt(&block).expect("excerpt");
        let _ = std::fs::remove_file(block.output.terminal_output_ref.unwrap());

        assert!(excerpt.contains("Killed"));
    }

    #[test]
    fn failed_command_analysis_uses_related_history_facts() {
        let mut setup = failed_block(0, "echo setup context");
        setup.id = "setup".to_string();
        setup.cwd = "/repo".to_string();
        setup.end_cwd = "/repo".to_string();
        setup.status = CommandStatus::Completed;
        setup.output.terminal_output_ref = Some("/tmp/setup-output.txt".to_string());
        let mut previous_failed = failed_block(2, "grep --bad-option");
        previous_failed.id = "previous-failed".to_string();
        previous_failed.ended_at_ms = 20;
        previous_failed.output.terminal_output_ref = Some("/tmp/previous-output.txt".to_string());
        let mut target = failed_block(127, "missing-context-command");
        target.id = "target".to_string();
        target.cwd = "/repo".to_string();
        target.end_cwd = "/repo".to_string();
        target.ended_at_ms = 30;
        target.output.terminal_output_ref = Some("/tmp/target-output.txt".to_string());
        let blocks = vec![setup.clone(), previous_failed.clone(), target.clone()];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Auto,
            ..InlineState::default()
        };
        state.agent_run.active = Some(test_active_run());
        let mut output = Vec::new();

        start_agent_for_block(
            &target,
            &blocks,
            &findings,
            &adapter,
            &mut state,
            &mut output,
            FailedCommandAgentStartOptions {
                selectable_after_event_index: None,
                trigger: FailedCommandAnalysisTrigger::Auto,
            },
        )
        .expect("start failed command analysis");

        let request = &state.agent_run.queued_requests[0].request;
        assert!(request.context_blocks.is_empty());
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("bounded insight evidence");
        assert!(evidence.contains("echo setup context"), "{evidence}");
        assert!(evidence.contains("grep --bad-option"), "{evidence}");
        assert!(!evidence.contains("/tmp/setup-output.txt"), "{evidence}");
        assert_eq!(request.user_input, None);
        assert!(!request.user_confirmed);
        assert!(request
            .context_hints
            .iter()
            .any(|hint| hint == "__cosh_request_source=auto_failure_analysis"));
        assert!(request
            .context_hints
            .iter()
            .any(|hint| hint == "__cosh_context_binding=failed_command"));
    }

    #[test]
    fn provider_context_keeps_mandatory_metadata_within_total_budget() {
        let mut block = failed_block(126, &format!("deploy {}", "x".repeat(12 * 1024)));
        block.origin = CommandOrigin::UserInteractive;
        let output_path =
            write_output(format!("permission denied\n{}", "e".repeat(30 * 1024)).as_bytes());
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = 30 * 1024;
        let mut request = agent_request_for_auto_failure("session-1", &block, &[]);

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        let serialized_context_bytes = request
            .context_hints
            .iter()
            .map(|hint| hint.len() + 1)
            .sum::<usize>();
        assert!(
            serialized_context_bytes <= crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES,
            "{serialized_context_bytes}"
        );
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        for required in [
            "command_id=cmd-126",
            "exit_code=126",
            "execution_scope=ExecutionScope",
            "origin=UserInteractive",
            "evidence_status=available",
            "redaction_status=",
            "truncation_status=truncated",
            "severity=Warning",
            "confidence=High",
            "structured_evidence=failure_class=PermissionDenied",
            "bundle_status:",
        ] {
            assert!(
                evidence.contains(required),
                "missing {required}: {evidence}"
            );
        }
    }

    #[test]
    fn provider_context_reports_redaction_from_the_injected_tail_excerpt() {
        let mut block = failed_block(126, "deploy app");
        let mut output = (0..120)
            .map(|index| format!("setup detail {index}"))
            .collect::<Vec<_>>();
        output.push("Authorization: Bearer abc.def.ghi".to_string());
        let output = output.join("\n");
        let output_path = write_output(output.as_bytes());
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = output.len() as u64;
        let mut request = agent_request_for_auto_failure("session-1", &block, &[]);

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert!(
            evidence.contains("redaction_status=excerpt_redacted"),
            "{evidence}"
        );
        assert!(evidence.contains("Bearer <redacted>"), "{evidence}");
        assert!(!evidence.contains("abc.def.ghi"), "{evidence}");
    }

    #[test]
    fn provider_context_does_not_duplicate_overlapping_build_lines() {
        let mut block = failed_block(2, "make all");
        let mut output = (0..79)
            .map(|index| format!("compile detail {index}"))
            .collect::<Vec<_>>();
        output.push("make: *** [all] Error 2".to_string());
        let output = output.join("\n");
        let output_path = write_output(output.as_bytes());
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = output.len() as u64;
        let mut request = agent_request_for_auto_failure("session-1", &block, &[]);

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert_eq!(
            evidence.matches("compile detail 40").count(),
            1,
            "{evidence}"
        );
    }

    #[test]
    fn provider_context_reports_complete_for_untruncated_permission_excerpt() {
        let mut block = failed_block(126, "deploy app");
        let output = (0..100)
            .map(|index| format!("permission detail {index}: {}", "x".repeat(60)))
            .collect::<Vec<_>>()
            .join("\n");
        let output_path = write_output(output.as_bytes());
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = output.len() as u64;
        let mut request = agent_request_for_auto_failure("session-1", &block, &[]);

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert!(
            evidence.contains("truncation_status=complete"),
            "{evidence}"
        );
    }

    #[test]
    fn provider_context_drops_oversized_optional_hints_before_budgeting_evidence() {
        let mut block = failed_block(126, "deploy app");
        let output = b"permission denied\n";
        let output_path = write_output(output);
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = output.len() as u64;
        let mut request = agent_request_for_auto_failure("session-1", &block, &[]);
        request.context_hints.push("optional".repeat(8 * 1024));

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        assert!(request
            .context_hints
            .iter()
            .all(|hint| !hint.starts_with("optionaloptional")));
        assert!(
            request.context_hints.iter().map(String::len).sum::<usize>()
                + request.context_hints.len().saturating_sub(1)
                <= crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES
        );
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert!(evidence.contains("command_id="), "{evidence}");
        assert!(
            evidence.contains("truncation_status=complete"),
            "{evidence}"
        );
        assert!(evidence.contains("permission denied"), "{evidence}");
    }

    #[test]
    fn runtime_exception_evidence_uses_focused_profile() {
        let mut block = failed_block(1, "python app.py");
        let output =
            b"Traceback (most recent call last):\n  File \"app.py\", line 1\nValueError: boom\n";
        let output_path = write_output(output);
        block.output.terminal_output_ref = Some(output_path.clone());
        block.output.terminal_output_bytes = output.len() as u64;
        let mut request = agent_request_after_confirmation("session-1", &block, &[], true)
            .expect("confirmed request");

        attach_failure_evidence_bundle(&mut request);

        let _ = std::fs::remove_file(output_path);
        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert!(
            evidence.contains("failure_profile=runtime_exception"),
            "{evidence}"
        );
        assert!(
            evidence.contains("failure_objectives=first_failing_frame,direct_cause,minimal_reproduction,smallest_safe_fix"),
            "{evidence}"
        );
        assert!(
            evidence.contains("failure_auto_eligibility=SuggestOnly"),
            "{evidence}"
        );
    }

    #[test]
    fn failure_evidence_records_each_closed_analysis_profile() {
        for (class, expected_profile) in [
            (FailureClass::PermissionDenied, "failure_profile=permission"),
            (
                FailureClass::BuildOrTestFailure,
                "failure_profile=build_or_test",
            ),
            (
                FailureClass::RuntimeException,
                "failure_profile=runtime_exception",
            ),
            (
                FailureClass::AbnormalSignal,
                "failure_profile=abnormal_signal",
            ),
        ] {
            let evidence = failure_structured_evidence(&FailureSemantics {
                class,
                confidence: FailureConfidence::High,
                auto_eligibility: FailureAutoEligibility::SuggestOnly,
                reasons: Vec::new(),
            });
            assert!(
                evidence.iter().any(|item| item == expected_profile),
                "missing {expected_profile}: {evidence:?}"
            );
        }
    }

    #[test]
    fn failure_evidence_records_structured_classifier_reasons() {
        let evidence = failure_structured_evidence(&FailureSemantics {
            class: FailureClass::PermissionDenied,
            confidence: FailureConfidence::High,
            auto_eligibility: FailureAutoEligibility::LegacyAllowlisted,
            reasons: vec![
                FailureReason::ExitCode(126),
                FailureReason::PermissionDenied,
            ],
        });

        assert!(
            evidence
                .iter()
                .any(|item| item == "failure_reason_0=ExitCode(126)"),
            "{evidence:?}"
        );
        assert!(
            !evidence
                .iter()
                .any(|item| item.contains("PermissionDenied") && item.starts_with("failure_reason")),
            "{evidence:?}"
        );
    }

    #[test]
    fn manual_mode_does_not_render_failed_command_card() {
        let mut block = failed_block(127, "missing-command");
        block.id = "target".to_string();
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();

        let events = [failed_event(&block)];
        collect_failed_command_insights(&events, &[block], &mut state, &mut output, 0)
            .expect("render failed command card");

        let rendered = String::from_utf8(output).expect("utf8");
        assert!(rendered.is_empty(), "{rendered}");
    }

    #[test]
    fn command_not_found_without_ready_catalog_expires_in_first_batch() {
        let mut block = failed_block(127, "grpe file");
        block.id = "target".to_string();
        block.shell_environment_generation = Some(7);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Smart,
            ..InlineState::default()
        };

        let events = [failed_event(&block)];
        collect_failed_command_insights(&events, &[block.clone()], &mut state, &mut Vec::new(), 0)
            .expect("evaluate failed command");

        assert!(state.evaluated_failed_command_insights.contains("target"));
        assert!(state.pending_command_insight.is_none());
        collect_failed_command_insights(&events, &[block], &mut state, &mut Vec::new(), 0)
            .expect("do not retry expired command");
        assert!(state.pending_command_insight.is_none());
    }

    #[test]
    fn smart_build_failure_produces_agent_prompt_candidate() {
        let mut block = failed_block(101, "cargo test");
        block.id = "target".to_string();
        block.origin = CommandOrigin::UserInteractive;
        let path = write_output(b"error: could not compile demo\ntest result: FAILED");
        block.output.terminal_output_ref = Some(path.clone());
        let events = [failed_event(&block)];
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Smart,
            ..InlineState::default()
        };

        collect_failed_command_insights(&events, &[block], &mut state, &mut Vec::new(), 0)
            .expect("collect build insight");

        std::fs::remove_file(path).expect("remove output");
        assert!(matches!(
            state
                .pending_command_insight
                .and_then(|candidate| candidate.suggestion),
            Some(PromptSuggestion::AgentPrompt { .. })
        ));
    }

    #[test]
    fn auto_abnormal_signal_without_excerpt_downgrades_to_agent_prompt() {
        let mut block = failed_block(139, "demo");
        block.id = "target".to_string();
        block.origin = CommandOrigin::UserInteractive;
        let events = [failed_event(&block)];
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Auto,
            ..InlineState::default()
        };

        collect_failed_command_insights(&events, &[block], &mut state, &mut Vec::new(), 0)
            .expect("collect signal insight");

        assert!(matches!(
            state
                .pending_command_insight
                .and_then(|candidate| candidate.suggestion),
            Some(PromptSuggestion::AgentPrompt { .. })
        ));
    }

    #[test]
    fn manual_mode_skips_user_interrupted_failed_command_card() {
        let mut block = failed_block(1, "aliyun configure");
        block.started_at_ms = 100;
        block.ended_at_ms = 200;
        let mut ctrl_c = ShellEvent::user_input_intercepted("session-1", "ctrl_c");
        ctrl_c.component = Some("control".to_string());
        ctrl_c.started_at_ms = Some(150);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();

        collect_failed_command_insights(&[ctrl_c], &[block], &mut state, &mut output, 0)
            .expect("render failed command card");

        assert!(output.is_empty());
    }
}
