use crate::evidence::model::OutputExcerptDirection;
use crate::evidence::output_policy::bounded_output_excerpt_for_block;
use crate::insight::evidence::{
    build_provider_evidence_payload, provider_target_facts, take_bound_insight_metadata,
    trim_optional_context_hints, EvidenceBundleInput,
};
use crate::runtime::prelude::*;
use crate::runtime::state::PendingInputGhostBinding;

pub(crate) fn render_intercept_agent_guidance<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        clear_dismissed_prompt_ghost_context(event, state);
        if !is_standalone_agent_intercept(event) {
            continue;
        }

        let key = stable_event_key("intercept", event_index, event);
        if !state.handled_intercepts.insert(key) {
            continue;
        }

        if let Some(answer_run) =
            agent_request_from_pending_question_answer(event, event_index, state)
        {
            render_question_answer_notice(state, &answer_run, output)?;
            stop_active_agent_run_without_rendering(state, output)?;
            state.agent_run.needs_prompt_after_run = event.cwd.is_none();
            start_agent_run_with_origin(
                &answer_run.request,
                answer_run.origin,
                adapter,
                state,
                output,
                Some(event_index),
            )?;
            output.flush()?;
            continue;
        }

        let (request, origin) = match agent_request_from_pending_insight(event, blocks, state) {
            Some(mut request) => {
                attach_bound_insight_evidence(&mut request, state);
                (Some(request), AgentRunOrigin::InsightPrompt)
            }
            None => (
                agent_request_from_intercepted_input(event, event_index, true),
                AgentRunOrigin::Standard,
            ),
        };
        if let Some(mut request) = request {
            let user_input = request.user_input.clone();
            if let Some(input) = user_input.as_deref() {
                bind_pending_input_ghost_context(&mut request, state, event);
                if let Some(hint) = continuity_prompt_hint(state, input) {
                    request.context_hints.push(hint);
                }
            }
            state.agent_run.needs_prompt_after_run = event.cwd.is_none();
            start_agent_run_with_origin(
                &request,
                origin,
                adapter,
                state,
                output,
                Some(event_index),
            )?;
            if let Some(input) = user_input.as_deref() {
                record_user_intent(state, input);
            }
        }
        output.flush()?;
    }

    Ok(())
}

fn attach_bound_insight_evidence(request: &mut AgentRequest, state: &mut InlineState) {
    if request.command_block.exit_code != 0 {
        crate::agent::failed_command::attach_failure_evidence_bundle(request);
        return;
    }
    let bounded_excerpt = bounded_output_excerpt_for_block(
        &request.command_block,
        OutputExcerptDirection::Head,
        120,
        12 * 1024,
    );
    let evidence_status = bounded_excerpt.evidence_status();
    let truncation_status = bounded_excerpt.truncation_status();
    let redaction_status = bounded_excerpt.redaction_status;
    let excerpt = bounded_excerpt.text.unwrap_or_default();
    let scope = crate::insight::scope::resolve_execution_scope(
        &request.command_block.session_id,
        &request.command_block.command,
    );
    let related_facts = state.insight_correlation.recent_memory_pressure_facts(
        &scope,
        request.command_block.ended_at_ms,
        &request.command_block.id,
    );
    let metadata =
        take_bound_insight_metadata(&mut request.context_hints, "Candidate", "High", Vec::new());
    let target_facts = provider_target_facts(
        &request.command_block,
        &format!("{scope:?}"),
        &format!("{:?}", request.command_block.origin),
        evidence_status,
        redaction_status,
        truncation_status,
        &metadata,
    );
    trim_optional_context_hints(&mut request.context_hints);
    let other_context_bytes = request
        .context_hints
        .iter()
        .map(|hint| hint.len() + 1)
        .sum();
    request.context_hints.push(build_provider_evidence_payload(
        EvidenceBundleInput {
            target_facts,
            target_excerpt: excerpt,
            related_facts,
        },
        other_context_bytes,
    ));
}

