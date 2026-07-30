macro_rules! approval_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalTitle,
            ApprovalRequiredTitle,
            ApprovalResolutionApprovedTitle,
            ApprovalResolutionAutoApprovedTitle,
            ApprovalResolutionTrustedTitle,
            ApprovalResolutionDeniedTitle,
            ApprovalResolutionCancelledTitle,
            ApprovalResolutionBlockedTitle,
            ApprovalResolutionDeferredTitle,
            ApprovalActionAllowOnce,
            ApprovalActionAlwaysTrust,
            ApprovalActionDeny,
            ApprovalActionDetails,
            ApprovalToolInputLabel,
            ApprovalCommandLabel,
            ApprovalDetailsTitle,
            ApprovalDetailsSourceLabel,
            ApprovalDetailsRunLabel,
            ApprovalDetailsExecutionLabel,
            ApprovalDetailsCommandBlockLabel,
            ApprovalDetailsRedactionLabel,
            ApprovalDetailsProviderRequestLabel,
            ApprovalDetailsToolUseLabel,
            ApprovalDetailsDefaultDenyLine,
            ApprovalDetailsRequestLabel,
            ApprovalDetailsInputLabel,
            ApprovalDetailsBashCommandSubject,
            ApprovalDetailsShellCommandSubject,
            ApprovalDetailsToolSubject,
            ApprovalDetailsPendingValue,
            ApprovalDetailsNoneValue,
            ApprovalDetailsNotApplicableValue,
            ApprovalAssessmentSummaryLine,
            ApprovalAssessmentReasonLine,
            ApprovalJournalTitle,
            ApprovalJournalDecisionCount,
            ApprovalJournalEmptyBody,
            ApprovalJournalActorLabel,
            ApprovalJournalPreviewHashLabel,
            ApprovalJournalSubjectLabel,
            ApprovalJournalPreviewLabel,
            ApprovalRiskSuffix,
            ApprovalQueueCompactLine,
            ApprovalQueueFullLine,
            ApprovalQueueNextSuffix,
            ApprovalSubjectLabel,
            ApprovalNextLabel,
            ApprovalKeysPrefix,
            ApprovalKeysText,
            ApprovalExecutableToolPolicy,
            ApprovalExecutableToolPolicyExtra,
            ApprovalCommandDefaultPolicy,
            ApprovalRunShellCommandPrompt,
            ApprovalRunBashCommandPrompt,
            ApprovalNotFoundTitle,
            ApprovalNotFoundBody,
            ApprovalShellHandoffNotFoundTitle,
            ApprovalShellHandoffNotFoundBody,
            ApprovalShellHandoffBlockedTitle,
            ApprovalShellHandoffBlockedFooter,
            ApprovalShellHandoffValidationEmptyCommand,
            ApprovalShellHandoffValidationMultilineCommand,
            ApprovalShellHandoffValidationControlCharacter,
            ApprovalShellHandoffValidationEmptyPreview,
            ApprovalShellHandoffValidationEmptyApprovalId,
            ApprovalShellHandoffValidationEmptyRunId,
            ApprovalShellHandoffSendingTitle,
            ApprovalShellHandoffSendingBody,
            ApprovalShellHandoffTimeoutTitle,
            ApprovalShellHandoffTimeoutExceededBody,
            ApprovalShellHandoffTimeoutInterruptBody,
            ApprovalReceiptKindToolRequest,
            ApprovalReceiptKindShellCommandRequest,
            ApprovalReceiptKindBashTool,
            ApprovalReceiptDecisionPending,
            ApprovalReceiptDecisionApproved,
            ApprovalReceiptDecisionSentToShell,
            ApprovalReceiptDecisionProviderNativeAllowed,
            ApprovalReceiptDecisionApprovedDisplayOnly,
            ApprovalReceiptDecisionDenied,
            ApprovalReceiptDecisionCancelled,
            ApprovalReceiptDecisionBlocked,
            ApprovalReceiptSubjectBashSentToShell,
            ApprovalReceiptSubjectBashProviderNative,
            ApprovalReceiptBashSentToShellMessage,
            ApprovalReceiptProviderNativeAllowedMessage,
            ApprovalHookHeading,
        );
    };
}

macro_rules! approval_reason_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalRiskDetailLabel,
            ApprovalRiskLevelHigh,
            ApprovalRiskLevelMedium,
            ApprovalRiskLevelLow,
            ApprovalQueueMetaSuffix,
            ApprovalRiskPhrasePrivilegeEscalation,
            ApprovalRiskPhraseCredentialAccess,
            ApprovalRiskPhraseFilesystemDelete,
            ApprovalRiskPhraseFilesystemWrite,
            ApprovalRiskPhrasePermissionChange,
            ApprovalRiskPhraseProcessControl,
            ApprovalRiskPhraseServiceControl,
            ApprovalRiskPhraseServiceOrContainerControl,
            ApprovalRiskPhrasePackageManagerMutation,
            ApprovalRiskPhraseInteractiveEditor,
            ApprovalRiskPhraseRemoteCodeExecution,
            ApprovalRiskPhraseSensitivePath,
            ApprovalRiskPhraseSensitiveSearch,
            ApprovalRiskPhraseCommandSubstitution,
            ApprovalRiskPhraseRedirectionWrite,
            ApprovalRiskPhraseAwkShellExecution,
            ApprovalRiskLevelUnknown,
        );
    };
}

// Trailing segment (issue #1773): appended after all existing segments so
// every pre-existing MessageId discriminant stays stable, per the
// stable-runtime-api trailing-segment contract established in #1721.
macro_rules! approval_turn_consent_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalResolutionTurnApprovedTitle,
            ApprovalActionApproveTurn,
        );
    };
}

// Trailing segment (issue #1988): appended after all existing segments so
// every pre-existing MessageId discriminant stays stable, per the
// stable-runtime-api trailing-segment contract established in #1721.
macro_rules! approval_foreground_interactive_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalReceiptForegroundInteractiveHint,
        );
    };
}

// Trailing segment (issue #2029): appended after all existing segments so
// every pre-existing MessageId discriminant stays stable.
macro_rules! approval_turn_extension_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalTurnExtensionSubject,
            ApprovalTurnExtensionPreview,
            ApprovalTurnExtensionLabel,
            ApprovalActionContinue,
            ApprovalActionStop,
            ApprovalResolutionContinuingTitle,
            ApprovalResolutionStoppedTitle,
            ApprovalReceiptKindTurnExtension,
            ApprovalTurnExtensionUnavailableTitle,
            ApprovalTurnExtensionUnavailableBody,
        );
    };
}
