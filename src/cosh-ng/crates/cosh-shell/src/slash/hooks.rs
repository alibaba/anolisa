use crate::runtime::prelude::*;
use crate::runtime::state::{HookFeedback, RuntimeHookDisplayEvent, RuntimeHookFinding};
use crate::slash::panel::render_notice_panel;
use cosh_shell::hook_types::FindingSeverity;

pub(crate) fn render_hooks_command<W: Write>(
    sub: Option<&str>,
    arg: Option<&str>,
    extra: Option<&str>,
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    match (sub, arg, extra) {
        (None, _, _) => {
            let i18n = state.i18n();
            let hooks = state.hooks.engine.registered_hook_infos();
            let body = hooks_status_body(state, &hooks, &i18n);
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksRegisteredTitle),
                body,
                Some(&hooks_footer(state, hooks.len(), &i18n)),
            )
        }
        (Some("history"), None, None) => render_hooks_history(state, output),
        (Some("events"), None, None) => render_hooks_events(state, output),
        (Some("analyze"), Some(hint_id), None)
        | (Some("ignore"), Some(hint_id), None)
        | (Some("details"), Some(hint_id), None) => {
            crate::hooks::slash::handle_command_hook_hint_action(
                sub.unwrap(),
                hint_id,
                blocks,
                adapter,
                state,
                output,
            )
        }
        (Some("feedback"), Some(action), Some(finding_id)) => {
            render_hooks_feedback(action, finding_id, state, output)
        }
        (Some("clear-feedback"), None, None) => render_clear_hook_feedback(state, output),
        (Some("mute"), Some(target), None) => {
            state.hooks.muted_targets.insert(target.to_string());
            let i18n = state.i18n();
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksTargetMutedTitle),
                vec![i18n.format(
                    cosh_shell::MessageId::SlashHooksTargetMutedBody,
                    &[("target", target)],
                )],
                Some(i18n.t(cosh_shell::MessageId::SlashHooksTargetMutedFooter)),
            )
        }
        (Some("unmute"), Some(target), None) => {
            let removed = state.hooks.muted_targets.remove(target);
            let i18n = state.i18n();
            let body = if removed {
                vec![i18n.format(
                    cosh_shell::MessageId::SlashHooksTargetUnmutedBody,
                    &[("target", target)],
                )]
            } else {
                vec![i18n.format(
                    cosh_shell::MessageId::SlashHooksTargetNotMutedBody,
                    &[("target", target)],
                )]
            };
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksTargetUnmutedTitle),
                body,
                None,
            )
        }
        (Some("trust-project"), None, None) => render_project_hook_trust(true, state, output),
        (Some("untrust-project"), None, None) => render_project_hook_trust(false, state, output),
        (Some("clear-project-trust"), None, None) => render_clear_project_hook_trust(state, output),
        (Some("enable"), Some(id), None) => {
            state.hooks.disabled.remove(id);
            let i18n = state.i18n();
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksEnabledTitle),
                vec![i18n.format(cosh_shell::MessageId::SlashHooksEnabledBody, &[("id", id)])],
                None,
            )
        }
        (Some("disable"), Some(id), None) => {
            state.hooks.disabled.insert(id.to_string());
            let i18n = state.i18n();
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksDisabledTitle),
                vec![i18n.format(cosh_shell::MessageId::SlashHooksDisabledBody, &[("id", id)])],
                None,
            )
        }
        _ => {
            let i18n = state.i18n();
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::SlashHooksUsageTitle),
                hooks_usage_body(&i18n),
                None,
            )
        }
    }
}