fn clear_dismissed_prompt_ghost_context(event: &ShellEvent, state: &mut InlineState) {
    if event.kind == ShellEventKind::UserInputIntercepted
        && event.component.as_deref() == Some("prompt_ghost")
        && event.message.as_deref() == Some("dismissed")
    {
        state.pending_input_ghost_binding = None;
    }
}

fn bind_pending_input_ghost_context(
    request: &mut AgentRequest,
    state: &mut InlineState,
    event: &ShellEvent,
) {
    if crate::types::request_context_binding(request) != AgentContextBinding::FreeForm {
        return;
    }
    if prompt_ghost_suggestion_id(event).is_none() {
        return;
    }
    let Some(PendingInputGhostBinding::AgentContext(binding)) =
        state.pending_input_ghost_binding.take()
    else {
        return;
    };
    crate::types::set_request_context_binding(request, binding);
}

fn agent_request_from_pending_insight(
    event: &ShellEvent,
    blocks: &[CommandBlock],
    state: &mut InlineState,
) -> Option<AgentRequest> {
    let submitted_suggestion_id = prompt_ghost_suggestion_id(event)?;
    if !matches!(
        state.pending_input_ghost_binding.as_ref(),
        Some(PendingInputGhostBinding::Insight(_))
    ) {
        return None;
    }
    let PendingInputGhostBinding::Insight(binding) = state.pending_input_ghost_binding.take()?
    else {
        unreachable!("binding kind checked before take");
    };
    if submitted_suggestion_id != Some(binding.suggestion_id.as_str()) {
        return None;
    }
    let findings = findings_from_blocks(blocks);
    agent_request_from_insight_binding(
        &binding,
        &event.session_id,
        event.input.as_deref().unwrap_or_default(),
        blocks,
        &findings,
    )
}

fn is_standalone_agent_intercept(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && (matches!(
            event.component.as_deref(),
            Some("natural_language") | Some("agent_marker")
        ) || prompt_ghost_suggestion_id(event).is_some())
}

