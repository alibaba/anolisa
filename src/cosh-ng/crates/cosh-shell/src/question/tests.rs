use super::*;

#[test]
fn record_user_questions_localizes_empty_question_fallback() {
    let mut state = InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    };
    let events = vec![GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::UserQuestion {
            run_id: "run-1".to_string(),
            provider_request_id: None,
            question: String::new(),
            options: Vec::new(),
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        },
        reason: "display".to_string(),
        display_text: String::new(),
        auto_execute: false,
    }];

    let ids = record_user_questions(&mut state, &events, AgentRunOrigin::InsightPrompt, None);

    assert_eq!(ids, vec!["q-1".to_string()]);
    assert_eq!(state.questions.items[0].question, "Agent 需要你的输入");
    assert_eq!(
        state.questions.items[0].origin,
        AgentRunOrigin::InsightPrompt
    );
    let mut output = Vec::new();
    render_user_questions(&mut state, &ids, &mut output).expect("render question");
    let text = String::from_utf8(output).expect("utf8 question");
    assert!(text.contains("Agent 需要你的输入"), "{text}");
    assert!(!text.contains("Agent needs your input"), "{text}");
}

#[test]
fn pending_question_answer_keeps_the_origin_without_an_active_run() {
    let mut state = InlineState::default();
    let events = vec![GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::UserQuestion {
            run_id: "run-1".to_string(),
            provider_request_id: Some("provider-question-1".to_string()),
            question: "Choose a scope".to_string(),
            options: vec!["Current project".to_string()],
            allow_free_text: false,
            selection_mode: QuestionSelectionMode::Single,
        },
        reason: "display".to_string(),
        display_text: String::new(),
        auto_execute: false,
    }];
    record_user_questions(
        &mut state,
        &events,
        AgentRunOrigin::AutoFailure,
        Some("owner-request"),
    );
    let mut answer = ShellEvent::user_input_intercepted("session-1", "1");
    answer.component = Some("card".to_string());
    answer.message = Some("answer".to_string());
    answer.input = Some("1".to_string());

    let answer_run = agent_request_from_pending_question_answer(&answer, 2, &mut state)
        .expect("question answer run");

    assert_eq!(answer_run.origin, AgentRunOrigin::AutoFailure);
    assert_eq!(
        answer_run.provider_owner_request_id.as_deref(),
        Some("owner-request")
    );
    assert_eq!(
        respond_question_answer_to_provider(&state, &answer_run),
        ProviderQuestionResponse::OwnerUnavailable
    );
}

#[test]
fn provider_question_does_not_respond_through_an_unrelated_active_run() {
    let mut state = InlineState::default();
    let mut unrelated_event = ShellEvent::user_input_intercepted("session-1", "other task");
    unrelated_event.cwd = Some("/repo".to_string());
    let mut unrelated_request =
        agent_request_from_intercepted_input(&unrelated_event, 1, true).expect("unrelated request");
    unrelated_request.id = "different-owner".to_string();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    start_agent_run(
        &unrelated_request,
        &adapter,
        &mut state,
        &mut Vec::new(),
        None,
    )
    .expect("start unrelated run");
    let events = vec![GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::UserQuestion {
            run_id: "run-1".to_string(),
            provider_request_id: Some("provider-question-1".to_string()),
            question: "Choose a scope".to_string(),
            options: vec!["Current project".to_string()],
            allow_free_text: false,
            selection_mode: QuestionSelectionMode::Single,
        },
        reason: "display".to_string(),
        display_text: String::new(),
        auto_execute: false,
    }];
    record_user_questions(
        &mut state,
        &events,
        AgentRunOrigin::InsightPrompt,
        Some("question-owner"),
    );
    let mut answer = ShellEvent::user_input_intercepted("session-1", "1");
    answer.component = Some("card".to_string());
    answer.message = Some("answer".to_string());
    answer.input = Some("1".to_string());
    let answer_run = agent_request_from_pending_question_answer(&answer, 2, &mut state)
        .expect("question answer run");

    assert_eq!(
        respond_question_answer_to_provider(&state, &answer_run),
        ProviderQuestionResponse::OwnerUnavailable
    );
    assert_eq!(
        state
            .agent_run
            .active
            .as_ref()
            .map(|run| run.request.id.as_str()),
        Some("different-owner")
    );
}

#[test]
fn question_cancel_without_matching_owner_keeps_unrelated_active_run() {
    for owner_request_id in [None, Some("question-owner")] {
        let mut state = InlineState::default();
        let mut unrelated_event = ShellEvent::user_input_intercepted("session-1", "other task");
        unrelated_event.cwd = Some("/repo".to_string());
        let mut unrelated_request = agent_request_from_intercepted_input(&unrelated_event, 1, true)
            .expect("unrelated request");
        unrelated_request.id = "different-owner".to_string();
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        start_agent_run(
            &unrelated_request,
            &adapter,
            &mut state,
            &mut Vec::new(),
            None,
        )
        .expect("start unrelated run");
        let events = vec![GovernedEvent {
            decision: GovernanceDecision::Display,
            policy_decision: GovernancePolicyDecision::DisplayOnly,
            event: AgentEvent::UserQuestion {
                run_id: "run-1".to_string(),
                provider_request_id: Some("provider-question-1".to_string()),
                question: "Choose a scope".to_string(),
                options: vec!["Current project".to_string()],
                allow_free_text: false,
                selection_mode: QuestionSelectionMode::Single,
            },
            reason: "display".to_string(),
            display_text: String::new(),
            auto_execute: false,
        }];
        record_user_questions(
            &mut state,
            &events,
            AgentRunOrigin::InsightPrompt,
            owner_request_id,
        );
        let mut cancel = ShellEvent::user_input_intercepted("session-1", "q-1");
        cancel.component = Some("card".to_string());
        cancel.message = Some("question_cancel".to_string());

        render_question_cancel_actions(&[cancel], &mut state, &mut Vec::new(), 2)
            .expect("cancel question");

        assert_eq!(state.questions.pending_id, None);
        assert_eq!(state.questions.items[0].answer.as_deref(), Some(""));
        assert_eq!(
            state
                .agent_run
                .active
                .as_ref()
                .map(|run| run.request.id.as_str()),
            Some("different-owner"),
            "owner={owner_request_id:?}"
        );
    }
}
