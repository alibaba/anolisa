use crate::hooks::interrupt::command_should_skip_failure_analysis;
use crate::runtime::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailedCommandAnalysisTrigger {
    Auto,
    UserConfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailedCommandAgentStartOptions {
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) trigger: FailedCommandAnalysisTrigger,
}

pub(crate) fn render_failed_command_cards<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.analysis_mode != AnalysisMode::Manual {
        return Ok(());
    }

    for block in blocks.iter().filter(|block| {
        should_offer_failed_command_card(block)
            && !command_should_skip_failure_analysis(events, block)
            && !state.analyzed_blocks.contains(&block.id)
            && !state.canceled_blocks.contains(&block.id)
    }) {
        if !state.rendered_failed_command_cards.insert(block.id.clone()) {
            continue;
        }

        RatatuiInlineRenderer::for_terminal().write_notice_panel(
            output,
            NoticePanelModel {
                title: state.i18n().t(MessageId::FailedCommandCardTitle),
                body: vec![state.i18n().format(
                    MessageId::FailedCommandCardBody,
                    &[
                        ("command", block.command.as_str()),
                        ("exit_code", &block.exit_code.to_string()),
                        ("id", block.id.as_str()),
                    ],
                )],
                footer: Some(state.i18n().t(MessageId::FailedCommandCardFooter)),
            },
        )?;
        output.flush()?;
    }

    Ok(())
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

        let key = format!("details-{event_index}");
        if event_requests_failed_command_details(event) && state.handled_confirmations.insert(key) {
            let Some(block) = failed_command_card_details_target(blocks, event) else {
                continue;
            };
            render_runtime_details(state, blocks, &block.id, output)?;
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
        should_analyze_failed_block(block, state.analysis_mode)
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
    if is_failed_command_card_event(event) {
        return failed_command_card_target(blocks, state, event);
    }
    latest_pending_failed_block_before_event(blocks, state, event)
}

fn failed_command_card_target<'a>(
    blocks: &'a [CommandBlock],
    state: &InlineState,
    event: &ShellEvent,
) -> Option<&'a CommandBlock> {
    if !is_failed_command_card_event(event) {
        return None;
    }

    let id = event.input.as_deref()?.trim();
    blocks.iter().find(|block| {
        block.id == id
            && should_offer_failed_command_card(block)
            && !state.analyzed_blocks.contains(&block.id)
            && !state.canceled_blocks.contains(&block.id)
    })
}

fn failed_command_card_details_target<'a>(
    blocks: &'a [CommandBlock],
    event: &ShellEvent,
) -> Option<&'a CommandBlock> {
    if !is_failed_command_card_event(event) {
        return None;
    }
    let id = event.input.as_deref()?.trim();
    blocks
        .iter()
        .find(|block| block.id == id && should_offer_failed_command_card(block))
}

fn is_failed_command_card_event(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && event.component.as_deref() == Some("card")
        && matches!(
            event.message.as_deref(),
            Some("failed_command_analyze" | "failed_command_dismiss" | "failed_command_details")
        )
}

pub(crate) fn should_analyze_failed_block(block: &CommandBlock, mode: AnalysisMode) -> bool {
    if block.exit_code == 0 || block.command.trim().is_empty() {
        return false;
    }
    if mode == AnalysisMode::Manual {
        return false;
    }
    let category = classify_exit(block.exit_code, &block.command);
    match category {
        ExitCodeCategory::Success
        | ExitCodeCategory::UserInterrupt
        | ExitCodeCategory::PipelineNormal => false,
        ExitCodeCategory::CommandSpecificNormal => mode == AnalysisMode::Auto,
        _ => true,
    }
}

fn should_offer_failed_command_card(block: &CommandBlock) -> bool {
    should_analyze_failed_block(block, AnalysisMode::Auto)
}