fn hooks_status_body(
    state: &InlineState,
    hooks: &[cosh_shell::hook_engine::RegisteredHookInfo],
    i18n: &cosh_shell::I18n,
) -> Vec<String> {
    if hooks.is_empty() {
        return vec![i18n
            .t(cosh_shell::MessageId::SlashHooksNoHooksBody)
            .to_string()];
    }

    let total = hooks.len();
    let disabled = hooks
        .iter()
        .filter(|hook| state.hooks.disabled.contains(hook.id.as_str()))
        .count();
    let enabled = total.saturating_sub(disabled);
    let builtin = hooks
        .iter()
        .filter(|hook| {
            matches!(
                hook.source,
                cosh_shell::hook_engine::HookSourceInfo::Builtin
            )
        })
        .count();
    let user = hooks
        .iter()
        .filter(|hook| {
            matches!(
                hook.source,
                cosh_shell::hook_engine::HookSourceInfo::ExternalUser
            )
        })
        .count();
    let project = hooks
        .iter()
        .filter(|hook| {
            matches!(
                hook.source,
                cosh_shell::hook_engine::HookSourceInfo::ExternalProject
            )
        })
        .count();
    let trusted = hooks
        .iter()
        .filter(|hook| {
            matches!(
                hook.source,
                cosh_shell::hook_engine::HookSourceInfo::ExternalProject
            ) && hook.trusted == Some(true)
        })
        .count();
    let untrusted = project.saturating_sub(trusted);

    let total = total.to_string();
    let enabled = enabled.to_string();
    let disabled = disabled.to_string();
    let builtin = builtin.to_string();
    let user = user.to_string();
    let project = project.to_string();
    let trusted = trusted.to_string();
    let untrusted = untrusted.to_string();
    let mut body = vec![
        i18n.format(
            cosh_shell::MessageId::SlashHooksStatusCountLine,
            &[
                ("total", &total),
                ("enabled", &enabled),
                ("disabled", &disabled),
            ],
        ),
        i18n.format(
            cosh_shell::MessageId::SlashHooksStatusSourcesLine,
            &[
                ("builtin", &builtin),
                ("user", &user),
                ("project", &project),
            ],
        ),
    ];
    if project != "0" {
        body.push(i18n.format(
            cosh_shell::MessageId::SlashHooksStatusProjectTrustLine,
            &[("trusted", &trusted), ("untrusted", &untrusted)],
        ));
    }
    body
}

fn hooks_usage_body(i18n: &cosh_shell::I18n) -> Vec<String> {
    [cosh_shell::MessageId::SlashHooksUsageListLine]
        .into_iter()
        .map(|id| i18n.t(id).to_string())
        .collect()
}

fn render_project_hook_trust<W: Write>(
    trusted: bool,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let project_roots = registered_project_hook_roots(state);
    let updated = state.hooks.engine.set_project_hooks_trusted(trusted);
    let i18n = state.i18n();
    let count = updated.to_string();
    let (title, updated_body) = if trusted {
        (
            i18n.t(cosh_shell::MessageId::SlashHooksProjectTrustedTitle),
            i18n.format(
                cosh_shell::MessageId::SlashHooksProjectTrustedBody,
                &[("count", &count)],
            ),
        )
    } else {
        (
            i18n.t(cosh_shell::MessageId::SlashHooksProjectUntrustedTitle),
            i18n.format(
                cosh_shell::MessageId::SlashHooksProjectUntrustedBody,
                &[("count", &count)],
            ),
        )
    };
    let body = if updated == 0 {
        vec![i18n
            .t(cosh_shell::MessageId::SlashHooksProjectTrustNoHooksBody)
            .to_string()]
    } else {
        vec![updated_body]
    };
    let footer = if updated == 0 {
        i18n.t(cosh_shell::MessageId::SlashHooksProjectTrustNoChangeFooter)
            .to_string()
    } else {
        persist_project_hook_trust(trusted, &project_roots, &i18n)
    };
    render_notice_panel(output, title, body, Some(&footer))
}

fn render_clear_project_hook_trust<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let updated = state.hooks.engine.set_project_hooks_trusted(false);
    let i18n = state.i18n();
    let footer = match cosh_shell::config::clear_project_trust_store() {
        Ok(()) => i18n
            .t(cosh_shell::MessageId::SlashHooksProjectTrustClearedFooter)
            .to_string(),
        Err(err) => {
            let err = err.to_string();
            i18n.format(
                cosh_shell::MessageId::SlashHooksProjectTrustClearFailedFooter,
                &[("error", &err)],
            )
        }
    };
    let count = updated.to_string();
    render_notice_panel(
        output,
        i18n.t(cosh_shell::MessageId::SlashHooksProjectTrustClearedTitle),
        vec![i18n.format(
            cosh_shell::MessageId::SlashHooksProjectTrustClearedBody,
            &[("count", &count)],
        )],
        Some(&footer),
    )
}

