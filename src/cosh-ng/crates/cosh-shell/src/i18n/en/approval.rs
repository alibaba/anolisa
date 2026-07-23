use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ApprovalTitle => "Approval",
        MessageId::ApprovalRequiredTitle => "Approval required",
        MessageId::ApprovalResolutionApprovedTitle => "Approved",
        MessageId::ApprovalResolutionAutoApprovedTitle => "Auto-approved",
        MessageId::ApprovalResolutionTrustedTitle => "Trusted",
        MessageId::ApprovalResolutionDeniedTitle => "Denied",
        MessageId::ApprovalResolutionCancelledTitle => "Cancelled",
        MessageId::ApprovalResolutionBlockedTitle => "Blocked",
        MessageId::ApprovalResolutionDeferredTitle => "Deferred",
        MessageId::ApprovalActionAllowOnce => "Allow once",
        MessageId::ApprovalActionAlwaysTrust => "Always trust",
        MessageId::ApprovalActionDeny => "Deny",
        MessageId::ApprovalActionDetails => "Details",
        MessageId::ApprovalToolInputLabel => "Tool input",
        MessageId::ApprovalCommandLabel => "Command",
        MessageId::ApprovalDetailsTitle => "Approval details",
        MessageId::ApprovalDetailsSourceLabel => "Source",
        MessageId::ApprovalDetailsRunLabel => "Run",
        MessageId::ApprovalDetailsExecutionLabel => "Execution",
        MessageId::ApprovalDetailsCommandBlockLabel => "Command block",
        MessageId::ApprovalDetailsRedactionLabel => "Redaction",
        MessageId::ApprovalDetailsProviderRequestLabel => "Provider request",
        MessageId::ApprovalDetailsToolUseLabel => "Tool use",
        MessageId::ApprovalDetailsDefaultDenyLine => "Default: deny",
        MessageId::ApprovalDetailsRequestLabel => "Request",
        MessageId::ApprovalDetailsInputLabel => "Input",
        MessageId::ApprovalDetailsBashCommandSubject => "Bash command",
        MessageId::ApprovalDetailsShellCommandSubject => "Shell command",
        MessageId::ApprovalDetailsToolSubject => "{tool} tool",
        MessageId::ApprovalDetailsPendingValue => "<pending>",
        MessageId::ApprovalDetailsNoneValue => "<none>",
        MessageId::ApprovalDetailsNotApplicableValue => "<not-applicable>",
        MessageId::ApprovalAssessmentSummaryLine => {
            "Assessment: impact {impact}; decision {decision}; confidence {confidence}"
        }
        MessageId::ApprovalAssessmentReasonLine => "Reason: {reason}",
        MessageId::ApprovalJournalTitle => "Approval journal",
        MessageId::ApprovalJournalDecisionCount => "{count} decisions",
        MessageId::ApprovalJournalEmptyBody => {
            "No approval decisions recorded in this shell session."
        }
        MessageId::ApprovalJournalActorLabel => "Actor",
        MessageId::ApprovalJournalPreviewHashLabel => "Preview hash",
        MessageId::ApprovalJournalSubjectLabel => "Subject",
        MessageId::ApprovalJournalPreviewLabel => "Preview",
        MessageId::ApprovalRiskSuffix => "{risk} risk",
        MessageId::ApprovalQueueCompactLine => "Queue: {position}/{total} pending",
        MessageId::ApprovalQueueFullLine => "Queue: {position} of {total} pending",
        MessageId::ApprovalQueueNextSuffix => "; next {next}",
        MessageId::ApprovalSubjectLabel => "Subject: ",
        MessageId::ApprovalNextLabel => "Next: ",
        MessageId::ApprovalKeysPrefix => "Keys: ",
        MessageId::ApprovalKeysText => "Left/Right select  Enter confirm  d details  Esc cancel",
        MessageId::ApprovalExecutableToolPolicy => {
            "Policy: user approval is required before any executable tool request."
        }
        MessageId::ApprovalExecutableToolPolicyExtra => {
            "Only approved read-only Bash/shell tool requests may run in this MVP."
        }
        MessageId::ApprovalCommandDefaultPolicy => {
            "Default: deny. Approved command is rechecked by read-only broker."
        }
        MessageId::ApprovalRunShellCommandPrompt => "Run shell command?",
        MessageId::ApprovalRunBashCommandPrompt => "Run Bash command?",
        MessageId::ApprovalNotFoundTitle => "Approval not found",
        MessageId::ApprovalNotFoundBody => {
            "{id} is not available; the approval card may already be resolved"
        }
        MessageId::ApprovalShellHandoffNotFoundTitle => "Shell handoff not found",
        MessageId::ApprovalShellHandoffNotFoundBody => {
            "{id} is not available; use Details on the provider tool failure first"
        }
        MessageId::ApprovalShellHandoffBlockedTitle => "Shell handoff blocked",
        MessageId::ApprovalShellHandoffBlockedFooter => {
            "The command was not written to the foreground shell."
        }
        MessageId::ApprovalShellHandoffValidationEmptyCommand => "Shell handoff command is empty.",
        MessageId::ApprovalShellHandoffValidationMultilineCommand => {
            "Shell handoff command contains a newline; multiline handoff is not enabled."
        }
        MessageId::ApprovalShellHandoffValidationControlCharacter => {
            "Shell handoff command contains a blocked control character."
        }
        MessageId::ApprovalShellHandoffValidationEmptyPreview => "Shell handoff preview is empty.",
        MessageId::ApprovalShellHandoffValidationEmptyApprovalId => {
            "Shell handoff approval id is empty."
        }
        MessageId::ApprovalShellHandoffValidationEmptyRunId => "Shell handoff run id is empty.",
        MessageId::ApprovalShellHandoffSendingTitle => "Sending to shell",
        MessageId::ApprovalShellHandoffSendingBody => "{id} will run in the foreground shell.",
        MessageId::ApprovalShellHandoffTimeoutTitle => "Shell recovery",
        MessageId::ApprovalShellHandoffTimeoutExceededBody => {
            "Command exceeded configured shell handoff timeout ({seconds}s)."
        }
        MessageId::ApprovalShellHandoffTimeoutInterruptBody => {
            "Sent interrupt to foreground PTY; waiting for shell evidence."
        }
        MessageId::ApprovalReceiptKindToolRequest => "tool request",
        MessageId::ApprovalReceiptKindShellCommandRequest => "shell command request",
        MessageId::ApprovalReceiptKindBashTool => "Bash tool",
        MessageId::ApprovalReceiptDecisionPending => "pending",
        MessageId::ApprovalReceiptDecisionApproved => "approved",
        MessageId::ApprovalReceiptDecisionSentToShell => "sent to shell",
        MessageId::ApprovalReceiptDecisionProviderNativeAllowed => {
            "allowed provider-native execution"
        }
        MessageId::ApprovalReceiptDecisionApprovedDisplayOnly => "approved for display only",
        MessageId::ApprovalReceiptDecisionDenied => "denied",
        MessageId::ApprovalReceiptDecisionCancelled => "cancelled by user",
        MessageId::ApprovalReceiptDecisionBlocked => "blocked by cosh-shell",
        MessageId::ApprovalReceiptSubjectBashSentToShell => "Bash tool: sent to shell",
        MessageId::ApprovalReceiptSubjectBashProviderNative => {
            "Bash tool: provider-native execution"
        }
        MessageId::ApprovalReceiptBashSentToShellMessage => "Bash tool sent to shell",
        MessageId::ApprovalReceiptProviderNativeAllowedMessage => {
            "Provider-native shell tool allowed"
        }
        MessageId::ApprovalHookHeading => "Hook review",
        _ => return None,
    })
}