fn event_requests_failed_command_details(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && event.component.as_deref() == Some("card")
        && event.message.as_deref() == Some("failed_command_details")
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
        FailedCommandAnalysisTrigger::Auto => {
            should_analyze_failed_block(block, state.analysis_mode)
        }
        FailedCommandAnalysisTrigger::UserConfirmed => should_offer_failed_command_card(block),
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

    match agent_request_after_confirmation(&block.session_id, block, findings, true) {
        Some(mut request) => {
            let ctx_config = RelatedHistoryConfig::default();
            let ctx_entries = build_related_history_index(blocks, block, &ctx_config);
            request.context_blocks = context_blocks_from_entries(&ctx_entries);
            request.context_hints = hook_routing_hints_for_block(state, block);
            if options.trigger == FailedCommandAnalysisTrigger::Auto
                && !request.context_hints.is_empty()
                && state.agent_run.active.is_none()
            {
                RatatuiInlineRenderer::for_terminal().write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: state.i18n().t(MessageId::HookAutoAnalyzedTitle),
                        body: vec![state.i18n().format(
                            MessageId::HookAutoAnalyzedBody,
                            &[
                                ("command", block.command.as_str()),
                                ("exit_code", &block.exit_code.to_string()),
                            ],
                        )],
                        footer: Some(state.i18n().t(MessageId::HookAutoAnalyzedFooter)),
                    },
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
            start_agent_run(
                &request,
                adapter,
                state,
                output,
                options.selectable_after_event_index,
            )
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    fn card_event(message: &str, input: Option<&str>) -> ShellEvent {
        ShellEvent {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: "session-1".to_string(),
            command_id: None,
            command: None,
            cwd: Some("/tmp".to_string()),
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(3),
            ended_at_ms: Some(3),
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: input.map(str::to_string),
            component: Some("card".to_string()),
            message: Some(message.to_string()),
            command_origin: Some(CommandOrigin::UserInteractive),
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
        let block = failed_block(2, "ls --bad-flag");

        assert!(should_analyze_failed_block(&block, AnalysisMode::Auto));
        assert!(should_analyze_failed_block(&block, AnalysisMode::Smart));
        assert!(!should_analyze_failed_block(&block, AnalysisMode::Manual));
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
        let mut target = failed_block(2, "ls --bad-context");
        target.id = "target".to_string();
        target.cwd = "/repo".to_string();
        target.end_cwd = "/repo".to_string();
        target.ended_at_ms = 30;
        target.output.terminal_output_ref = Some("/tmp/target-output.txt".to_string());
        let blocks = vec![setup.clone(), previous_failed.clone(), target.clone()];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState::default();
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

        let request = &state.agent_run.active.as_ref().expect("active run").request;
        let ids = request
            .context_blocks
            .iter()
            .map(|block| block.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["setup", "previous-failed"]);
        assert!(request
            .context_blocks
            .iter()
            .all(|block| block.id != target.id));
    }

    #[test]
    fn manual_mode_renders_failed_command_action_card() {
        let mut block = failed_block(2, "ls --bad-flag");
        block.id = "target".to_string();
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();

        render_failed_command_cards(&[], &[block], &mut state, &mut output)
            .expect("render failed command card");

        let rendered = String::from_utf8(output).expect("utf8");
        assert!(rendered.contains("Command failed"), "{rendered}");
        assert!(rendered.contains("ls --bad-flag"), "{rendered}");
        assert!(
            rendered.contains("[Analyze] [Dismiss] [Details]"),
            "{rendered}"
        );
        assert!(!rendered.contains("/explain"), "{rendered}");
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

        render_failed_command_cards(&[ctrl_c], &[block], &mut state, &mut output)
            .expect("render failed command card");

        assert!(output.is_empty());
    }

    #[test]
    fn card_analyze_starts_agent_even_in_manual_mode() {
        let mut target = failed_block(2, "ls --bad-flag");
        target.id = "target".to_string();
        let blocks = vec![target];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();
        let events = vec![card_event("failed_command_analyze", Some("target"))];

        render_post_failure_actions(
            &events,
            &blocks,
            &findings,
            &adapter,
            &mut state,
            &mut output,
            0,
        )
        .expect("handle card analyze");

        assert!(state.agent_run.active.is_some());
        assert!(state.analyzed_blocks.contains("target"));
    }

    #[test]
    fn card_analyze_requires_explicit_matching_command_id() {
        let mut target = failed_block(2, "ls --bad-flag");
        target.id = "target".to_string();
        let blocks = vec![target];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();
        let events = vec![card_event("failed_command_analyze", None)];

        render_post_failure_actions(
            &events,
            &blocks,
            &findings,
            &adapter,
            &mut state,
            &mut output,
            0,
        )
        .expect("handle card analyze");

        assert!(state.agent_run.active.is_none());
        assert!(!state.analyzed_blocks.contains("target"));
    }

    #[test]
    fn card_dismiss_cancels_failed_command_analysis() {
        let mut target = failed_block(2, "ls --bad-flag");
        target.id = "target".to_string();
        let blocks = vec![target];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();
        let events = vec![card_event("failed_command_dismiss", Some("target"))];

        render_post_failure_actions(
            &events,
            &blocks,
            &findings,
            &adapter,
            &mut state,
            &mut output,
            0,
        )
        .expect("handle card dismiss");

        let rendered = String::from_utf8(output).expect("utf8");
        assert!(state.canceled_blocks.contains("target"));
        assert!(rendered.contains("Agent cancelled"), "{rendered}");
    }

    #[test]
    fn card_details_renders_command_details() {
        let mut target = failed_block(2, "ls --bad-flag");
        target.id = "target".to_string();
        let blocks = vec![target];
        let findings = findings_from_blocks(&blocks);
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState {
            analysis_mode: AnalysisMode::Manual,
            ..InlineState::default()
        };
        let mut output = Vec::new();
        let events = vec![card_event("failed_command_details", Some("target"))];

        render_post_failure_actions(
            &events,
            &blocks,
            &findings,
            &adapter,
            &mut state,
            &mut output,
            0,
        )
        .expect("handle card details");

        let rendered = String::from_utf8(output).expect("utf8");
        assert!(rendered.contains("Command details"), "{rendered}");
        assert!(rendered.contains("command_id: target"), "{rendered}");
    }
}