fn registered_project_hook_roots(state: &InlineState) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    for hook in state.hooks.engine.registered_hook_infos() {
        if hook.source != cosh_shell::hook_engine::HookSourceInfo::ExternalProject {
            continue;
        }
        let Some(root) = hook.project_root else {
            continue;
        };
        if !roots.iter().any(|existing| existing == &root) {
            roots.push(root);
        }
    }
    roots
}

fn persist_project_hook_trust(
    trusted: bool,
    roots: &[std::path::PathBuf],
    i18n: &cosh_shell::I18n,
) -> String {
    let mut failures = Vec::new();
    for root in roots {
        let result = if trusted {
            cosh_shell::config::trust_project_root(root)
        } else {
            cosh_shell::config::untrust_project_root(root)
        };
        if let Err(err) = result {
            failures.push(format!("{}: {err}", root.display()));
        }
    }
    if failures.is_empty() {
        if trusted {
            i18n.t(cosh_shell::MessageId::SlashHooksProjectTrustPersistedFooter)
                .to_string()
        } else {
            i18n.t(cosh_shell::MessageId::SlashHooksProjectTrustRemovedFooter)
                .to_string()
        }
    } else {
        let failures = failures.join("; ");
        i18n.format(
            cosh_shell::MessageId::SlashHooksProjectTrustPersistenceFailedFooter,
            &[("failures", &failures)],
        )
    }
}

fn hooks_footer(state: &InlineState, hook_count: usize, i18n: &cosh_shell::I18n) -> String {
    let count = hook_count.to_string();
    if state.hooks.muted_targets.is_empty() {
        return i18n.format(
            cosh_shell::MessageId::SlashHooksFooterCount,
            &[("count", &count)],
        );
    }
    let mut muted = state
        .hooks
        .muted_targets
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    muted.sort();
    let targets = muted.join(", ");
    i18n.format(
        cosh_shell::MessageId::SlashHooksFooterMutedTargets,
        &[("count", &count), ("targets", &targets)],
    )
}

fn render_hooks_feedback<W: Write>(
    action: &str,
    finding_id: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(feedback) = parse_hook_feedback(action) else {
        let i18n = state.i18n();
        return render_notice_panel(
            output,
            i18n.t(cosh_shell::MessageId::SlashHooksUsageTitle),
            vec![i18n
                .t(cosh_shell::MessageId::SlashHooksFeedbackUsageBody)
                .to_string()],
            None,
        );
    };
    let Some(hint) = state
        .hooks
        .findings
        .iter()
        .find(|hint| hint.id == finding_id)
    else {
        let i18n = state.i18n();
        return render_notice_panel(
            output,
            i18n.t(cosh_shell::MessageId::SlashHooksFeedbackTitle),
            vec![i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackFindingNotFoundBody,
                &[("finding_id", finding_id)],
            )],
            Some(i18n.t(cosh_shell::MessageId::SlashHooksFeedbackFindingNotFoundFooter)),
        );
    };
    let suppression_key = hint.suppression_key.clone();
    let hook_id = hint
        .hook_finding
        .as_ref()
        .map(|finding| finding.hook_id.clone())
        .unwrap_or_else(|| "unknown".to_string());

    if feedback == HookFeedback::Useful {
        state.hooks.ignored_cards.remove(&suppression_key);
    }
    state
        .hooks
        .feedback
        .insert(suppression_key.clone(), feedback);
    let i18n = state.i18n();
    let footer = match cosh_shell::config::record_hook_feedback_preference(
        hook_feedback_preference(hint, feedback),
    ) {
        Ok(()) => i18n
            .t(cosh_shell::MessageId::SlashHooksFeedbackPersistedFooter)
            .to_string(),
        Err(err) => {
            let err = err.to_string();
            i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackPersistenceFailedFooter,
                &[("error", &err)],
            )
        }
    };

    render_notice_panel(
        output,
        i18n.t(cosh_shell::MessageId::SlashHooksFeedbackRecordedTitle),
        vec![
            i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackRecordedBody,
                &[("feedback", feedback.label()), ("finding_id", finding_id)],
            ),
            i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackHookLine,
                &[("hook_id", &hook_id)],
            ),
            i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackPolicyKeyLine,
                &[("key", &suppression_key)],
            ),
        ],
        Some(&footer),
    )
}