fn prompt_ghost_suggestion_id(event: &ShellEvent) -> Option<Option<&str>> {
    let component = event.component.as_deref()?;
    if component == "prompt_ghost" {
        return Some(None);
    }
    component
        .strip_prefix("prompt_ghost:")
        .filter(|id| !id.is_empty())
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::failed_command::failed_command_candidate;
    use crate::insight::correlation::MemoryPressureFact;
    use crate::insight::model::{
        ExecutionScope, InsightBinding, InsightConfidence, InsightSeverity, InsightTarget,
        OutputExcerptStatus, PromptSuggestion,
    };

    fn prompt_ghost_event(message: Option<&str>, input: Option<&str>) -> ShellEvent {
        ShellEvent {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: "session-1".to_string(),
            command_id: None,
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(1),
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: input.map(str::to_string),
            component: Some("prompt_ghost".to_string()),
            message: message.map(str::to_string),
            command_origin: None,
            shell_environment_generation: None,
        }
    }

    fn source_block() -> CommandBlock {
        CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "cargo test".to_string(),
            origin: Default::default(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
        }
    }

    fn insight_binding(suggestion_id: &str) -> InsightBinding {
        InsightBinding {
            suggestion_id: suggestion_id.to_string(),
            target: InsightTarget {
                insight_id: "insight-1".to_string(),
                source_session_id: "session-1".to_string(),
                source_command_block_id: "cmd-1".to_string(),
                scope: ExecutionScope::local("session-1"),
                evidence_handle: None,
                evidence_status: OutputExcerptStatus::Available,
                severity: crate::insight::model::InsightSeverity::Warning,
                confidence: crate::insight::model::InsightConfidence::High,
                evidence: Vec::new(),
                created_at_ms: 1,
            },
        }
    }

    #[test]
    fn dismissed_prompt_ghost_clears_pending_binding() {
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::AgentContext(
                AgentContextBinding::StartupHealthFollowUp,
            )),
            ..Default::default()
        };

        clear_dismissed_prompt_ghost_context(
            &prompt_ghost_event(Some("dismissed"), None),
            &mut state,
        );

        assert!(state.pending_input_ghost_binding.is_none());
    }

    #[test]
    fn accepted_prompt_ghost_does_not_clear_pending_binding_before_binding() {
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::AgentContext(
                AgentContextBinding::StartupHealthFollowUp,
            )),
            ..Default::default()
        };

        clear_dismissed_prompt_ghost_context(
            &prompt_ghost_event(
                Some("input intercepted before reaching bash"),
                Some("analyze"),
            ),
            &mut state,
        );

        assert!(state.pending_input_ghost_binding.is_some());
    }

    #[test]
    fn matching_insight_suggestion_consumes_binding_once_and_uses_source_block() {
        let block = source_block();
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::Insight(Box::new(
                insight_binding("suggestion-1"),
            ))),
            ..Default::default()
        };
        let mut event = prompt_ghost_event(
            Some("input intercepted before reaching bash"),
            Some("analyze edited failure"),
        );
        event.component = Some("prompt_ghost:suggestion-1".to_string());

        let request =
            agent_request_from_pending_insight(&event, std::slice::from_ref(&block), &mut state)
                .expect("matching bound request");

        assert_eq!(request.command_block.id, block.id);
        assert_eq!(
            request.user_input.as_deref(),
            Some("analyze edited failure")
        );
        assert!(state.pending_input_ghost_binding.is_none());
        assert!(agent_request_from_pending_insight(
            &event,
            std::slice::from_ref(&block),
            &mut state,
        )
        .is_none());
    }

    #[test]
    fn mismatched_suggestion_clears_binding_and_falls_back_without_history() {
        let block = source_block();
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::Insight(Box::new(
                insight_binding("new-suggestion"),
            ))),
            ..Default::default()
        };
        let mut event = prompt_ghost_event(
            Some("input intercepted before reaching bash"),
            Some("analyze visible text"),
        );
        event.component = Some("prompt_ghost:old-suggestion".to_string());

        assert!(agent_request_from_pending_insight(
            &event,
            std::slice::from_ref(&block),
            &mut state,
        )
        .is_none());
        assert!(state.pending_input_ghost_binding.is_none());

        let fallback =
            agent_request_from_intercepted_input(&event, 1, true).expect("free-form fallback");
        assert_eq!(
            crate::types::request_context_binding(&fallback),
            AgentContextBinding::FreeForm
        );
        assert!(fallback.findings.is_empty());
        assert!(fallback.context_blocks.is_empty());
    }

    #[test]
    fn missing_source_block_clears_binding_without_cross_binding() {
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::Insight(Box::new(
                insight_binding("suggestion-1"),
            ))),
            ..Default::default()
        };
        let mut event = prompt_ghost_event(
            Some("input intercepted before reaching bash"),
            Some("analyze visible text"),
        );
        event.component = Some("prompt_ghost:suggestion-1".to_string());

        assert!(agent_request_from_pending_insight(&event, &[], &mut state).is_none());
        assert!(state.pending_input_ghost_binding.is_none());
    }

    #[test]
    fn session_mismatch_clears_binding_without_cross_binding() {
        let block = source_block();
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding::Insight(Box::new(
                insight_binding("suggestion-1"),
            ))),
            ..Default::default()
        };
        let mut event = prompt_ghost_event(
            Some("input intercepted before reaching bash"),
            Some("analyze visible text"),
        );
        event.session_id = "other-session".to_string();
        event.component = Some("prompt_ghost:suggestion-1".to_string());

        assert!(agent_request_from_pending_insight(
            &event,
            std::slice::from_ref(&block),
            &mut state,
        )
        .is_none());
        assert!(state.pending_input_ghost_binding.is_none());
    }

    #[test]
    fn memory_evidence_includes_recent_provider_safe_facts_not_boolean_marker() {
        let mut block = source_block();
        block.command = "ps aux".to_string();
        block.exit_code = 0;
        block.status = CommandStatus::Completed;
        block.ended_at_ms = 2_000;
        let mut request = AgentRequest {
            id: "request-1".to_string(),
            session_id: block.session_id.clone(),
            command_block: block,
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("analyze memory".to_string()),
            findings: Vec::new(),
            mode: AgentMode::AnalysisOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };
        let mut state = InlineState::default();
        state.insight_correlation.record(MemoryPressureFact {
            scope: ExecutionScope::local("session-1"),
            ended_at_ms: 1_000,
            severity: InsightSeverity::Warning,
            confidence: InsightConfidence::High,
            source_command_block_id: "cmd-pressure".to_string(),
            provider_safe_fact: "memory_pressure severity=Warning ended_at_ms=1000".to_string(),
        });

        attach_bound_insight_evidence(&mut request, &mut state);

        let evidence = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("insight_evidence\n"))
            .expect("insight evidence");
        assert!(evidence.contains(
            "source_command_block_id=cmd-pressure; memory_pressure severity=Warning ended_at_ms=1000"
        ));
        assert!(!evidence.contains("recent_memory_pressure=true"));
    }

    #[test]
    fn smart_and_auto_failure_requests_share_the_same_bounded_evidence() {
        for (command, exit_code, output, expected_profile) in [
            (
                "./demo-script",
                126,
                "bash: ./demo-script: Permission denied\n",
                "failure_profile=permission",
            ),
            (
                "make all",
                2,
                "make: *** [Makefile:2: all] Error 1\n",
                "failure_profile=build_or_test",
            ),
            (
                "python3 demo.py",
                1,
                "Traceback (most recent call last):\n  File \"demo.py\", line 1\nRuntimeError: boom\n",
                "failure_profile=runtime_exception",
            ),
            (
                "./demo-signal",
                139,
                "Segmentation fault (core dumped)\n",
                "failure_profile=abnormal_signal",
            ),
        ] {
            let output_path = std::env::temp_dir().join(format!(
                "cosh-smart-auto-prompt-parity-{}-{exit_code}",
                std::process::id()
            ));
            std::fs::write(&output_path, output).expect("write failure output");
            let mut block = source_block();
            block.command = command.to_string();
            block.exit_code = exit_code;
            block.output.terminal_output_ref = Some(output_path.to_string_lossy().into_owned());
            block.output.terminal_output_bytes = output.len() as u64;
            let candidate = failed_command_candidate(&[], &block).expect("failure insight");
            let PromptSuggestion::AgentPrompt { binding } =
                candidate.suggestion.expect("agent prompt")
            else {
                panic!("expected agent prompt");
            };
            let mut state = InlineState {
                pending_input_ghost_binding: Some(PendingInputGhostBinding::Insight(
                    binding.clone(),
                )),
                ..Default::default()
            };
            let mut event = prompt_ghost_event(None, Some("分析这次失败"));
            event.component = Some(format!("prompt_ghost:{}", binding.suggestion_id));
            let mut smart =
                agent_request_from_pending_insight(&event, std::slice::from_ref(&block), &mut state)
                    .expect("smart request");
            attach_bound_insight_evidence(&mut smart, &mut state);

            let mut auto = agent_request_for_auto_failure("session-1", &block, &[]);
            crate::agent::failed_command::attach_failure_evidence_bundle(&mut auto);
            let _ = std::fs::remove_file(output_path);

            let evidence = |request: &AgentRequest| {
                request
                    .context_hints
                    .iter()
                    .find(|hint| hint.starts_with("insight_evidence\n"))
                    .cloned()
                    .expect("insight evidence")
            };
            let smart_evidence = evidence(&smart);
            assert_eq!(smart_evidence, evidence(&auto));
            assert!(smart_evidence.contains(expected_profile), "{smart_evidence}");
            assert!(smart.user_confirmed);
            assert!(!auto.user_confirmed);
            assert!(smart.user_input.is_some());
            assert!(auto.user_input.is_none());
        }
    }
}
