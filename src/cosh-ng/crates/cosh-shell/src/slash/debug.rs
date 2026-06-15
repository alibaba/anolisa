use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

pub(super) fn render_debug_command<W: Write>(
    sub: Option<&str>,
    adapter: &AdapterInstance,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    match sub {
        Some("session") | None => {
            let i18n = state.i18n();
            let debug_line = |id, value: String| i18n.format(id, &[("value", value.as_str())]);
            let mut body = vec![
                debug_line(
                    cosh_shell::MessageId::DebugAdapterLine,
                    adapter.name().to_string(),
                ),
                debug_line(
                    cosh_shell::MessageId::DebugProviderInvocationLine,
                    adapter
                        .provider_invocation()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                debug_line(
                    cosh_shell::MessageId::DebugProviderCommittedSessionLine,
                    adapter
                        .committed_session_id()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                debug_line(
                    cosh_shell::MessageId::DebugActiveRunLine,
                    state.agent_run.active.is_some().to_string(),
                ),
                debug_line(
                    cosh_shell::MessageId::DebugQueuedRunsLine,
                    state.agent_run.queued_requests.len().to_string(),
                ),
            ];
            if let Some(active_run) = state.agent_run.active.as_ref() {
                let capabilities = active_run.handle.control_capabilities();
                body.push(debug_line(
                    cosh_shell::MessageId::DebugProviderPendingSessionLine,
                    active_run
                        .handle
                        .pending_provider_session_id()
                        .unwrap_or_else(|| "<none>".to_string()),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugProviderInitializeSeenLine,
                    capabilities.provider_initialize_seen.to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugHostExecutedShellResultLine,
                    capabilities
                        .can_handle_host_executed_shell_tool_result
                        .to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugSelectedShellExecutionPathLine,
                    if capabilities.can_handle_host_executed_shell_tool_result {
                        "control_protocol_host_executed_shell_result"
                    } else if adapter.capabilities().control_protocol {
                        "provider_native_shell_tool_execution"
                    } else {
                        "unsupported"
                    }
                    .to_string(),
                ));
            } else {
                let latest_shell = state.evidence.latest_shell_command_completed();
                body.push(debug_line(
                    cosh_shell::MessageId::DebugProviderPendingSessionLine,
                    "<none>".to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugProviderInitializeSeenLine,
                    "<none>".to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugHostExecutedShellResultLine,
                    latest_shell
                        .map(|evidence| evidence.provider_result_delivery_status.to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugSelectedShellExecutionPathLine,
                    latest_shell
                        .map(|evidence| evidence.selected_execution_path().to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                ));
            }
            let latest_shell = state.evidence.latest_shell_command_completed();
            body.push(debug_line(
                cosh_shell::MessageId::DebugLatestProviderRequestLine,
                latest_shell
                    .and_then(|evidence| evidence.provider_request_id.as_deref())
                    .unwrap_or("<none>")
                    .to_string(),
            ));
            body.push(debug_line(
                cosh_shell::MessageId::DebugLatestToolUseLine,
                latest_shell
                    .and_then(|evidence| evidence.tool_use_id.as_deref())
                    .unwrap_or("<none>")
                    .to_string(),
            ));
            if let Some(evidence) = state.evidence.latest_recovery() {
                body.push(debug_line(
                    cosh_shell::MessageId::DebugLatestRecoveryStatusLine,
                    evidence.provider_result_delivery_status.to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugLatestRecoveryReasonLine,
                    evidence.recovery_reason.unwrap_or("<none>").to_string(),
                ));
            } else {
                body.push(debug_line(
                    cosh_shell::MessageId::DebugLatestRecoveryStatusLine,
                    "<none>".to_string(),
                ));
                body.push(debug_line(
                    cosh_shell::MessageId::DebugLatestRecoveryReasonLine,
                    "<none>".to_string(),
                ));
            }
            body.extend(continuity_debug_lines(state));
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::DebugSessionTitle),
                body,
                None,
            )
        }
        Some(other) => {
            let i18n = state.i18n();
            render_notice_panel(
                output,
                i18n.t(cosh_shell::MessageId::DebugSessionTitle),
                vec![i18n.format(
                    cosh_shell::MessageId::DebugUnknownTargetBody,
                    &[("target", other)],
                )],
                Some(i18n.t(cosh_shell::MessageId::DebugUnknownTargetFooter)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh_state() -> InlineState {
        InlineState {
            language: cosh_shell::Language::ZhCn,
            ..InlineState::default()
        }
    }

    #[test]
    fn debug_session_uses_zh_catalog_labels() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let state = zh_state();
        let mut output = Vec::new();

        render_debug_command(Some("session"), &adapter, &state, &mut output)
            .expect("render debug session");

        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("会话调试"), "{output}");
        assert!(output.contains("适配器: fake"), "{output}");
        assert!(output.contains("provider 已提交会话: <none>"), "{output}");
        assert!(output.contains("活跃运行: false"), "{output}");
        assert!(output.contains("已选择 shell 执行路径: <none>"), "{output}");
        assert!(!output.contains("Session debug"), "{output}");
        assert!(!output.contains("provider committed session"), "{output}");
    }

    #[test]
    fn debug_unknown_target_uses_zh_catalog_notice() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let state = zh_state();
        let mut output = Vec::new();

        render_debug_command(Some("bad"), &adapter, &state, &mut output)
            .expect("render debug unknown target");

        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("未知 debug 目标: bad"), "{output}");
        assert!(output.contains("使用 /debug session。"), "{output}");
        assert!(!output.contains("Unknown debug target"), "{output}");
    }
}