fn hook_feedback_preference(
    hint: &RuntimeHookFinding,
    feedback: HookFeedback,
) -> cosh_shell::config::HookFeedbackPreference {
    cosh_shell::config::HookFeedbackPreference {
        suppression_key: hint.suppression_key.clone(),
        label: feedback.label().to_string(),
        topic: hint.topic.clone(),
        entity_key: hint.entity_key.clone(),
        severity: hook_severity_label(hint.effective_severity).to_string(),
        command_intent: command_intent_from_suppression_key(&hint.suppression_key).to_string(),
        action: feedback.label().to_string(),
        recorded_at_ms: current_epoch_ms(),
        window_ms: 10 * 60 * 1000,
    }
}

fn command_intent_from_suppression_key(suppression_key: &str) -> &str {
    suppression_key.rsplit(':').next().unwrap_or("unknown")
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn render_clear_hook_feedback<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let cleared = state.hooks.feedback.len();
    state.hooks.feedback.clear();
    state.hooks.ignored_cards.clear();
    let i18n = state.i18n();
    let footer = match cosh_shell::config::clear_hook_feedback_store() {
        Ok(()) => i18n
            .t(cosh_shell::MessageId::SlashHooksFeedbackClearedFooter)
            .to_string(),
        Err(err) => {
            let err = err.to_string();
            i18n.format(
                cosh_shell::MessageId::SlashHooksFeedbackClearFailedFooter,
                &[("error", &err)],
            )
        }
    };
    let count = cleared.to_string();
    render_notice_panel(
        output,
        i18n.t(cosh_shell::MessageId::SlashHooksFeedbackClearedTitle),
        vec![i18n.format(
            cosh_shell::MessageId::SlashHooksFeedbackClearedBody,
            &[("count", &count)],
        )],
        Some(&footer),
    )
}

fn parse_hook_feedback(action: &str) -> Option<HookFeedback> {
    match action {
        "noisy" => Some(HookFeedback::Noisy),
        "useful" => Some(HookFeedback::Useful),
        _ => None,
    }
}

fn render_hooks_history<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<()> {
    let i18n = state.i18n();
    let body = if state.hooks.findings.is_empty() {
        vec![i18n
            .t(cosh_shell::MessageId::SlashHooksHistoryEmptyBody)
            .to_string()]
    } else {
        state
            .hooks
            .findings
            .iter()
            .rev()
            .take(10)
            .map(hook_history_line)
            .collect()
    };
    render_notice_panel(
        output,
        i18n.t(cosh_shell::MessageId::SlashHooksHistoryTitle),
        body,
        Some(i18n.t(cosh_shell::MessageId::SlashHooksHistoryFooter)),
    )
}

fn render_hooks_events<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<()> {
    let i18n = state.i18n();
    let body = if state.hooks.display_events.is_empty() {
        vec![i18n
            .t(cosh_shell::MessageId::SlashHooksEventsEmptyBody)
            .to_string()]
    } else {
        state
            .hooks
            .display_events
            .iter()
            .rev()
            .take(10)
            .map(hook_event_line)
            .collect()
    };
    render_notice_panel(
        output,
        i18n.t(cosh_shell::MessageId::SlashHooksEventsTitle),
        body,
        Some(i18n.t(cosh_shell::MessageId::SlashHooksEventsFooter)),
    )
}

fn hook_event_line(event: &RuntimeHookDisplayEvent) -> String {
    format!(
        "action={} id={} hook={} display={} reason={} topic={} entity={} confidence={} ended_at_ms={} block={} suppression_key={}",
        event.action.label(),
        event.finding_id,
        event.hook_id,
        event.display.label(),
        event.display_reason,
        event.topic,
        event.entity_key,
        event.confidence,
        event.ended_at_ms,
        event.command_block_id,
        event.suppression_key
    )
}

