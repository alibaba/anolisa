use crate::runtime::prelude::*;

pub(crate) fn render_approval_journal<W: Write>(
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let entries = state
        .approvals
        .journal
        .iter()
        .map(|entry| ApprovalJournalEntryModel {
            id: &entry.id,
            run_id: &entry.run_id,
            source: entry.source,
            decision: entry.decision.label(),
            kind: entry.kind.label(),
            risk: entry.risk,
            subject: &entry.subject,
            preview: &entry.preview,
            preview_hash: &entry.preview_hash,
            request_id: entry.request_id.as_deref(),
            tool_use_id: entry.tool_use_id.as_deref(),
            actor: entry.actor,
            execution_path: entry.execution_path,
            command_block_id: entry.command_block_id.as_deref(),
            redaction_status: entry.redaction_status,
            assessment: entry.assessment.as_ref().map(assessment_summary_model),
        })
        .collect::<Vec<_>>();
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_approval_journal_panel(output, ApprovalJournalPanelModel { entries: &entries })?;
    Ok(())
}

pub(super) fn write_approval_receipt<W: Write>(
    language: Language,
    request: &RuntimeApprovalRequest,
    title: &str,
    output: &mut W,
) -> std::io::Result<()> {
    let i18n = I18n::new(language);
    let foreground_shell_handoff = request.status == ApprovalRequestStatus::Approved
        && request_is_executable_bash_tool(request)
        && request.execution_path != Some("provider_native_shell_tool_execution");
    let provider_native_shell = request.status == ApprovalRequestStatus::Approved
        && request_is_executable_bash_tool(request)
        && request.execution_path == Some("provider_native_shell_tool_execution");
    let decision = approval_receipt_decision(
        &i18n,
        request,
        foreground_shell_handoff,
        provider_native_shell,
    );

    let message = if foreground_shell_handoff {
        i18n.t(MessageId::ApprovalReceiptBashSentToShellMessage)
    } else if provider_native_shell {
        i18n.t(MessageId::ApprovalReceiptProviderNativeAllowedMessage)
    } else {
        ""
    };

    let kind = approval_receipt_kind(&i18n, request, foreground_shell_handoff);
    let subject = approval_receipt_subject(
        &i18n,
        request,
        foreground_shell_handoff,
        provider_native_shell,
    );

    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_approval_receipt_panel(
            output,
            ApprovalReceiptPanelModel {
                title,
                negative: approval_receipt_is_negative(request.status),
                id: &request.id,
                kind,
                decision,
                subject,
                preview: &request.preview,
                message,
            },
        )?;
    Ok(())
}

fn approval_receipt_is_negative(status: ApprovalRequestStatus) -> bool {
    matches!(
        status,
        ApprovalRequestStatus::Denied
            | ApprovalRequestStatus::Cancelled
            | ApprovalRequestStatus::Blocked
    )
}

fn approval_receipt_decision<'a>(
    i18n: &'a I18n,
    request: &RuntimeApprovalRequest,
    foreground_shell_handoff: bool,
    provider_native_shell: bool,
) -> &'a str {
    match request.status {
        ApprovalRequestStatus::Pending => i18n.t(MessageId::ApprovalReceiptDecisionPending),
        ApprovalRequestStatus::Approved => {
            if foreground_shell_handoff {
                i18n.t(MessageId::ApprovalReceiptDecisionSentToShell)
            } else if provider_native_shell {
                i18n.t(MessageId::ApprovalReceiptDecisionProviderNativeAllowed)
            } else if request.kind == ApprovalRequestKind::Tool {
                i18n.t(MessageId::ApprovalReceiptDecisionApproved)
            } else {
                i18n.t(MessageId::ApprovalReceiptDecisionApprovedDisplayOnly)
            }
        }
        ApprovalRequestStatus::Denied => i18n.t(MessageId::ApprovalReceiptDecisionDenied),
        ApprovalRequestStatus::Cancelled => i18n.t(MessageId::ApprovalReceiptDecisionCancelled),
        ApprovalRequestStatus::Blocked => i18n.t(MessageId::ApprovalReceiptDecisionBlocked),
    }
}

fn approval_receipt_kind<'a>(
    i18n: &'a I18n,
    request: &RuntimeApprovalRequest,
    foreground_shell_handoff: bool,
) -> &'a str {
    if foreground_shell_handoff {
        return i18n.t(MessageId::ApprovalReceiptKindBashTool);
    }
    match request.kind {
        ApprovalRequestKind::Tool => i18n.t(MessageId::ApprovalReceiptKindToolRequest),
        ApprovalRequestKind::ShellCommand => {
            i18n.t(MessageId::ApprovalReceiptKindShellCommandRequest)
        }
    }
}

