use crate::agent::approval_bridge::{render_auto_approved_tool, render_trusted_tool};
use crate::runtime::prelude::*;

use super::events::{event_may_render_structured_surface, render_active_agent_event};

pub(crate) fn render_new_agent_structured_events<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let (events, run_request, origin) = {
        let Some(active_run) = state.agent_run.active.as_mut() else {
            return Ok(());
        };
        let start = active_run.rendered_governed_event_count;
        let end = active_run.governed_events.len();
        if start >= end {
            return Ok(());
        }
        let events = active_run.governed_events[start..end].to_vec();
        if events.iter().any(event_may_render_structured_surface) {
            active_run.prepare_structured_surface(output)?;
        }
        active_run.rendered_governed_event_count = end;
        (events, active_run.request.clone(), active_run.origin)
    };
    render_agent_structured_events(state, &events, Some(&run_request), origin, output, adapter)
}

pub(crate) fn render_agent_structured_events<W: Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    origin: AgentRunOrigin,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let ignore_tool_calls = adapter.capabilities().control_protocol;
    let (question_ids, question_rejection) = record_user_questions(
        state,
        governed_events,
        origin,
        run_request.map(|request| request.id.as_str()),
    );
    let activity_events = question_rejection
        .map(|(_, event_index, _)| &governed_events[..event_index])
        .unwrap_or(governed_events);
    let activity_ids = record_activity_rows_with_policy(
        state,
        activity_events,
        ActivityRecordPolicy {
            suppress_provider_native_shell: adapter.capabilities().control_protocol,
            shell_evidence_tool_available: shell_evidence_tool_available(state, adapter),
            origin,
        },
    );
    render_provider_native_shell_transcript(state, &activity_ids, output)?;
    render_activity_rows(state, &activity_ids, output)?;
    render_user_questions(state, &question_ids, output)?;
    if let Some((reason, _, should_report)) = question_rejection {
        if should_report {
            let active_run = state.agent_run.active.as_mut().expect("active Core run");
            render_active_agent_event(
                active_run,
                AgentEvent::AgentFailed {
                    run_id: active_run.request.id.clone(),
                    error: format!("cosh-core-question-protocol:{reason}"),
                },
                output,
                None,
            )?;
            active_run.completed = true;
        }
        return Ok(());
    }
    crate::auth::runtime::record_auth_results(state, governed_events, output)?;
    let auth_ids = crate::auth::runtime::record_auth_required(state, governed_events);
    crate::auth::runtime::render_auth_panel(state, &auth_ids, output)?;
    if render_trusted_tool(state, governed_events, run_request, origin, output, adapter)? {
        return Ok(());
    }
    if render_auto_approved_tool(state, governed_events, run_request, origin, output, adapter)? {
        return Ok(());
    }
    if state.approval_mode == CoshApprovalMode::Recommend {
        return Ok(());
    }
    let approval_ids = record_approval_requests(
        state,
        governed_events,
        run_request,
        origin,
        ignore_tool_calls,
    );
    render_approval_requests(state, &approval_ids, output)?;
    Ok(())
}

fn shell_evidence_tool_available(state: &InlineState, adapter: &AdapterInstance) -> bool {
    state
        .agent_run
        .active
        .as_ref()
        .map(|active| {
            active
                .handle
                .control_capabilities()
                .can_handle_shell_evidence_tool
        })
        .unwrap_or_else(|| adapter.name() == "cosh-core" && adapter.capabilities().control_protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::{state_has_pending_interaction, tests::test_active_run};

    fn governed(event: AgentEvent, policy_decision: GovernancePolicyDecision) -> GovernedEvent {
        GovernedEvent {
            decision: GovernanceDecision::Display,
            policy_decision,
            event,
            reason: "test".to_string(),
            display_text: String::new(),
            auto_execute: false,
        }
    }

    fn question(provider_request_id: &str, text: &str) -> GovernedEvent {
        governed(
            AgentEvent::UserQuestion {
                run_id: "run-1".to_string(),
                provider_request_id: Some(provider_request_id.to_string()),
                question: text.to_string(),
                options: vec!["One".to_string(), "Two".to_string()],
                allow_free_text: false,
                selection_mode: QuestionSelectionMode::Single,
            },
            GovernancePolicyDecision::DisplayOnly,
        )
    }

    #[test]
    fn conflicting_core_question_discards_trailing_interactions_and_finishes_once() {
        let mut state = InlineState::default();
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut active_run = test_active_run();
        active_run.provider_name = "cosh-core";
        let run_request = active_run.request.clone();
        state.agent_run.active = Some(active_run);
        let mut output = Vec::new();

        render_agent_structured_events(
            &mut state,
            &[question("question-1", "First")],
            Some(&run_request),
            AgentRunOrigin::Standard,
            &mut output,
            &adapter,
        )
        .expect("render initial question");
        let trailing_tool = governed(
            AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "approval-1".to_string(),
                tool_name: "run_shell_command".to_string(),
                tool_input: serde_json::json!({ "command": "rm -f /tmp/example" }),
                tool_use_id: "tool-1".to_string(),
                hook_requires_approval: false,
            },
            GovernancePolicyDecision::NeedsUserApproval,
        );
        let trailing_auth = governed(
            AgentEvent::AuthRequired {
                run_id: "run-1".to_string(),
                request_id: "auth-1".to_string(),
                reason: "credentials required".to_string(),
                error_message: None,
                credentials_unavailable: false,
                providers: Vec::new(),
            },
            GovernancePolicyDecision::DisplayOnly,
        );
        render_agent_structured_events(
            &mut state,
            &[
                question("question-2", "Second"),
                trailing_tool,
                trailing_auth,
            ],
            Some(&run_request),
            AgentRunOrigin::Standard,
            &mut output,
            &adapter,
        )
        .expect("reject concurrent question");

        let active_run = state.agent_run.active.as_ref().expect("active run");
        assert!(active_run.completed);
        assert_eq!(active_run.deferred_events.len(), 1);
        assert_eq!(state.questions.pending_id.as_deref(), Some("q-1"));
        crate::agent::poll::poll_active_agent_run(&mut state, &mut output, &adapter)
            .expect("finish local terminal");
        assert!(state.agent_run.active.is_none());
        assert!(state.questions.pending_id.is_none());
        assert!(state.auth.state.is_none());
        assert!(state.approvals.requests.is_empty());
        assert!(state.activity.rows.is_empty());
        assert!(state.activity.tool_invocations.is_empty());
        assert!(!state_has_pending_interaction(&state));
        let rendered = String::from_utf8(output).expect("UTF-8");
        assert_eq!(
            rendered.matches("Agent question unavailable").count(),
            1,
            "{rendered}"
        );
        assert!(
            !rendered.contains("cosh-core-question-protocol"),
            "{rendered}"
        );
        assert!(!rendered.contains("cancelled"), "{rendered}");
        assert!(!rendered.contains("rm -f /tmp/example"), "{rendered}");
    }
}