fn hook_history_line(hint: &RuntimeHookFinding) -> String {
    let hook_id = hint
        .hook_finding
        .as_ref()
        .map(|finding| finding.hook_id.as_str())
        .unwrap_or("unknown");
    format!(
        "id={} {} [{}] display={} reason={} topic={} entity={} block={} command={}",
        hint.id,
        hook_id,
        hook_severity_label(hint.effective_severity),
        hint.display.label(),
        hint.display_reason,
        hint.topic,
        hint.entity_key,
        hint.command_block_id,
        hint.command.trim()
    )
}

fn hook_severity_label(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::Info => "info",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state::RuntimeHookDisplay;
    use cosh_shell::hook_types::HookFinding;

    struct EnvLock {
        path: std::path::PathBuf,
    }

    impl Drop for EnvLock {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn zh_state() -> InlineState {
        InlineState {
            language: cosh_shell::Language::ZhCn,
            ..InlineState::default()
        }
    }

    fn env_lock() -> EnvLock {
        let path =
            std::env::temp_dir().join(format!("cosh-shell-test-env-{}.lock", std::process::id()));
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return EnvLock { path },
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(err) => panic!("create env test lock failed: {err}"),
            }
        }
    }

    fn register_project_hook(state: &mut InlineState) {
        state
            .hooks
            .engine
            .register_external(cosh_shell::hook_engine::ExternalHookConfig {
                path: std::path::PathBuf::from("/tmp/project/.cosh/hooks/project.sh"),
                matcher: cosh_shell::hook_types::HookMatcher {
                    id: "project-hook".to_string(),
                    commands: vec!["echo".to_string()],
                    command_patterns: Vec::new(),
                    command_regex: None,
                    min_output_bytes: None,
                    exit_codes: None,
                    trigger: cosh_shell::hook_types::HookTrigger::OnComplete,
                },
                timeout_ms: 1000,
                source: cosh_shell::hook_engine::ExternalHookSource::Project,
                project_root: Some(std::path::PathBuf::from("/tmp/project")),
                trusted: false,
            });
    }

    fn hook_finding() -> HookFinding {
        HookFinding {
            hook_id: "memory-pressure".to_string(),
            severity: FindingSeverity::Critical,
            title: "Available memory is low".to_string(),
            description: "description".to_string(),
            suggestion: "suggestion".to_string(),
            skill: Some("memory-analysis".to_string()),
            cli_hint: Some("free -m".to_string()),
            context_refs: Vec::new(),
        }
    }

    fn hook_hint() -> RuntimeHookFinding {
        RuntimeHookFinding {
            id: "hook-cmd-1-memory-pressure".to_string(),
            command_block_id: "cmd-1".to_string(),
            command: "free -m".to_string(),
            output_ref: Some("/tmp/out".to_string()),
            ended_at_ms: 200,
            prompt_hint: "hook_finding=memory-pressure".to_string(),
            finding_markdown: None,
            hook_finding: Some(hook_finding()),
            recommended_skill: Some("memory-analysis".to_string()),
            display: RuntimeHookDisplay::Hint,
            display_reason: "allowed".to_string(),
            related_hook_ids: Vec::new(),
            topic: "memory".to_string(),
            entity_key: "system-memory".to_string(),
            effective_severity: FindingSeverity::Critical,
            confidence: "high".to_string(),
            suppression_key: "memory:memory-pressure:free".to_string(),
        }
    }

    fn render_hooks_test_command(
        sub: Option<&str>,
        arg: Option<&str>,
        extra: Option<&str>,
        state: &mut InlineState,
    ) -> String {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut output = Vec::new();
        render_hooks_command(sub, arg, extra, &[], &adapter, state, &mut output)
            .expect("render hooks command");
        String::from_utf8(output).expect("utf8 output")
    }

    #[test]
    fn hooks_empty_list_uses_zh_catalog_text() {
        let mut state = zh_state();

        let output = render_hooks_test_command(None, None, None, &mut state);

        assert!(output.contains("Hook 状态"), "{output}");
        assert!(output.contains("未注册 Hook。"), "{output}");
        assert!(output.contains("已注册 0 个 Hook。"), "{output}");
        assert!(!output.contains("Hook status"), "{output}");
        assert!(!output.contains("No hooks registered"), "{output}");
    }

    #[test]
    fn hooks_session_actions_use_zh_catalog_text() {
        let mut state = zh_state();

        let muted = render_hooks_test_command(Some("mute"), Some("memory"), None, &mut state);
        assert!(muted.contains("Hook 目标已静音"), "{muted}");
        assert!(
            muted.contains("本会话已静音 Hook 目标 'memory'。"),
            "{muted}"
        );
        assert!(!muted.contains("Hook target muted"), "{muted}");

        let unmuted = render_hooks_test_command(Some("unmute"), Some("memory"), None, &mut state);
        assert!(unmuted.contains("Hook 目标已取消静音"), "{unmuted}");
        assert!(
            unmuted.contains("已取消静音 Hook 目标 'memory'。"),
            "{unmuted}"
        );
        assert!(!unmuted.contains("Hook target unmuted"), "{unmuted}");

        let enabled =
            render_hooks_test_command(Some("enable"), Some("linux-memory"), None, &mut state);
        assert!(enabled.contains("Hook 已启用"), "{enabled}");
        assert!(
            enabled.contains("Hook 'linux-memory' 已启用。"),
            "{enabled}"
        );
        assert!(!enabled.contains("Hook enabled"), "{enabled}");

        let disabled =
            render_hooks_test_command(Some("disable"), Some("linux-memory"), None, &mut state);
        assert!(disabled.contains("Hook 已禁用"), "{disabled}");
        assert!(
            disabled.contains("Hook 'linux-memory' 已禁用。"),
            "{disabled}"
        );
        assert!(!disabled.contains("Hook disabled"), "{disabled}");
    }

    #[test]
    fn hooks_history_and_events_empty_state_use_zh_catalog_text() {
        let mut state = zh_state();

        let history = render_hooks_test_command(Some("history"), None, None, &mut state);
        assert!(history.contains("Hook 历史"), "{history}");
        assert!(history.contains("本会话未记录 Hook finding。"), "{history}");
        assert!(!history.contains("No hook findings recorded"), "{history}");

        let events = render_hooks_test_command(Some("events"), None, None, &mut state);
        assert!(events.contains("Hook 显示事件"), "{events}");
        assert!(events.contains("本会话未记录 Hook 显示事件。"), "{events}");
        assert!(
            !events.contains("No hook display events recorded"),
            "{events}"
        );
    }

    #[test]
    fn hooks_usage_uses_zh_catalog_text() {
        let mut state = zh_state();

        let output = render_hooks_test_command(Some("bogus"), None, None, &mut state);

        assert!(output.contains("用法"), "{output}");
        assert!(
            output.contains("/hooks                - 显示 Hook 状态"),
            "{output}"
        );
        assert!(!output.contains("/hooks clear-project-trust"), "{output}");
        assert!(!output.contains("/hooks feedback"), "{output}");
        assert!(!output.contains("/hooks analyze"), "{output}");
        assert!(!output.contains("show hook status"), "{output}");
        assert!(
            !output.contains("clear project hook trust store"),
            "{output}"
        );
    }

    #[test]
    fn hooks_project_trust_empty_state_uses_zh_catalog_text() {
        let mut state = zh_state();

        let output = render_hooks_test_command(Some("trust-project"), None, None, &mut state);

        assert!(output.contains("项目 Hook 已信任"), "{output}");
        assert!(output.contains("本会话未注册项目 Hook。"), "{output}");
        assert!(output.contains("信任状态未变更。"), "{output}");
        assert!(!output.contains("Project hooks trusted"), "{output}");
        assert!(
            !output.contains("No project hooks are registered"),
            "{output}"
        );
    }

    #[test]
    fn hooks_feedback_errors_use_zh_catalog_text() {
        let mut state = zh_state();

        let usage =
            render_hooks_test_command(Some("feedback"), Some("bad"), Some("id-1"), &mut state);
        assert!(usage.contains("用法"), "{usage}");
        assert!(
            usage.contains("/hooks feedback noisy|useful <finding_id>"),
            "{usage}"
        );

        let missing =
            render_hooks_test_command(Some("feedback"), Some("noisy"), Some("missing"), &mut state);
        assert!(missing.contains("Hook 反馈"), "{missing}");
        assert!(
            missing.contains("本会话未找到 finding 'missing'。"),
            "{missing}"
        );
        assert!(
            missing.contains("使用 /hooks history 复制最近的 finding id。"),
            "{missing}"
        );
        assert!(
            !missing.contains("Finding 'missing' was not found"),
            "{missing}"
        );
    }

    #[test]
    fn hooks_project_trust_uses_zh_catalog_text() {
        let _env_lock = env_lock();
        let store = std::env::temp_dir().join(format!(
            "cosh-shell-slash-hooks-trust-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&store);
        std::env::set_var("COSH_SHELL_PROJECT_TRUST_STORE", &store);
        let mut state = zh_state();
        register_project_hook(&mut state);

        let trusted = render_hooks_test_command(Some("trust-project"), None, None, &mut state);
        assert!(trusted.contains("项目 Hook 已信任"), "{trusted}");
        assert!(
            trusted.contains("已将 1 个项目 Hook 标记为 trusted。"),
            "{trusted}"
        );
        assert!(
            trusted.contains("信任已持久化；已禁用 Hook 保持禁用。"),
            "{trusted}"
        );
        assert!(!trusted.contains("Project hooks trusted"), "{trusted}");

        let cleared =
            render_hooks_test_command(Some("clear-project-trust"), None, None, &mut state);
        assert!(cleared.contains("项目 Hook 信任已清除"), "{cleared}");
        assert!(
            cleared.contains("已将 1 个项目 Hook 标记为 untrusted。"),
            "{cleared}"
        );
        assert!(cleared.contains("项目 Hook 信任存储已清除"), "{cleared}");
        assert!(!cleared.contains("Project hook trust cleared"), "{cleared}");

        std::env::remove_var("COSH_SHELL_PROJECT_TRUST_STORE");
        let _ = std::fs::remove_file(&store);
    }

    #[test]
    fn hooks_feedback_uses_zh_catalog_text() {
        let _env_lock = env_lock();
        let store = std::env::temp_dir().join(format!(
            "cosh-shell-slash-hooks-feedback-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&store);
        std::env::set_var("COSH_SHELL_HOOK_FEEDBACK_STORE", &store);
        let mut state = zh_state();

        let usage = render_hooks_test_command(
            Some("feedback"),
            Some("bad"),
            Some("finding-id"),
            &mut state,
        );
        assert!(usage.contains("用法"), "{usage}");
        assert!(
            usage.contains("/hooks feedback noisy|useful <finding_id>"),
            "{usage}"
        );

        let missing = render_hooks_test_command(
            Some("feedback"),
            Some("noisy"),
            Some("missing-id"),
            &mut state,
        );
        assert!(missing.contains("Hook 反馈"), "{missing}");
        assert!(
            missing.contains("本会话未找到 finding 'missing-id'。"),
            "{missing}"
        );
        assert!(
            !missing.contains("Finding 'missing-id' was not found"),
            "{missing}"
        );

        state.hooks.findings.push(hook_hint());
        let recorded = render_hooks_test_command(
            Some("feedback"),
            Some("noisy"),
            Some("hook-cmd-1-memory-pressure"),
            &mut state,
        );
        assert!(recorded.contains("Hook 反馈已记录"), "{recorded}");
        assert!(
            recorded.contains("已为 finding 'hook-cmd-1-memory-pressure' 记录反馈 'noisy'。"),
            "{recorded}"
        );
        assert!(
            recorded.contains("反馈已持久化，仅影响展示策略。"),
            "{recorded}"
        );
        assert!(!recorded.contains("Hook feedback recorded"), "{recorded}");

        let cleared = render_hooks_test_command(Some("clear-feedback"), None, None, &mut state);
        assert!(cleared.contains("Hook 反馈已清除"), "{cleared}");
        assert!(
            cleared.contains("已从本会话清除 1 条反馈偏好。"),
            "{cleared}"
        );
        assert!(cleared.contains("Hook 反馈偏好已清除。"), "{cleared}");
        assert!(!cleared.contains("Hook feedback cleared"), "{cleared}");

        std::env::remove_var("COSH_SHELL_HOOK_FEEDBACK_STORE");
        let _ = std::fs::remove_file(&store);
    }
}