fn approval_receipt_subject<'a>(
    i18n: &'a I18n,
    request: &'a RuntimeApprovalRequest,
    foreground_shell_handoff: bool,
    provider_native_shell: bool,
) -> &'a str {
    if foreground_shell_handoff {
        i18n.t(MessageId::ApprovalReceiptSubjectBashSentToShell)
    } else if provider_native_shell {
        i18n.t(MessageId::ApprovalReceiptSubjectBashProviderNative)
    } else {
        &request.subject
    }
}

pub(crate) fn render_approval_details<W: Write>(
    language: Language,
    request: &RuntimeApprovalRequest,
    output: &mut W,
) -> std::io::Result<()> {
    let i18n = I18n::new(language);
    let preview_label = match request.kind {
        ApprovalRequestKind::Tool => i18n.t(MessageId::ApprovalToolInputLabel),
        ApprovalRequestKind::ShellCommand => i18n.t(MessageId::ApprovalCommandLabel),
    };

    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_approval_details_panel(
            output,
            ApprovalDetailsPanelModel {
                id: &request.id,
                run_id: &request.run_id,
                source: request.source,
                kind: request.kind.label(),
                status: request.status.label(),
                risk: request.risk,
                subject: &request.subject,
                preview_label,
                preview: &request.preview,
                request_id: request.request_id.as_deref(),
                tool_use_id: request.tool_use_id.as_deref(),
                execution_path: request.execution_path,
                command_block_id: request.command_block_id.as_deref(),
                redaction_status: request.redaction_status,
                assessment: request.assessment.as_ref().map(assessment_summary_model),
            },
        )?;
    Ok(())
}

fn assessment_summary_model(
    assessment: &RuntimeCommandAssessmentSummary,
) -> CommandAssessmentSummaryModel<'_> {
    CommandAssessmentSummaryModel {
        impact: assessment.impact,
        execution: assessment.execution,
        confidence: assessment.confidence,
        primary_reason: assessment.primary_reason,
        reason_trace: &assessment.reason_trace,
        auto_allow: assessment.auto_allow,
        output_stability: assessment.output_stability,
        output_exposure: assessment.output_exposure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved_bash_request(execution_path: Option<&'static str>) -> RuntimeApprovalRequest {
        RuntimeApprovalRequest {
            id: "req-zh".to_string(),
            run_id: "run-1".to_string(),
            session_id: "sess-1".to_string(),
            cwd: "/tmp".to_string(),
            source: "control-protocol",
            provider_shell_request_kind: ProviderShellRequestKind::ControlPermission,
            kind: ApprovalRequestKind::Tool,
            subject: "Bash".to_string(),
            preview: "$ echo hi".to_string(),
            risk: "medium",
            request_id: Some("ctrl-1".to_string()),
            tool_use_id: Some("toolu-1".to_string()),
            tool_input: None,
            original_user_request: None,
            status: ApprovalRequestStatus::Approved,
            execution_path,
            command_block_id: None,
            redaction_status: None,
            assessment: None,
        }
    }

    #[test]
    fn approval_receipt_shell_execution_messages_use_zh_catalog() {
        let foreground = approved_bash_request(None);
        let provider_native = approved_bash_request(Some("provider_native_shell_tool_execution"));
        let mut output = Vec::new();

        write_approval_receipt(Language::ZhCn, &foreground, "Approved", &mut output).unwrap();
        write_approval_receipt(Language::ZhCn, &provider_native, "Approved", &mut output).unwrap();

        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Bash tool 已发送到 shell"), "{text}");
        assert!(
            text.contains("已允许 provider-native shell tool 执行"),
            "{text}"
        );
        let old_foreground_text = ["Bash tool", " sent to shell"].concat();
        let old_provider_native_text = ["Provider-native shell", " tool allowed"].concat();
        assert!(!text.contains(&old_foreground_text), "{text}");
        assert!(!text.contains(&old_provider_native_text), "{text}");
    }

    #[test]
    fn approval_receipt_metadata_uses_zh_catalog() {
        let i18n = I18n::new(Language::ZhCn);
        let foreground = approved_bash_request(None);
        let provider_native = approved_bash_request(Some("provider_native_shell_tool_execution"));

        assert_eq!(
            approval_receipt_decision(&i18n, &foreground, true, false),
            "已发送到 shell"
        );
        assert_eq!(approval_receipt_kind(&i18n, &foreground, true), "Bash tool");
        assert_eq!(
            approval_receipt_subject(&i18n, &foreground, true, false),
            "Bash tool: 已发送到 shell"
        );
        assert_eq!(
            approval_receipt_decision(&i18n, &provider_native, false, true),
            "已允许 provider-native 执行"
        );
        assert_eq!(
            approval_receipt_subject(&i18n, &provider_native, false, true),
            "Bash tool: provider-native 执行"
        );
    }
}
