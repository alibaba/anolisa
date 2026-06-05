use crate::config::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageId {
    StartupTitle,
    StartupAdapterLine,
    StartupCwdLine,
    StartupCommandsLine,
    StartupHooksNoneSummary,
    StartupHooksCompletedSummary,
    StartupHooksFindingsHeading,
    StartupHooksRustProjectFinding,
    StartupHooksNoFindings,
    StartupHooksReadOnlyNote,
    HelpTitle,
    HelpFooter,
    HelpGroupConfig,
    HelpGroupModes,
    HelpGroupHooks,
    HelpSummaryHelp,
    HelpSummaryConfig,
    HelpSummaryModeApproval,
    HelpSummaryModeAnalysis,
    HelpSummaryAgent,
    HelpSummaryExplain,
    HelpSummaryCancel,
    HelpSummaryDetails,
    HelpSummaryAudit,
    HelpSummaryHooks,
    HelpSummarySelect,
    HelpSummaryCopy,
    HelpSummaryDebug,
    HelpSummaryClear,
    HelpSummaryShell,
    HelpSummarySkill,
    HelpSummaryApprovalModeRemoved,
    SlashHintTitle,
    SlashHintPrefix,
    SlashHintCurrentMode,
    SlashHintFooter,
    SlashUnknownTitle,
    SlashUnknownBody,
    SlashUnknownSuggestionBody,
    SlashUnknownFooter,
    SlashInfoAuditTitle,
    SlashInfoAuditApprovalsBody,
    SlashInfoAuditActivityBody,
    SlashInfoAuditFooter,
    SlashInfoConfigTitle,
    SlashInfoConfigLanguageLine,
    SlashInfoConfigLanguageEffectiveLine,
    SlashInfoConfigPathLine,
    SlashInfoConfigDebugActivityLine,
    SlashInfoConfigAnalysisStrategyLine,
    SlashInfoConfigRenderFallbackLine,
    SlashInfoConfigFooter,
    SlashInfoSkillTitle,
    SlashInfoSkillHookRoutingBody,
    SlashInfoSkillRegistryBody,
    SlashInfoSkillFooter,
    ConfigInvalidLanguageBody,
    ConfigSupportedLanguagesFooter,
    ConfigUnknownKeyBody,
    ConfigHomeMissingBody,
    ConfigHomeMissingFooter,
    ConfigUnchangedTitle,
    ConfigNoFileChangedBody,
    ConfigSavedTitle,
    ConfigSavedValueLine,
    ConfigCurrentSessionLanguageLine,
    ConfigSavedFooter,
    ConfigSaveFailedTitle,
    ConfigSaveFailedBody,
    ConfigSavePromptTitle,
    ConfigFileLine,
    ConfigPendingChangeLine,
    ConfigSaveButton,
    ConfigCancelButton,
    ConfigApplyKeysFooter,
    ConfigLanguageTitle,
    ConfigLanguageAutoLine,
    ConfigLanguageEnLine,
    ConfigLanguageZhLine,
    ConfigLanguageKeysFooter,
    SlashHooksRegisteredTitle,
    SlashHooksNoHooksBody,
    SlashHooksStatusCountLine,
    SlashHooksStatusSourcesLine,
    SlashHooksStatusProjectTrustLine,
    SlashHooksFooterCount,
    SlashHooksFooterMutedTargets,
    SlashHooksTargetMutedTitle,
    SlashHooksTargetMutedBody,
    SlashHooksTargetMutedFooter,
    SlashHooksTargetUnmutedTitle,
    SlashHooksTargetUnmutedBody,
    SlashHooksTargetNotMutedBody,
    SlashHooksEnabledTitle,
    SlashHooksEnabledBody,
    SlashHooksDisabledTitle,
    SlashHooksDisabledBody,
    SlashHooksHistoryTitle,
    SlashHooksHistoryEmptyBody,
    SlashHooksHistoryFooter,
    SlashHooksEventsTitle,
    SlashHooksEventsEmptyBody,
    SlashHooksEventsFooter,
    SlashHooksUsageTitle,
    SlashHooksUsageListLine,
    SlashHooksUsageHistoryLine,
    SlashHooksUsageEventsLine,
    SlashHooksUsageAnalyzeLine,
    SlashHooksUsageIgnoreLine,
    SlashHooksUsageDetailsLine,
    SlashHooksUsageFeedbackLine,
    SlashHooksUsageClearFeedbackLine,
    SlashHooksUsageMuteLine,
    SlashHooksUsageUnmuteLine,
    SlashHooksUsageTrustProjectLine,
    SlashHooksUsageUntrustProjectLine,
    SlashHooksUsageClearProjectTrustLine,
    SlashHooksUsageEnableLine,
    SlashHooksUsageDisableLine,
    SlashHooksProjectTrustedTitle,
    SlashHooksProjectUntrustedTitle,
    SlashHooksProjectTrustNoHooksBody,
    SlashHooksProjectTrustedBody,
    SlashHooksProjectUntrustedBody,
    SlashHooksProjectTrustNoChangeFooter,
    SlashHooksProjectTrustPersistedFooter,
    SlashHooksProjectTrustRemovedFooter,
    SlashHooksProjectTrustPersistenceFailedFooter,
    SlashHooksProjectTrustClearedTitle,
    SlashHooksProjectTrustClearedBody,
    SlashHooksProjectTrustClearedFooter,
    SlashHooksProjectTrustClearFailedFooter,
    SlashHooksFeedbackUsageBody,
    SlashHooksFeedbackTitle,
    SlashHooksFeedbackFindingNotFoundBody,
    SlashHooksFeedbackFindingNotFoundFooter,
    SlashHooksFeedbackRecordedTitle,
    SlashHooksFeedbackRecordedBody,
    SlashHooksFeedbackHookLine,
    SlashHooksFeedbackPolicyKeyLine,
    SlashHooksFeedbackPersistedFooter,
    SlashHooksFeedbackPersistenceFailedFooter,
    SlashHooksFeedbackClearedTitle,
    SlashHooksFeedbackClearedBody,
    SlashHooksFeedbackClearedFooter,
    SlashHooksFeedbackClearFailedFooter,
    DebugSessionTitle,
    DebugAdapterLine,
    DebugProviderCommittedSessionLine,
    DebugActiveRunLine,
    DebugQueuedRunsLine,
    DebugProviderPendingSessionLine,
    DebugProviderInitializeSeenLine,
    DebugHostExecutedShellResultLine,
    DebugSelectedShellExecutionPathLine,
    DebugLatestRecoveryStatusLine,
    DebugLatestRecoveryReasonLine,
    DebugUnknownTargetBody,
    DebugUnknownTargetFooter,
    CommandRemovedTitle,
    ApprovalModeRemovedBody,
    ApprovalModeRemovedFooter,
    RemovedDecisionCommandBody,
    RemovedApprovalDecisionFooter,
    RemovedQuestionAnswerFooter,
    ModeTitle,
    ModesTitle,
    ModeApprovalLine,
    ModeAnalysisLine,
    ModeSummaryFooter,
    ModeRemovedTitle,
    ModeRemovedBody,
    ModeRemovedFooter,
    ModeLanguageBody,
    ModeLanguageFooter,
    ModeUnknownBody,
    ModeUnknownFooter,
    ApprovalModeTitle,
    ApprovalModeSetBody,
    ApprovalModeUnknownBody,
    ApprovalModeUsageFooter,
    ApprovalModeRecommendFooter,
    ApprovalModeAutoFooter,
    ApprovalModeTrustFooter,
    ApprovalModeTrustConfirmationTitle,
    ApprovalModeTrustConfirmationBody,
    ApprovalModeTrustConfirmationCommandBody,
    ApprovalModeTrustConfirmationFooter,
    ApprovalModeCardTitle,
    ApprovalModeCardCurrentLine,
    ApprovalModeCardRecommendLine,
    ApprovalModeCardAutoLine,
    ApprovalModeCardTrustLine,
    ApprovalModeCardFooter,
    ApprovalModeRemainsBody,
    ApprovalModeCancelBody,
    ApprovalModeCancelFooter,
    AnalysisModeTitle,
    AnalysisModeCurrentBody,
    AnalysisModeSetBody,
    AnalysisModeUnknownBody,
    AnalysisModeUsageFooter,
    AnalysisModeSmartFooter,
    AnalysisModeAutoFooter,
    AnalysisModeManualFooter,
    AgentThinking,
    AgentThinkingElapsed,
    AgentRecoveryTitle,
    AgentRecoveryFreshTurnBody,
    AgentRecoveryContinuityBody,
    AgentStatusTitle,
    AgentStillWorking,
    AgentStatusFooter,
    AgentStatusStarting,
    AgentStatusWaitingBackend,
    AgentStatusStreaming,
    AgentStatusReceivingResponse,
    AgentStatusSkill,
    AgentStatusLoadingSkill,
    AgentStatusApproval,
    AgentStatusWaitingApprovalTool,
    AgentStatusQuestion,
    AgentStatusWaitingUserAnswer,
    AgentStatusWaitingApprovalCommand,
    AgentStatusTool,
    AgentStatusCapturingToolOutput,
    AgentStatusToolCompleted,
    AgentStatusCompleted,
    AgentStatusFailed,
    AgentStatusCancelled,
    AgentStatusRunningApprovedProviderTool,
    AgentStatusSkillFailed,
    AgentProviderTimeoutDroppedQueuedBody,
    AgentCancellationRequestedTitle,
    AgentCancellationRequestedBody,
    AgentCancelledReasonLabel,
    AgentCancelledUserRequestedReason,
    AgentResponseTitle,
    AgentGovernanceTitle,
    AgentGovernanceStatusLine,
    AgentGovernanceReasonLine,
    AgentGovernanceSummaryLine,
    AgentGovernanceErrorLine,
    AgentGovernanceSkillLoadingLine,
    AgentGovernanceSkillLoadedLine,
    AgentGovernanceSkillFailedLine,
    AgentGovernanceToolOutputLine,
    AgentGovernanceToolCompletedLine,
    AgentGovernanceApprovalRequiredLine,
    AgentGovernanceShellCommandSubject,
    AgentGovernanceBashCommandSubject,
    AgentGovernanceToolSubject,
    AgentGovernanceBlockedUserApprovalLine,
    AgentGovernanceQuestionLine,
    AgentRecommendedCommandsLabel,
    InterceptNoticeTitle,
    InterceptNoticeBody,
    InterceptNoticeFooter,
    FailedCommandCardTitle,
    FailedCommandCardBody,
    FailedCommandCardFooter,
    FailedAnalysisCancelledTitle,
    FailedAnalysisCancelledBody,
    FailedAnalysisCancelNoActiveBody,
    FailedAnalysisCancelledFooter,
    AnalysisSkippedTitle,
    AnalysisSkippedBody,
    AnalysisSkippedFooter,
    HookAutoAnalyzedTitle,
    HookAutoAnalyzedBody,
    HookAutoAnalyzedFooter,
    AgentQueuedTitle,
    AgentQueuedBodyCommand,
    AgentQueuedBodyActive,
    AgentQueuedFooter,
    HookFindingTitle,
    HookFindingFooter,
    HookFindingMarkdownTitle,
    HookFindingMarkdownHookLine,
    HookFindingMarkdownSeverityLine,
    HookFindingMarkdownFindingLine,
    HookFindingMarkdownOutputRefLine,
    HookFindingMarkdownSuggestionLine,
    HookFindingMarkdownRelatedTitle,
    HookFindingMarkdownRelatedLine,
    HookFindingMarkdownAgentFollowUpLine,
    HookHintTitle,
    HookHintNotFoundBody,
    HookHintNotFoundFooter,
    HookHintNoFindingBody,
    HookHintBlockUnavailableBody,
    HookHintIgnoredTitle,
    HookHintIgnoredBody,
    HookHintIgnoredFooter,
    HookHintUsageTitle,
    HookHintUsageBody,
    HookFindingDetailsTitle,
    HookConsultationHookLabel,
    HookConsultationConfidenceReasonLine,
    HookConsultationAnalyzeAction,
    HookConsultationIgnoreAction,
    HookDetailsConfidenceLine,
    HookDetailsUserInterestLine,
    HookDetailsReasonLookupIntent,
    HookDetailsReasonPipelineIntent,
    HookDetailsReasonScriptIntent,
    HookDetailsReasonWrapperLowConfidence,
    HookDetailsReasonInteractiveIntent,
    HookDetailsReasonActiveRunDeferred,
    HookDetailsReasonUserContinuedInput,
    HookDetailsReasonNonDiagnosticSuccessCommand,
    HookDetailsReasonFeedbackNoisy,
    HookDetailsReasonIgnoredSameFinding,
    HookDetailsReasonSameCardAlreadyRendered,
    HookDetailsReasonInterruptionBudget,
    HookDetailsReasonLowConfidence,
    HookDetailsReasonDiagnosticIntent,
    HookDetailsReasonOtherIntent,
    HookDetailsTopicLine,
    HookDetailsSuppressionKeyLine,
    HookDetailsOutputRefLine,
    HookDetailsCreatedAtLine,
    HookDetailsPromptHintLine,
    HookDetailsRecommendedSkillLine,
    HookDetailsReadOnlyCliHintLine,
    HookDetailsFooter,
    RuntimeDetailsUnavailableTitle,
    RuntimeDetailsUnavailableBody,
    ActivityTitle,
    ActivityDetailsTitle,
    ActivityRunLabel,
    ActivityDetailLabel,
    ActivitySkillLabel,
    ActivitySkillUpdatedStatus,
    ActivityToolLabel,
    ActivityToolOutputLabel,
    ActivityShellLabel,
    ActivityStatusLoading,
    ActivityStatusLoaded,
    ActivityStatusFailed,
    ActivityStatusCalled,
    ActivityStatusRequested,
    ActivityStatusCaptured,
    ActivityStatusCompleted,
    ActivityStatusError,
    ActivityStatusInterrupted,
    ActivitySkillLoadingSummary,
    ActivitySkillLoadedSummary,
    ActivitySkillFailedSummary,
    ActivityToolCalledSummary,
    ActivityToolRequestedSummary,
    ActivityToolOutputCapturedSummary,
    ActivityProviderNativeShellBypassSummary,
    ActivityToolNeedsForegroundShellSummary,
    ActivityShellHandoffSentSummary,
    MarkdownCodeLabel,
    MarkdownCodeWithLanguageLabel,
    MarkdownTableLabel,
    RecommendationTitle,
    RecommendationEmptyBody,
    RecommendationFooter,
    RecommendationNoSelectableTitle,
    RecommendationNoSelectableBody,
    RecommendationUnavailableTitle,
    RecommendationUnavailableBody,
    RecommendationSelectedTitle,
    RecommendationSelectedBody,
    RecommendationCopiedTitle,
    RecommendationCopiedBody,
    RecommendationInsertTitle,
    RecommendationInsertBody,
    RecommendationDetailsTitle,
    RecommendationDetailsBody,
    RecommendationDisplayOnlyBody,
    RecommendationCopyOnlyBody,
    RecommendationInsertOnlyBody,
    RecommendationDetailsOnlyBody,
    ToolOutputStdoutCapturedSummary,
    ToolOutputStderrCapturedSummary,
    ToolSummaryExit,
    ToolSummaryBlocked,
    ToolSummaryTimedOut,
    ToolSummaryFailed,
    QuestionTitle,
    QuestionAnswerLabel,
    QuestionSelectOneLabel,
    QuestionSelectMultipleLabel,
    QuestionOtherEmptyLabel,
    QuestionOtherValueLabel,
    QuestionKeysPrefix,
    QuestionInstructionMoveTypeSend,
    QuestionInstructionMoveToggleSend,
    QuestionInstructionMoveSend,
    QuestionInstructionTypeSend,
    QuestionInstructionNoAnswer,
    QuestionNoPendingTitle,
    QuestionNoPendingBody,
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
}

impl MessageId {
    pub const ALL: &'static [MessageId] = &[
        MessageId::StartupTitle,
        MessageId::StartupAdapterLine,
        MessageId::StartupCwdLine,
        MessageId::StartupCommandsLine,
        MessageId::StartupHooksNoneSummary,
        MessageId::StartupHooksCompletedSummary,
        MessageId::StartupHooksFindingsHeading,
        MessageId::StartupHooksRustProjectFinding,
        MessageId::StartupHooksNoFindings,
        MessageId::StartupHooksReadOnlyNote,
        MessageId::HelpTitle,
        MessageId::HelpFooter,
        MessageId::HelpGroupConfig,
        MessageId::HelpGroupModes,
        MessageId::HelpGroupHooks,
        MessageId::HelpSummaryHelp,
        MessageId::HelpSummaryConfig,
        MessageId::HelpSummaryModeApproval,
        MessageId::HelpSummaryModeAnalysis,
        MessageId::HelpSummaryAgent,
        MessageId::HelpSummaryExplain,
        MessageId::HelpSummaryCancel,
        MessageId::HelpSummaryDetails,
        MessageId::HelpSummaryAudit,
        MessageId::HelpSummaryHooks,
        MessageId::HelpSummarySelect,
        MessageId::HelpSummaryCopy,
        MessageId::HelpSummaryDebug,
        MessageId::HelpSummaryClear,
        MessageId::HelpSummaryShell,
        MessageId::HelpSummarySkill,
        MessageId::HelpSummaryApprovalModeRemoved,
        MessageId::SlashHintTitle,
        MessageId::SlashHintPrefix,
        MessageId::SlashHintCurrentMode,
        MessageId::SlashHintFooter,
        MessageId::SlashUnknownTitle,
        MessageId::SlashUnknownBody,
        MessageId::SlashUnknownSuggestionBody,
        MessageId::SlashUnknownFooter,
        MessageId::SlashInfoAuditTitle,
        MessageId::SlashInfoAuditApprovalsBody,
        MessageId::SlashInfoAuditActivityBody,
        MessageId::SlashInfoAuditFooter,
        MessageId::SlashInfoConfigTitle,
        MessageId::SlashInfoConfigLanguageLine,
        MessageId::SlashInfoConfigLanguageEffectiveLine,
        MessageId::SlashInfoConfigPathLine,
        MessageId::SlashInfoConfigDebugActivityLine,
        MessageId::SlashInfoConfigAnalysisStrategyLine,
        MessageId::SlashInfoConfigRenderFallbackLine,
        MessageId::SlashInfoConfigFooter,
        MessageId::SlashInfoSkillTitle,
        MessageId::SlashInfoSkillHookRoutingBody,
        MessageId::SlashInfoSkillRegistryBody,
        MessageId::SlashInfoSkillFooter,
        MessageId::ConfigInvalidLanguageBody,
        MessageId::ConfigSupportedLanguagesFooter,
        MessageId::ConfigUnknownKeyBody,
        MessageId::ConfigHomeMissingBody,
        MessageId::ConfigHomeMissingFooter,
        MessageId::ConfigUnchangedTitle,
        MessageId::ConfigNoFileChangedBody,
        MessageId::ConfigSavedTitle,
        MessageId::ConfigSavedValueLine,
        MessageId::ConfigCurrentSessionLanguageLine,
        MessageId::ConfigSavedFooter,
        MessageId::ConfigSaveFailedTitle,
        MessageId::ConfigSaveFailedBody,
        MessageId::ConfigSavePromptTitle,
        MessageId::ConfigFileLine,
        MessageId::ConfigPendingChangeLine,
        MessageId::ConfigSaveButton,
        MessageId::ConfigCancelButton,
        MessageId::ConfigApplyKeysFooter,
        MessageId::ConfigLanguageTitle,
        MessageId::ConfigLanguageAutoLine,
        MessageId::ConfigLanguageEnLine,
        MessageId::ConfigLanguageZhLine,
        MessageId::ConfigLanguageKeysFooter,
        MessageId::SlashHooksRegisteredTitle,
        MessageId::SlashHooksNoHooksBody,
        MessageId::SlashHooksStatusCountLine,
        MessageId::SlashHooksStatusSourcesLine,
        MessageId::SlashHooksStatusProjectTrustLine,
        MessageId::SlashHooksFooterCount,
        MessageId::SlashHooksFooterMutedTargets,
        MessageId::SlashHooksTargetMutedTitle,
        MessageId::SlashHooksTargetMutedBody,
        MessageId::SlashHooksTargetMutedFooter,
        MessageId::SlashHooksTargetUnmutedTitle,
        MessageId::SlashHooksTargetUnmutedBody,
        MessageId::SlashHooksTargetNotMutedBody,
        MessageId::SlashHooksEnabledTitle,
        MessageId::SlashHooksEnabledBody,
        MessageId::SlashHooksDisabledTitle,
        MessageId::SlashHooksDisabledBody,
        MessageId::SlashHooksHistoryTitle,
        MessageId::SlashHooksHistoryEmptyBody,
        MessageId::SlashHooksHistoryFooter,
        MessageId::SlashHooksEventsTitle,
        MessageId::SlashHooksEventsEmptyBody,
        MessageId::SlashHooksEventsFooter,
        MessageId::SlashHooksUsageTitle,
        MessageId::SlashHooksUsageListLine,
        MessageId::SlashHooksUsageHistoryLine,
        MessageId::SlashHooksUsageEventsLine,
        MessageId::SlashHooksUsageAnalyzeLine,
        MessageId::SlashHooksUsageIgnoreLine,
        MessageId::SlashHooksUsageDetailsLine,
        MessageId::SlashHooksUsageFeedbackLine,
        MessageId::SlashHooksUsageClearFeedbackLine,
        MessageId::SlashHooksUsageMuteLine,
        MessageId::SlashHooksUsageUnmuteLine,
        MessageId::SlashHooksUsageTrustProjectLine,
        MessageId::SlashHooksUsageUntrustProjectLine,
        MessageId::SlashHooksUsageClearProjectTrustLine,
        MessageId::SlashHooksUsageEnableLine,
        MessageId::SlashHooksUsageDisableLine,
        MessageId::SlashHooksProjectTrustedTitle,
        MessageId::SlashHooksProjectUntrustedTitle,
        MessageId::SlashHooksProjectTrustNoHooksBody,
        MessageId::SlashHooksProjectTrustedBody,
        MessageId::SlashHooksProjectUntrustedBody,
        MessageId::SlashHooksProjectTrustNoChangeFooter,
        MessageId::SlashHooksProjectTrustPersistedFooter,
        MessageId::SlashHooksProjectTrustRemovedFooter,
        MessageId::SlashHooksProjectTrustPersistenceFailedFooter,
        MessageId::SlashHooksProjectTrustClearedTitle,
        MessageId::SlashHooksProjectTrustClearedBody,
        MessageId::SlashHooksProjectTrustClearedFooter,
        MessageId::SlashHooksProjectTrustClearFailedFooter,
        MessageId::SlashHooksFeedbackUsageBody,
        MessageId::SlashHooksFeedbackTitle,
        MessageId::SlashHooksFeedbackFindingNotFoundBody,
        MessageId::SlashHooksFeedbackFindingNotFoundFooter,
        MessageId::SlashHooksFeedbackRecordedTitle,
        MessageId::SlashHooksFeedbackRecordedBody,
        MessageId::SlashHooksFeedbackHookLine,
        MessageId::SlashHooksFeedbackPolicyKeyLine,
        MessageId::SlashHooksFeedbackPersistedFooter,
        MessageId::SlashHooksFeedbackPersistenceFailedFooter,
        MessageId::SlashHooksFeedbackClearedTitle,
        MessageId::SlashHooksFeedbackClearedBody,
        MessageId::SlashHooksFeedbackClearedFooter,
        MessageId::SlashHooksFeedbackClearFailedFooter,
        MessageId::DebugSessionTitle,
        MessageId::DebugAdapterLine,
        MessageId::DebugProviderCommittedSessionLine,
        MessageId::DebugActiveRunLine,
        MessageId::DebugQueuedRunsLine,
        MessageId::DebugProviderPendingSessionLine,
        MessageId::DebugProviderInitializeSeenLine,
        MessageId::DebugHostExecutedShellResultLine,
        MessageId::DebugSelectedShellExecutionPathLine,
        MessageId::DebugLatestRecoveryStatusLine,
        MessageId::DebugLatestRecoveryReasonLine,
        MessageId::DebugUnknownTargetBody,
        MessageId::DebugUnknownTargetFooter,
        MessageId::CommandRemovedTitle,
        MessageId::ApprovalModeRemovedBody,
        MessageId::ApprovalModeRemovedFooter,
        MessageId::RemovedDecisionCommandBody,
        MessageId::RemovedApprovalDecisionFooter,
        MessageId::RemovedQuestionAnswerFooter,
        MessageId::ModeTitle,
        MessageId::ModesTitle,
        MessageId::ModeApprovalLine,
        MessageId::ModeAnalysisLine,
        MessageId::ModeSummaryFooter,
        MessageId::ModeRemovedTitle,
        MessageId::ModeRemovedBody,
        MessageId::ModeRemovedFooter,
        MessageId::ModeLanguageBody,
        MessageId::ModeLanguageFooter,
        MessageId::ModeUnknownBody,
        MessageId::ModeUnknownFooter,
        MessageId::ApprovalModeTitle,
        MessageId::ApprovalModeSetBody,
        MessageId::ApprovalModeUnknownBody,
        MessageId::ApprovalModeUsageFooter,
        MessageId::ApprovalModeRecommendFooter,
        MessageId::ApprovalModeAutoFooter,
        MessageId::ApprovalModeTrustFooter,
        MessageId::ApprovalModeTrustConfirmationTitle,
        MessageId::ApprovalModeTrustConfirmationBody,
        MessageId::ApprovalModeTrustConfirmationCommandBody,
        MessageId::ApprovalModeTrustConfirmationFooter,
        MessageId::ApprovalModeCardTitle,
        MessageId::ApprovalModeCardCurrentLine,
        MessageId::ApprovalModeCardRecommendLine,
        MessageId::ApprovalModeCardAutoLine,
        MessageId::ApprovalModeCardTrustLine,
        MessageId::ApprovalModeCardFooter,
        MessageId::ApprovalModeRemainsBody,
        MessageId::ApprovalModeCancelBody,
        MessageId::ApprovalModeCancelFooter,
        MessageId::AnalysisModeTitle,
        MessageId::AnalysisModeCurrentBody,
        MessageId::AnalysisModeSetBody,
        MessageId::AnalysisModeUnknownBody,
        MessageId::AnalysisModeUsageFooter,
        MessageId::AnalysisModeSmartFooter,
        MessageId::AnalysisModeAutoFooter,
        MessageId::AnalysisModeManualFooter,
        MessageId::AgentThinking,
        MessageId::AgentThinkingElapsed,
        MessageId::AgentRecoveryTitle,
        MessageId::AgentRecoveryFreshTurnBody,
        MessageId::AgentRecoveryContinuityBody,
        MessageId::AgentStatusTitle,
        MessageId::AgentStillWorking,
        MessageId::AgentStatusFooter,
        MessageId::AgentStatusStarting,
        MessageId::AgentStatusWaitingBackend,
        MessageId::AgentStatusStreaming,
        MessageId::AgentStatusReceivingResponse,
        MessageId::AgentStatusSkill,
        MessageId::AgentStatusLoadingSkill,
        MessageId::AgentStatusApproval,
        MessageId::AgentStatusWaitingApprovalTool,
        MessageId::AgentStatusQuestion,
        MessageId::AgentStatusWaitingUserAnswer,
        MessageId::AgentStatusWaitingApprovalCommand,
        MessageId::AgentStatusTool,
        MessageId::AgentStatusCapturingToolOutput,
        MessageId::AgentStatusToolCompleted,
        MessageId::AgentStatusCompleted,
        MessageId::AgentStatusFailed,
        MessageId::AgentStatusCancelled,
        MessageId::AgentStatusRunningApprovedProviderTool,
        MessageId::AgentStatusSkillFailed,
        MessageId::AgentProviderTimeoutDroppedQueuedBody,
        MessageId::AgentCancellationRequestedTitle,
        MessageId::AgentCancellationRequestedBody,
        MessageId::AgentCancelledReasonLabel,
        MessageId::AgentCancelledUserRequestedReason,
        MessageId::AgentResponseTitle,
        MessageId::AgentGovernanceTitle,
        MessageId::AgentGovernanceStatusLine,
        MessageId::AgentGovernanceReasonLine,
        MessageId::AgentGovernanceSummaryLine,
        MessageId::AgentGovernanceErrorLine,
        MessageId::AgentGovernanceSkillLoadingLine,
        MessageId::AgentGovernanceSkillLoadedLine,
        MessageId::AgentGovernanceSkillFailedLine,
        MessageId::AgentGovernanceToolOutputLine,
        MessageId::AgentGovernanceToolCompletedLine,
        MessageId::AgentGovernanceApprovalRequiredLine,
        MessageId::AgentGovernanceShellCommandSubject,
        MessageId::AgentGovernanceBashCommandSubject,
        MessageId::AgentGovernanceToolSubject,
        MessageId::AgentGovernanceBlockedUserApprovalLine,
        MessageId::AgentGovernanceQuestionLine,
        MessageId::AgentRecommendedCommandsLabel,
        MessageId::InterceptNoticeTitle,
        MessageId::InterceptNoticeBody,
        MessageId::InterceptNoticeFooter,
        MessageId::FailedCommandCardTitle,
        MessageId::FailedCommandCardBody,
        MessageId::FailedCommandCardFooter,
        MessageId::FailedAnalysisCancelledTitle,
        MessageId::FailedAnalysisCancelledBody,
        MessageId::FailedAnalysisCancelNoActiveBody,
        MessageId::FailedAnalysisCancelledFooter,
        MessageId::AnalysisSkippedTitle,
        MessageId::AnalysisSkippedBody,
        MessageId::AnalysisSkippedFooter,
        MessageId::HookAutoAnalyzedTitle,
        MessageId::HookAutoAnalyzedBody,
        MessageId::HookAutoAnalyzedFooter,
        MessageId::AgentQueuedTitle,
        MessageId::AgentQueuedBodyCommand,
        MessageId::AgentQueuedBodyActive,
        MessageId::AgentQueuedFooter,
        MessageId::HookFindingTitle,
        MessageId::HookFindingFooter,
        MessageId::HookFindingMarkdownTitle,
        MessageId::HookFindingMarkdownHookLine,
        MessageId::HookFindingMarkdownSeverityLine,
        MessageId::HookFindingMarkdownFindingLine,
        MessageId::HookFindingMarkdownOutputRefLine,
        MessageId::HookFindingMarkdownSuggestionLine,
        MessageId::HookFindingMarkdownRelatedTitle,
        MessageId::HookFindingMarkdownRelatedLine,
        MessageId::HookFindingMarkdownAgentFollowUpLine,
        MessageId::HookHintTitle,
        MessageId::HookHintNotFoundBody,
        MessageId::HookHintNotFoundFooter,
        MessageId::HookHintNoFindingBody,
        MessageId::HookHintBlockUnavailableBody,
        MessageId::HookHintIgnoredTitle,
        MessageId::HookHintIgnoredBody,
        MessageId::HookHintIgnoredFooter,
        MessageId::HookHintUsageTitle,
        MessageId::HookHintUsageBody,
        MessageId::HookFindingDetailsTitle,
        MessageId::HookConsultationHookLabel,
        MessageId::HookConsultationConfidenceReasonLine,
        MessageId::HookConsultationAnalyzeAction,
        MessageId::HookConsultationIgnoreAction,
        MessageId::HookDetailsConfidenceLine,
        MessageId::HookDetailsUserInterestLine,
        MessageId::HookDetailsReasonLookupIntent,
        MessageId::HookDetailsReasonPipelineIntent,
        MessageId::HookDetailsReasonScriptIntent,
        MessageId::HookDetailsReasonWrapperLowConfidence,
        MessageId::HookDetailsReasonInteractiveIntent,
        MessageId::HookDetailsReasonActiveRunDeferred,
        MessageId::HookDetailsReasonUserContinuedInput,
        MessageId::HookDetailsReasonNonDiagnosticSuccessCommand,
        MessageId::HookDetailsReasonFeedbackNoisy,
        MessageId::HookDetailsReasonIgnoredSameFinding,
        MessageId::HookDetailsReasonSameCardAlreadyRendered,
        MessageId::HookDetailsReasonInterruptionBudget,
        MessageId::HookDetailsReasonLowConfidence,
        MessageId::HookDetailsReasonDiagnosticIntent,
        MessageId::HookDetailsReasonOtherIntent,
        MessageId::HookDetailsTopicLine,
        MessageId::HookDetailsSuppressionKeyLine,
        MessageId::HookDetailsOutputRefLine,
        MessageId::HookDetailsCreatedAtLine,
        MessageId::HookDetailsPromptHintLine,
        MessageId::HookDetailsRecommendedSkillLine,
        MessageId::HookDetailsReadOnlyCliHintLine,
        MessageId::HookDetailsFooter,
        MessageId::RuntimeDetailsUnavailableTitle,
        MessageId::RuntimeDetailsUnavailableBody,
        MessageId::ActivityTitle,
        MessageId::ActivityDetailsTitle,
        MessageId::ActivityRunLabel,
        MessageId::ActivityDetailLabel,
        MessageId::ActivitySkillLabel,
        MessageId::ActivitySkillUpdatedStatus,
        MessageId::ActivityToolLabel,
        MessageId::ActivityToolOutputLabel,
        MessageId::ActivityShellLabel,
        MessageId::ActivityStatusLoading,
        MessageId::ActivityStatusLoaded,
        MessageId::ActivityStatusFailed,
        MessageId::ActivityStatusCalled,
        MessageId::ActivityStatusRequested,
        MessageId::ActivityStatusCaptured,
        MessageId::ActivityStatusCompleted,
        MessageId::ActivityStatusError,
        MessageId::ActivityStatusInterrupted,
        MessageId::ActivitySkillLoadingSummary,
        MessageId::ActivitySkillLoadedSummary,
        MessageId::ActivitySkillFailedSummary,
        MessageId::ActivityToolCalledSummary,
        MessageId::ActivityToolRequestedSummary,
        MessageId::ActivityToolOutputCapturedSummary,
        MessageId::ActivityToolNeedsForegroundShellSummary,
        MessageId::ActivityShellHandoffSentSummary,
        MessageId::MarkdownCodeLabel,
        MessageId::MarkdownCodeWithLanguageLabel,
        MessageId::MarkdownTableLabel,
        MessageId::RecommendationTitle,
        MessageId::RecommendationEmptyBody,
        MessageId::RecommendationFooter,
        MessageId::RecommendationNoSelectableTitle,
        MessageId::RecommendationNoSelectableBody,
        MessageId::RecommendationUnavailableTitle,
        MessageId::RecommendationUnavailableBody,
        MessageId::RecommendationSelectedTitle,
        MessageId::RecommendationSelectedBody,
        MessageId::RecommendationCopiedTitle,
        MessageId::RecommendationCopiedBody,
        MessageId::RecommendationInsertTitle,
        MessageId::RecommendationInsertBody,
        MessageId::RecommendationDetailsTitle,
        MessageId::RecommendationDetailsBody,
        MessageId::RecommendationDisplayOnlyBody,
        MessageId::RecommendationCopyOnlyBody,
        MessageId::RecommendationInsertOnlyBody,
        MessageId::RecommendationDetailsOnlyBody,
        MessageId::ToolOutputStdoutCapturedSummary,
        MessageId::ToolOutputStderrCapturedSummary,
        MessageId::ToolSummaryExit,
        MessageId::ToolSummaryBlocked,
        MessageId::ToolSummaryTimedOut,
        MessageId::ToolSummaryFailed,
        MessageId::QuestionTitle,
        MessageId::QuestionAnswerLabel,
        MessageId::QuestionSelectOneLabel,
        MessageId::QuestionSelectMultipleLabel,
        MessageId::QuestionOtherEmptyLabel,
        MessageId::QuestionOtherValueLabel,
        MessageId::QuestionKeysPrefix,
        MessageId::QuestionInstructionMoveTypeSend,
        MessageId::QuestionInstructionMoveToggleSend,
        MessageId::QuestionInstructionMoveSend,
        MessageId::QuestionInstructionTypeSend,
        MessageId::QuestionInstructionNoAnswer,
        MessageId::QuestionNoPendingTitle,
        MessageId::QuestionNoPendingBody,
        MessageId::ApprovalTitle,
        MessageId::ApprovalRequiredTitle,
        MessageId::ApprovalResolutionApprovedTitle,
        MessageId::ApprovalResolutionAutoApprovedTitle,
        MessageId::ApprovalResolutionTrustedTitle,
        MessageId::ApprovalResolutionDeniedTitle,
        MessageId::ApprovalResolutionCancelledTitle,
        MessageId::ApprovalResolutionBlockedTitle,
        MessageId::ApprovalResolutionDeferredTitle,
        MessageId::ApprovalActionAllowOnce,
        MessageId::ApprovalActionAlwaysTrust,
        MessageId::ApprovalActionDeny,
        MessageId::ApprovalActionDetails,
        MessageId::ApprovalToolInputLabel,
        MessageId::ApprovalCommandLabel,
        MessageId::ApprovalDetailsTitle,
        MessageId::ApprovalDetailsSourceLabel,
        MessageId::ApprovalDetailsRunLabel,
        MessageId::ApprovalDetailsExecutionLabel,
        MessageId::ApprovalDetailsCommandBlockLabel,
        MessageId::ApprovalDetailsRedactionLabel,
        MessageId::ApprovalDetailsProviderRequestLabel,
        MessageId::ApprovalDetailsToolUseLabel,
        MessageId::ApprovalDetailsDefaultDenyLine,
        MessageId::ApprovalDetailsRequestLabel,
        MessageId::ApprovalDetailsInputLabel,
        MessageId::ApprovalDetailsBashCommandSubject,
        MessageId::ApprovalDetailsShellCommandSubject,
        MessageId::ApprovalDetailsToolSubject,
        MessageId::ApprovalDetailsPendingValue,
        MessageId::ApprovalDetailsNoneValue,
        MessageId::ApprovalDetailsNotApplicableValue,
        MessageId::ApprovalJournalTitle,
        MessageId::ApprovalJournalDecisionCount,
        MessageId::ApprovalJournalEmptyBody,
        MessageId::ApprovalJournalActorLabel,
        MessageId::ApprovalJournalPreviewHashLabel,
        MessageId::ApprovalJournalSubjectLabel,
        MessageId::ApprovalJournalPreviewLabel,
        MessageId::ApprovalRiskSuffix,
        MessageId::ApprovalQueueCompactLine,
        MessageId::ApprovalQueueFullLine,
        MessageId::ApprovalQueueNextSuffix,
        MessageId::ApprovalSubjectLabel,
        MessageId::ApprovalNextLabel,
        MessageId::ApprovalKeysPrefix,
        MessageId::ApprovalKeysText,
        MessageId::ApprovalExecutableToolPolicy,
        MessageId::ApprovalExecutableToolPolicyExtra,
        MessageId::ApprovalCommandDefaultPolicy,
        MessageId::ApprovalRunShellCommandPrompt,
        MessageId::ApprovalRunBashCommandPrompt,
        MessageId::ApprovalNotFoundTitle,
        MessageId::ApprovalNotFoundBody,
        MessageId::ApprovalShellHandoffNotFoundTitle,
        MessageId::ApprovalShellHandoffNotFoundBody,
        MessageId::ApprovalShellHandoffBlockedTitle,
        MessageId::ApprovalShellHandoffBlockedFooter,
        MessageId::ApprovalShellHandoffValidationEmptyCommand,
        MessageId::ApprovalShellHandoffValidationMultilineCommand,
        MessageId::ApprovalShellHandoffValidationControlCharacter,
        MessageId::ApprovalShellHandoffValidationEmptyPreview,
        MessageId::ApprovalShellHandoffValidationEmptyApprovalId,
        MessageId::ApprovalShellHandoffValidationEmptyRunId,
        MessageId::ApprovalShellHandoffSendingTitle,
        MessageId::ApprovalShellHandoffSendingBody,
        MessageId::ApprovalShellHandoffTimeoutTitle,
        MessageId::ApprovalShellHandoffTimeoutExceededBody,
        MessageId::ApprovalShellHandoffTimeoutInterruptBody,
        MessageId::ApprovalReceiptKindToolRequest,
        MessageId::ApprovalReceiptKindShellCommandRequest,
        MessageId::ApprovalReceiptKindBashTool,
        MessageId::ApprovalReceiptDecisionPending,
        MessageId::ApprovalReceiptDecisionApproved,
        MessageId::ApprovalReceiptDecisionSentToShell,
        MessageId::ApprovalReceiptDecisionProviderNativeAllowed,
        MessageId::ApprovalReceiptDecisionApprovedDisplayOnly,
        MessageId::ApprovalReceiptDecisionDenied,
        MessageId::ApprovalReceiptDecisionCancelled,
        MessageId::ApprovalReceiptDecisionBlocked,
        MessageId::ApprovalReceiptSubjectBashSentToShell,
        MessageId::ApprovalReceiptSubjectBashProviderNative,
        MessageId::ApprovalReceiptBashSentToShellMessage,
        MessageId::ApprovalReceiptProviderNativeAllowedMessage,
    ];
}

#[derive(Debug, Clone, Copy)]
pub struct I18n {
    language: Language,
}

impl I18n {
    pub fn new(language: Language) -> Self {
        Self { language }
    }

    pub fn t(&self, id: MessageId) -> &'static str {
        message(self.language, id)
    }

    pub fn format(&self, id: MessageId, args: &[(&str, &str)]) -> String {
        let mut text = self.t(id).to_string();
        for (key, value) in args {
            text = text.replace(&format!("{{{key}}}"), value);
        }
        text
    }
}

fn message(language: Language, id: MessageId) -> &'static str {
    match language {
        Language::EnUs => en_message(id),
        Language::ZhCn => zh_message(id),
    }
}

fn en_message(id: MessageId) -> &'static str {
    match id {
        MessageId::StartupTitle => "cosh-shell",
        MessageId::StartupAdapterLine => "Adapter: {adapter} · Shell: {shell} · Mode: {mode}",
        MessageId::StartupCwdLine => "cwd: {cwd}",
        MessageId::StartupCommandsLine => "/help · /mode · /hooks",
        MessageId::StartupHooksNoneSummary => "Startup hooks: none configured.",
        MessageId::StartupHooksCompletedSummary => {
            "Startup hooks: built-in read-only checks completed."
        }
        MessageId::StartupHooksFindingsHeading => "Startup findings",
        MessageId::StartupHooksRustProjectFinding => {
            "Rust project detected from `Cargo.toml`; `/skill` can show project-oriented Agent capabilities."
        }
        MessageId::StartupHooksNoFindings => {
            "No startup findings from built-in read-only checks."
        }
        MessageId::StartupHooksReadOnlyNote => {
            "`cosh-shell` only inspected lightweight startup context."
        }
        MessageId::HelpTitle => "Slash commands",
        MessageId::HelpFooter => "Mode: {mode}. Strategy: {strategy}.",
        MessageId::HelpGroupConfig => "Config",
        MessageId::HelpGroupModes => "Modes",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "show command reference",
        MessageId::HelpSummaryConfig => "configure UI language",
        MessageId::HelpSummaryModeApproval => "change approval mode",
        MessageId::HelpSummaryModeAnalysis => "change analysis strategy",
        MessageId::HelpSummaryAgent => "start an explicit Agent request",
        MessageId::HelpSummaryExplain => "analyze the last failed command",
        MessageId::HelpSummaryCancel => "cancel active Agent work",
        MessageId::HelpSummaryDetails => "inspect approval/activity details",
        MessageId::HelpSummaryAudit => "show audit entry points",
        MessageId::HelpSummaryHooks => "show hook status",
        MessageId::HelpSummarySelect => "show a displayed recommendation",
        MessageId::HelpSummaryCopy => "copy a displayed recommendation",
        MessageId::HelpSummaryDebug => "show session debug details",
        MessageId::HelpSummaryClear => "clear local shell state",
        MessageId::HelpSummaryShell => "return to shell input",
        MessageId::HelpSummarySkill => "show skill routing hints",
        MessageId::HelpSummaryApprovalModeRemoved => "removed approval-mode alias",
        MessageId::SlashHintTitle => "Slash command hint",
        MessageId::SlashHintPrefix => "Prefix: {prefix}",
        MessageId::SlashHintCurrentMode => "Current mode: {mode}",
        MessageId::SlashHintFooter => {
            "Type a full command and press Enter; paths like /tmp/foo stay in shell."
        }
        MessageId::SlashUnknownTitle => "Slash command",
        MessageId::SlashUnknownBody => "Unknown slash command: {command}",
        MessageId::SlashUnknownSuggestionBody => "Did you mean {command}?",
        MessageId::SlashUnknownFooter => "Use /help to see available commands.",
        MessageId::SlashInfoAuditTitle => "Audit",
        MessageId::SlashInfoAuditApprovalsBody => {
            "Approval decisions are available with Details actions."
        }
        MessageId::SlashInfoAuditActivityBody => {
            "Activity output refs are available with Details actions."
        }
        MessageId::SlashInfoAuditFooter => "Audit views are read-only; no shell command runs.",
        MessageId::SlashInfoConfigTitle => "Config",
        MessageId::SlashInfoConfigLanguageLine => "language: {effective} source: {source}",
        MessageId::SlashInfoConfigLanguageEffectiveLine => {
            "language: {effective} effective, setting: {setting}, source: {source}"
        }
        MessageId::SlashInfoConfigPathLine => "config: {path}",
        MessageId::SlashInfoConfigDebugActivityLine => {
            "debug activity: {state} (ui.debug or COSH_SHELL_DEBUG=1)"
        }
        MessageId::SlashInfoConfigAnalysisStrategyLine => {
            "analysis strategy: /mode analysis smart|auto|manual"
        }
        MessageId::SlashInfoConfigRenderFallbackLine => {
            "render fallback: set COSH_SHELL_RENDER=plain before starting cosh-shell."
        }
        MessageId::SlashInfoConfigFooter => {
            "Use /config language [auto|en-US|zh-CN]. Saved language takes effect next startup."
        }
        MessageId::SlashInfoSkillTitle => "Skill",
        MessageId::SlashInfoSkillHookRoutingBody => {
            "Hook routing hints can route Agent analysis toward a skill."
        }
        MessageId::SlashInfoSkillRegistryBody => {
            "No external skill registry is configured for this shell session."
        }
        MessageId::SlashInfoSkillFooter => {
            "Skill hooks are advisory and still go through governance."
        }
        MessageId::ConfigInvalidLanguageBody => "Invalid language: {language}",
        MessageId::ConfigSupportedLanguagesFooter => "Supported: auto, en-US, zh-CN.",
        MessageId::ConfigUnknownKeyBody => "Unknown config key: {key}",
        MessageId::ConfigHomeMissingBody => "HOME is not set; cannot persist config.",
        MessageId::ConfigHomeMissingFooter => "Set HOME or edit config manually.",
        MessageId::ConfigUnchangedTitle => "Config unchanged",
        MessageId::ConfigNoFileChangedBody => "No config file was changed.",
        MessageId::ConfigSavedTitle => "Config saved",
        MessageId::ConfigSavedValueLine => "Saved ui.{setting} = \"{value}\".",
        MessageId::ConfigCurrentSessionLanguageLine => "Current session language: {language}.",
        MessageId::ConfigSavedFooter => "Saved setting takes effect next startup.",
        MessageId::ConfigSaveFailedTitle => "Config save failed",
        MessageId::ConfigSaveFailedBody => "Config save failed: {error}",
        MessageId::ConfigSavePromptTitle => "Save config?",
        MessageId::ConfigFileLine => "file: {path}",
        MessageId::ConfigPendingChangeLine => "ui.{setting}: {before} -> {after}",
        MessageId::ConfigSaveButton => "Save",
        MessageId::ConfigCancelButton => "Cancel",
        MessageId::ConfigApplyKeysFooter => "Keys: Left/Right select | Enter apply | Esc cancel",
        MessageId::ConfigLanguageTitle => "Language",
        MessageId::ConfigLanguageAutoLine => "auto    Follow LC_ALL/LC_MESSAGES/LANG",
        MessageId::ConfigLanguageEnLine => "en-US   English",
        MessageId::ConfigLanguageZhLine => "zh-CN   Simplified Chinese",
        MessageId::ConfigLanguageKeysFooter => {
            "Keys: Left/Right select | Enter choose | Esc cancel"
        }
        MessageId::SlashHooksRegisteredTitle => "Hook status",
        MessageId::SlashHooksNoHooksBody => "No hooks registered.",
        MessageId::SlashHooksStatusCountLine => {
            "Registered: {total}; enabled: {enabled}; disabled: {disabled}."
        }
        MessageId::SlashHooksStatusSourcesLine => {
            "Sources: builtin={builtin}; user={user}; project={project}."
        }
        MessageId::SlashHooksStatusProjectTrustLine => {
            "Project trust: trusted={trusted}; untrusted={untrusted}."
        }
        MessageId::SlashHooksFooterCount => "{count} hook(s) registered.",
        MessageId::SlashHooksFooterMutedTargets => {
            "{count} hook(s) registered. Muted targets: {targets}."
        }
        MessageId::SlashHooksTargetMutedTitle => "Hook target muted",
        MessageId::SlashHooksTargetMutedBody => "Muted hook target '{target}' for this session.",
        MessageId::SlashHooksTargetMutedFooter => {
            "Muted findings are still recorded in /hooks history."
        }
        MessageId::SlashHooksTargetUnmutedTitle => "Hook target unmuted",
        MessageId::SlashHooksTargetUnmutedBody => "Unmuted hook target '{target}'.",
        MessageId::SlashHooksTargetNotMutedBody => "Hook target '{target}' was not muted.",
        MessageId::SlashHooksEnabledTitle => "Hook enabled",
        MessageId::SlashHooksEnabledBody => "Hook '{id}' enabled.",
        MessageId::SlashHooksDisabledTitle => "Hook disabled",
        MessageId::SlashHooksDisabledBody => "Hook '{id}' disabled.",
        MessageId::SlashHooksHistoryTitle => "Hook history",
        MessageId::SlashHooksHistoryEmptyBody => "No hook findings recorded in this session.",
        MessageId::SlashHooksHistoryFooter => {
            "Recent findings are read-only; Analyze still requires user confirmation."
        }
        MessageId::SlashHooksEventsTitle => "Hook display events",
        MessageId::SlashHooksEventsEmptyBody => "No hook display events recorded in this session.",
        MessageId::SlashHooksEventsFooter => {
            "Events are session-local and contain policy metadata, not command output."
        }
        MessageId::SlashHooksUsageTitle => "Usage",
        MessageId::SlashHooksUsageListLine => "/hooks                - show hook status",
        MessageId::SlashHooksUsageHistoryLine => {
            "/hooks history        - show recent hook findings"
        }
        MessageId::SlashHooksUsageEventsLine => {
            "/hooks events         - show recent hook display events"
        }
        MessageId::SlashHooksUsageAnalyzeLine => {
            "/hooks analyze <id>   - analyze a hint finding"
        }
        MessageId::SlashHooksUsageIgnoreLine => {
            "/hooks ignore <id>    - ignore a hint finding"
        }
        MessageId::SlashHooksUsageDetailsLine => {
            "/hooks details <id>   - show hook finding details"
        }
        MessageId::SlashHooksUsageFeedbackLine => {
            "/hooks feedback noisy|useful <id> - record feedback"
        }
        MessageId::SlashHooksUsageClearFeedbackLine => {
            "/hooks clear-feedback - clear hook feedback preferences"
        }
        MessageId::SlashHooksUsageMuteLine => "/hooks mute <target>  - mute a topic or hook id",
        MessageId::SlashHooksUsageUnmuteLine => {
            "/hooks unmute <target>- unmute a topic or hook id"
        }
        MessageId::SlashHooksUsageTrustProjectLine => {
            "/hooks trust-project  - trust project hooks for this session"
        }
        MessageId::SlashHooksUsageUntrustProjectLine => {
            "/hooks untrust-project- untrust project hooks for this session"
        }
        MessageId::SlashHooksUsageClearProjectTrustLine => {
            "/hooks clear-project-trust - clear project hook trust store"
        }
        MessageId::SlashHooksUsageEnableLine => "/hooks enable <id>    - enable a hook",
        MessageId::SlashHooksUsageDisableLine => "/hooks disable <id>   - disable a hook",
        MessageId::SlashHooksProjectTrustedTitle => "Project hooks trusted",
        MessageId::SlashHooksProjectUntrustedTitle => "Project hooks untrusted",
        MessageId::SlashHooksProjectTrustNoHooksBody => {
            "No project hooks are registered in this session."
        }
        MessageId::SlashHooksProjectTrustedBody => "{count} project hook(s) marked trusted.",
        MessageId::SlashHooksProjectUntrustedBody => "{count} project hook(s) marked untrusted.",
        MessageId::SlashHooksProjectTrustNoChangeFooter => "No trust state changed.",
        MessageId::SlashHooksProjectTrustPersistedFooter => {
            "Trust persisted; disabled hooks remain disabled."
        }
        MessageId::SlashHooksProjectTrustRemovedFooter => {
            "Trust removed from persistent store; disabled hooks remain disabled."
        }
        MessageId::SlashHooksProjectTrustPersistenceFailedFooter => {
            "Session state changed, but persistence failed: {failures}"
        }
        MessageId::SlashHooksProjectTrustClearedTitle => "Project hook trust cleared",
        MessageId::SlashHooksProjectTrustClearedBody => {
            "{count} project hook(s) marked untrusted."
        }
        MessageId::SlashHooksProjectTrustClearedFooter => {
            "Project hook trust store cleared; current session project hooks are untrusted."
        }
        MessageId::SlashHooksProjectTrustClearFailedFooter => {
            "Current session project hooks marked untrusted, but clearing persistent trust store failed: {error}"
        }
        MessageId::SlashHooksFeedbackUsageBody => "/hooks feedback noisy|useful <finding_id>",
        MessageId::SlashHooksFeedbackTitle => "Hook feedback",
        MessageId::SlashHooksFeedbackFindingNotFoundBody => {
            "Finding '{finding_id}' was not found in this session."
        }
        MessageId::SlashHooksFeedbackFindingNotFoundFooter => {
            "Use /hooks history to copy a recent finding id."
        }
        MessageId::SlashHooksFeedbackRecordedTitle => "Hook feedback recorded",
        MessageId::SlashHooksFeedbackRecordedBody => {
            "Feedback '{feedback}' recorded for finding '{finding_id}'."
        }
        MessageId::SlashHooksFeedbackHookLine => "Hook: {hook_id}.",
        MessageId::SlashHooksFeedbackPolicyKeyLine => "Policy key: {key}.",
        MessageId::SlashHooksFeedbackPersistedFooter => {
            "Feedback persisted. It affects display strategy only."
        }
        MessageId::SlashHooksFeedbackPersistenceFailedFooter => {
            "Session feedback recorded, but persistence failed: {error}"
        }
        MessageId::SlashHooksFeedbackClearedTitle => "Hook feedback cleared",
        MessageId::SlashHooksFeedbackClearedBody => {
            "{count} feedback preference(s) cleared from this session."
        }
        MessageId::SlashHooksFeedbackClearedFooter => "Hook feedback preferences cleared.",
        MessageId::SlashHooksFeedbackClearFailedFooter => {
            "Session feedback cleared, but persistent store clear failed: {error}"
        }
        MessageId::DebugSessionTitle => "Session debug",
        MessageId::DebugAdapterLine => "adapter: {value}",
        MessageId::DebugProviderCommittedSessionLine => "provider committed session: {value}",
        MessageId::DebugActiveRunLine => "active run: {value}",
        MessageId::DebugQueuedRunsLine => "queued runs: {value}",
        MessageId::DebugProviderPendingSessionLine => "provider pending session: {value}",
        MessageId::DebugProviderInitializeSeenLine => "provider initialize seen: {value}",
        MessageId::DebugHostExecutedShellResultLine => "host-executed shell result: {value}",
        MessageId::DebugSelectedShellExecutionPathLine => "selected shell execution path: {value}",
        MessageId::DebugLatestRecoveryStatusLine => "latest recovery status: {value}",
        MessageId::DebugLatestRecoveryReasonLine => "latest recovery reason: {value}",
        MessageId::DebugUnknownTargetBody => "Unknown debug target: {target}",
        MessageId::DebugUnknownTargetFooter => "Use /debug session.",
        MessageId::CommandRemovedTitle => "Command removed",
        MessageId::ApprovalModeRemovedBody => "/approval-mode is not supported.",
        MessageId::ApprovalModeRemovedFooter => "Use /mode approval [recommend|auto|trust].",
        MessageId::RemovedDecisionCommandBody => "{command} is no longer a supported input command.",
        MessageId::RemovedApprovalDecisionFooter => {
            "Use the approval card buttons instead; nothing was sent to the shell."
        },
        MessageId::RemovedQuestionAnswerFooter => {
            "Answer from the question card instead; nothing was sent to the shell."
        },
        MessageId::ModeTitle => "Mode",
        MessageId::ModesTitle => "Modes",
        MessageId::ModeApprovalLine => "approval: {mode}",
        MessageId::ModeAnalysisLine => "analysis: {mode}",
        MessageId::ModeSummaryFooter => {
            "Use /mode approval [recommend|auto|trust] or /mode analysis [smart|auto|manual]."
        }
        MessageId::ModeRemovedTitle => "Mode command removed",
        MessageId::ModeRemovedBody => "/mode {mode} is not supported.",
        MessageId::ModeRemovedFooter => "Use /mode approval {mode}.",
        MessageId::ModeLanguageBody => "Language is persistent config, not a runtime mode.",
        MessageId::ModeLanguageFooter => "Use /config language [auto|en-US|zh-CN].",
        MessageId::ModeUnknownBody => "Unknown mode: {mode}",
        MessageId::ModeUnknownFooter => {
            "Use /mode approval recommend|auto|trust or /mode analysis smart|auto|manual."
        }
        MessageId::ApprovalModeTitle => "Approval mode",
        MessageId::ApprovalModeSetBody => "Mode set to {mode}.",
        MessageId::ApprovalModeUnknownBody => "Unknown approval mode: {mode}",
        MessageId::ApprovalModeUsageFooter => "Use /mode approval recommend|auto|trust.",
        MessageId::ApprovalModeRecommendFooter => {
            "Agent explains and suggests; no tool calls are emitted."
        }
        MessageId::ApprovalModeAutoFooter => {
            "Read-only tools auto-approved; risky requests need confirmation."
        }
        MessageId::ApprovalModeTrustFooter => {
            "All tools auto-approved; audit trail preserved via control protocol."
        }
        MessageId::ApprovalModeTrustConfirmationTitle => "Trust confirmation required",
        MessageId::ApprovalModeTrustConfirmationBody => {
            "Trust mode auto-approves provider tool requests for this session."
        }
        MessageId::ApprovalModeTrustConfirmationCommandBody => {
            "Run /mode approval trust confirm to enable it explicitly."
        }
        MessageId::ApprovalModeTrustConfirmationFooter => {
            "Recommend or auto mode remains active until confirmation."
        }
        MessageId::ApprovalModeCardTitle => "User mode",
        MessageId::ApprovalModeCardCurrentLine => "Current: {mode}",
        MessageId::ApprovalModeCardRecommendLine => {
            "{marker}[ recommend ] Explain and suggest only"
        }
        MessageId::ApprovalModeCardAutoLine => {
            "{marker}[ auto      ] Read-only auto-approved; risky needs confirmation"
        }
        MessageId::ApprovalModeCardTrustLine => {
            "{marker}[ trust     ] All tools auto-approved with audit trail"
        }
        MessageId::ApprovalModeCardFooter => "Keys: Left/Right select | Enter apply | Esc cancel",
        MessageId::ApprovalModeRemainsBody => "Mode remains {mode}.",
        MessageId::ApprovalModeCancelBody => "Mode unchanged: {mode}.",
        MessageId::ApprovalModeCancelFooter => "No shell command ran.",
        MessageId::AnalysisModeTitle => "Analysis mode",
        MessageId::AnalysisModeCurrentBody => "Current: {mode}",
        MessageId::AnalysisModeSetBody => "Mode set to {mode}.",
        MessageId::AnalysisModeUnknownBody => "Unknown analysis mode: {mode}",
        MessageId::AnalysisModeUsageFooter => "Use /mode analysis smart|auto|manual.",
        MessageId::AnalysisModeSmartFooter => {
            "Hooks evaluate on failure; findings shown for review."
        }
        MessageId::AnalysisModeAutoFooter => {
            "Hooks evaluate on failure; Agent auto-triggered for failed commands."
        }
        MessageId::AnalysisModeManualFooter => {
            "Hooks and automatic analysis disabled; use slash commands to trigger."
        }
        MessageId::AgentThinking => "Thinking...",
        MessageId::AgentThinkingElapsed => "Thinking... {elapsed}s · {detail}",
        MessageId::AgentRecoveryTitle => "Agent recovery",
        MessageId::AgentRecoveryFreshTurnBody => {
            "Using a fresh provider turn for shell evidence recovery."
        }
        MessageId::AgentRecoveryContinuityBody => "Provider session continuity may be degraded.",
        MessageId::AgentStatusTitle => "Agent",
        MessageId::AgentStillWorking => "Still working... {elapsed}s · {detail}",
        MessageId::AgentStatusFooter => "Ctrl+C cancels · [Cancel]",
        MessageId::AgentStatusStarting => "starting",
        MessageId::AgentStatusWaitingBackend => "waiting for Agent backend",
        MessageId::AgentStatusStreaming => "streaming",
        MessageId::AgentStatusReceivingResponse => "receiving Agent response",
        MessageId::AgentStatusSkill => "skill",
        MessageId::AgentStatusLoadingSkill => "loading skill {skill}",
        MessageId::AgentStatusApproval => "approval",
        MessageId::AgentStatusWaitingApprovalTool => "waiting for approval: tool {tool}",
        MessageId::AgentStatusQuestion => "question",
        MessageId::AgentStatusWaitingUserAnswer => "waiting for user answer: {question}",
        MessageId::AgentStatusWaitingApprovalCommand => "waiting for approval: {command}",
        MessageId::AgentStatusTool => "tool",
        MessageId::AgentStatusCapturingToolOutput => "capturing output from {tool_id}",
        MessageId::AgentStatusToolCompleted => "{tool_id} completed with status {status}",
        MessageId::AgentStatusCompleted => "completed",
        MessageId::AgentStatusFailed => "failed",
        MessageId::AgentStatusCancelled => "cancelled",
        MessageId::AgentStatusRunningApprovedProviderTool => "running approved provider tool",
        MessageId::AgentStatusSkillFailed => "{skill} failed: {error}",
        MessageId::AgentProviderTimeoutDroppedQueuedBody => {
            "{dropped} queued requests skipped after provider timeout"
        }
        MessageId::AgentCancellationRequestedTitle => "Agent cancellation requested",
        MessageId::AgentCancellationRequestedBody => "Stopping active Agent run...",
        MessageId::AgentCancelledReasonLabel => "Reason:",
        MessageId::AgentCancelledUserRequestedReason => "user requested cancellation",
        MessageId::AgentResponseTitle => "Agent",
        MessageId::AgentGovernanceTitle => "Governance",
        MessageId::AgentGovernanceStatusLine => "Status: {phase}",
        MessageId::AgentGovernanceReasonLine => "Reason: {reason}",
        MessageId::AgentGovernanceSummaryLine => "Summary: {summary}",
        MessageId::AgentGovernanceErrorLine => "Error: {error}",
        MessageId::AgentGovernanceSkillLoadingLine => "Skill loading: {skill}",
        MessageId::AgentGovernanceSkillLoadedLine => "Skill loaded: {skill}",
        MessageId::AgentGovernanceSkillFailedLine => "Skill failed: {skill}",
        MessageId::AgentGovernanceToolOutputLine => "Tool output: {tool_id} {stream}",
        MessageId::AgentGovernanceToolCompletedLine => "Tool completed: {tool_id}",
        MessageId::AgentGovernanceApprovalRequiredLine => "Approval required: {subject}",
        MessageId::AgentGovernanceShellCommandSubject => "Shell command",
        MessageId::AgentGovernanceBashCommandSubject => "Bash command",
        MessageId::AgentGovernanceToolSubject => "{tool} tool",
        MessageId::AgentGovernanceBlockedUserApprovalLine => "Blocked: user approval required",
        MessageId::AgentGovernanceQuestionLine => "Question: {question}",
        MessageId::AgentRecommendedCommandsLabel => "recommended commands:",
        MessageId::InterceptNoticeTitle => "AI request",
        MessageId::InterceptNoticeBody => "Sending input to Agent: {input}",
        MessageId::InterceptNoticeFooter => "Shell input was intercepted before Bash ran it.",
        MessageId::FailedCommandCardTitle => "Command failed",
        MessageId::FailedCommandCardBody => {
            "`{command}` exited with code {exit_code}; id: {id}"
        }
        MessageId::FailedCommandCardFooter => "[Analyze] [Dismiss] [Details]",
        MessageId::FailedAnalysisCancelledTitle => "Agent cancelled",
        MessageId::FailedAnalysisCancelledBody => "cancelled pending analysis for `{command}`",
        MessageId::FailedAnalysisCancelNoActiveBody => {
            "no active Agent run is currently waiting for cancellation"
        }
        MessageId::FailedAnalysisCancelledFooter => "Shell remains active.",
        MessageId::AnalysisSkippedTitle => "Analysis skipped",
        MessageId::AnalysisSkippedBody => "skipped repeated failure analysis for `{command}`",
        MessageId::AnalysisSkippedFooter => {
            "Too many consecutive failures for this command. Wait before retrying."
        }
        MessageId::HookAutoAnalyzedTitle => "Hook auto-analyzed",
        MessageId::HookAutoAnalyzedBody => "`{command}` exited with code {exit_code}",
        MessageId::HookAutoAnalyzedFooter => "Agent analysis is starting.",
        MessageId::AgentQueuedTitle => "Agent queued",
        MessageId::AgentQueuedBodyCommand => "Captured failed command: {command}",
        MessageId::AgentQueuedBodyActive => "Current Agent run is still streaming.",
        MessageId::AgentQueuedFooter => {
            "This failure will be analyzed after the current Agent run finishes."
        }
        MessageId::HookFindingTitle => "Hook finding",
        MessageId::HookFindingFooter => "Use /hooks analyze|ignore|details {hint_id}.",
        MessageId::HookFindingMarkdownTitle => "Command hook finding",
        MessageId::HookFindingMarkdownHookLine => "- Hook: `{hook_id}`.",
        MessageId::HookFindingMarkdownSeverityLine => "- Severity: `{severity}`.",
        MessageId::HookFindingMarkdownFindingLine => "- Finding: {finding}.",
        MessageId::HookFindingMarkdownOutputRefLine => "- Output id: `{output_ref}`.",
        MessageId::HookFindingMarkdownSuggestionLine => "- Suggestion: {suggestion}.",
        MessageId::HookFindingMarkdownRelatedTitle => "- Related findings:",
        MessageId::HookFindingMarkdownRelatedLine => "  - `{hook_id}` [{severity}]: {finding}",
        MessageId::HookFindingMarkdownAgentFollowUpLine => {
            "Agent follow-up must use bounded cosh-shell evidence before claiming details."
        }
        MessageId::HookHintTitle => "Hook hint",
        MessageId::HookHintNotFoundBody => "Hook hint '{hint_id}' was not found in this session.",
        MessageId::HookHintNotFoundFooter => "Use /hooks history to copy a recent finding id.",
        MessageId::HookHintNoFindingBody => "Hook hint '{hint_id}' has no finding attached.",
        MessageId::HookHintBlockUnavailableBody => {
            "Command block '{block_id}' is no longer available."
        }
        MessageId::HookHintIgnoredTitle => "Hook hint ignored",
        MessageId::HookHintIgnoredBody => "Ignored hook hint '{hint_id}' for this session.",
        MessageId::HookHintIgnoredFooter => "Future matching findings are downgraded by policy.",
        MessageId::HookHintUsageTitle => "Usage",
        MessageId::HookHintUsageBody => "/hooks analyze|ignore|details <hint_id>",
        MessageId::HookFindingDetailsTitle => "Hook finding details",
        MessageId::HookConsultationHookLabel => "Hook",
        MessageId::HookConsultationConfidenceReasonLine => {
            "Confidence: {confidence}; reason: {reason}"
        }
        MessageId::HookConsultationAnalyzeAction => "Analyze",
        MessageId::HookConsultationIgnoreAction => "Ignore",
        MessageId::HookDetailsConfidenceLine => "Confidence: {confidence}; policy reason: {reason}",
        MessageId::HookDetailsUserInterestLine => "User-interest reason: {code}: {description}",
        MessageId::HookDetailsReasonLookupIntent => {
            "the command targets a specific process or search, so the finding stays low-interruption"
        }
        MessageId::HookDetailsReasonPipelineIntent => {
            "the command pipeline may have transformed output, so missing or uncertain schema is not treated as high-confidence"
        }
        MessageId::HookDetailsReasonScriptIntent => {
            "script or batch output may not reflect the user's immediate focus, so interruption is reduced"
        }
        MessageId::HookDetailsReasonWrapperLowConfidence => {
            "wrapper or remote/container context makes the target view ambiguous, so verification is required"
        }
        MessageId::HookDetailsReasonInteractiveIntent => {
            "interactive output is not a stable diagnostic snapshot, so only sampling guidance is shown"
        }
        MessageId::HookDetailsReasonActiveRunDeferred => {
            "another Agent run was active, so this success-command finding waits and is rechecked before display"
        }
        MessageId::HookDetailsReasonUserContinuedInput => {
            "the user moved on to another input, so this success-command finding does not interrupt"
        }
        MessageId::HookDetailsReasonNonDiagnosticSuccessCommand => {
            "the command does not look like an explicit diagnostic snapshot, so interruption is reduced"
        }
        MessageId::HookDetailsReasonFeedbackNoisy => {
            "prior user feedback says similar findings are noisy, so interruption is reduced"
        }
        MessageId::HookDetailsReasonIgnoredSameFinding => {
            "the user ignored a matching finding earlier in this session"
        }
        MessageId::HookDetailsReasonSameCardAlreadyRendered => {
            "an equal-or-higher severity card was already shown for this finding key"
        }
        MessageId::HookDetailsReasonInterruptionBudget => {
            "recent similar cards already used the session interruption budget"
        }
        MessageId::HookDetailsReasonLowConfidence => {
            "partial evidence requires read-only verification before stronger claims"
        }
        MessageId::HookDetailsReasonDiagnosticIntent => {
            "explicit diagnostic command with sufficient evidence"
        }
        MessageId::HookDetailsReasonOtherIntent => "no explicit diagnostic intent was identified",
        MessageId::HookDetailsTopicLine => "Topic: {topic}; entity: {entity}",
        MessageId::HookDetailsSuppressionKeyLine => "Suppression key: {key}",
        MessageId::HookDetailsOutputRefLine => "Output capture: {ref}",
        MessageId::HookDetailsCreatedAtLine => "Created at: {created_at}",
        MessageId::HookDetailsPromptHintLine => "Prompt hint: {hint}",
        MessageId::HookDetailsRecommendedSkillLine => "Recommended skill: {skill}",
        MessageId::HookDetailsReadOnlyCliHintLine => "Read-only CLI hint: {hint}",
        MessageId::HookDetailsFooter => "Analyze still requires confirmation.",
        MessageId::RuntimeDetailsUnavailableTitle => "Details unavailable",
        MessageId::RuntimeDetailsUnavailableBody => {
            "{id} is not available; use a Details action with an approval or activity id"
        }
        MessageId::ActivityTitle => "Activity",
        MessageId::ActivityDetailsTitle => "Activity details",
        MessageId::ActivityRunLabel => "Run",
        MessageId::ActivityDetailLabel => "Detail",
        MessageId::ActivitySkillLabel => "Skill",
        MessageId::ActivitySkillUpdatedStatus => "updated",
        MessageId::ActivityToolLabel => "Tool",
        MessageId::ActivityToolOutputLabel => "Tool output",
        MessageId::ActivityShellLabel => "Shell",
        MessageId::ActivityStatusLoading => "loading",
        MessageId::ActivityStatusLoaded => "loaded",
        MessageId::ActivityStatusFailed => "failed",
        MessageId::ActivityStatusCalled => "called",
        MessageId::ActivityStatusRequested => "requested",
        MessageId::ActivityStatusCaptured => "captured",
        MessageId::ActivityStatusCompleted => "completed",
        MessageId::ActivityStatusError => "error",
        MessageId::ActivityStatusInterrupted => "interrupted",
        MessageId::ActivitySkillLoadingSummary => "{skill} loading",
        MessageId::ActivitySkillLoadedSummary => "{skill} loaded",
        MessageId::ActivitySkillFailedSummary => "{skill} failed",
        MessageId::ActivityToolCalledSummary => "{tool} called; [Details] {id}",
        MessageId::ActivityToolRequestedSummary => "{tool} requested; [Details] {id}",
        MessageId::ActivityToolOutputCapturedSummary => "{stream} captured; [Details] {id}",
        MessageId::ActivityProviderNativeShellBypassSummary => {
            "{tool} auto-approved by provider; [Details] {id}"
        }
        MessageId::ActivityToolNeedsForegroundShellSummary => {
            "may require foreground shell; [Send to shell] {handoff}; [Details] {id}"
        }
        MessageId::ActivityShellHandoffSentSummary => "{approval} sent to shell",
        MessageId::MarkdownCodeLabel => "code",
        MessageId::MarkdownCodeWithLanguageLabel => "code: {language}",
        MessageId::MarkdownTableLabel => "table",
        MessageId::RecommendationTitle => "Recommendations",
        MessageId::RecommendationEmptyBody => "No command recommendations",
        MessageId::RecommendationFooter => "[Copy] [Insert] [Details] - display-only",
        MessageId::RecommendationNoSelectableTitle => "No selectable recommendation",
        MessageId::RecommendationNoSelectableBody => {
            "No selectable recommendation is available yet"
        }
        MessageId::RecommendationUnavailableTitle => "Recommendation unavailable",
        MessageId::RecommendationUnavailableBody => {
            "Recommendation {index} is not available; choose 1..{total}"
        }
        MessageId::RecommendationSelectedTitle => "Recommendation selected",
        MessageId::RecommendationSelectedBody => "Selected recommendation {index}",
        MessageId::RecommendationCopiedTitle => "Recommendation copy",
        MessageId::RecommendationCopiedBody => "Copy recommendation {index}",
        MessageId::RecommendationInsertTitle => "Recommendation insert",
        MessageId::RecommendationInsertBody => "Prepared recommendation {index} for manual input",
        MessageId::RecommendationDetailsTitle => "Recommendation details",
        MessageId::RecommendationDetailsBody => "Details for recommendation {index}",
        MessageId::RecommendationDisplayOnlyBody => {
            "Display-only: command was not executed; copy or re-enter it to run"
        }
        MessageId::RecommendationCopyOnlyBody => {
            "Copy-only: command was shown for copying; it was not executed."
        }
        MessageId::RecommendationInsertOnlyBody => {
            "Insert is pending editable input only; nothing was submitted or written to the child shell."
        }
        MessageId::RecommendationDetailsOnlyBody => {
            "Details-only: inspect the command before deciding whether to type or copy it."
        }
        MessageId::ToolOutputStdoutCapturedSummary => "stdout captured; [Details] {id}",
        MessageId::ToolOutputStderrCapturedSummary => "stderr captured; [Details] {id}",
        MessageId::ToolSummaryExit => "exit {exit}",
        MessageId::ToolSummaryBlocked => "tool request blocked by shell broker guard",
        MessageId::ToolSummaryTimedOut => "tool request timed out",
        MessageId::ToolSummaryFailed => "tool request failed",
        MessageId::QuestionTitle => "Agent question",
        MessageId::QuestionAnswerLabel => "Answer",
        MessageId::QuestionSelectOneLabel => "Select one:",
        MessageId::QuestionSelectMultipleLabel => "Select one or more:",
        MessageId::QuestionOtherEmptyLabel => "Other...",
        MessageId::QuestionOtherValueLabel => "Other: {answer}",
        MessageId::QuestionKeysPrefix => "Keys: ",
        MessageId::QuestionInstructionMoveTypeSend => "Left/Right move | type answer | Enter send",
        MessageId::QuestionInstructionMoveToggleSend => {
            "Left/Right move | Space toggle | Enter send"
        }
        MessageId::QuestionInstructionMoveSend => "Left/Right move | Enter send",
        MessageId::QuestionInstructionTypeSend => "Type answer | Enter send",
        MessageId::QuestionInstructionNoAnswer => "No selectable answer is available.",
        MessageId::QuestionNoPendingTitle => "No pending question",
        MessageId::QuestionNoPendingBody => "There is no Agent question waiting for an answer.",
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
    }
}

fn zh_message(id: MessageId) -> &'static str {
    match id {
        MessageId::StartupTitle => "cosh-shell",
        MessageId::StartupAdapterLine => "后端: {adapter} · Shell: {shell} · 模式: {mode}",
        MessageId::StartupCwdLine => "cwd: {cwd}",
        MessageId::StartupCommandsLine => "/help · /mode · /hooks",
        MessageId::StartupHooksNoneSummary => "启动 hooks: 未配置。",
        MessageId::StartupHooksCompletedSummary => "启动 hooks: 内置只读检查已完成。",
        MessageId::StartupHooksFindingsHeading => "启动检查结果",
        MessageId::StartupHooksRustProjectFinding => {
            "检测到 `Cargo.toml` Rust 项目；`/skill` 可查看面向项目的 Agent 能力。"
        }
        MessageId::StartupHooksNoFindings => "内置只读检查未发现启动项。",
        MessageId::StartupHooksReadOnlyNote => "`cosh-shell` 只检查了轻量启动上下文。",
        MessageId::HelpTitle => "Slash 命令",
        MessageId::HelpFooter => "模式: {mode}. 策略: {strategy}.",
        MessageId::HelpGroupConfig => "配置",
        MessageId::HelpGroupModes => "模式",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "显示命令参考",
        MessageId::HelpSummaryConfig => "配置界面语言",
        MessageId::HelpSummaryModeApproval => "切换审批模式",
        MessageId::HelpSummaryModeAnalysis => "切换分析策略",
        MessageId::HelpSummaryAgent => "发起明确的 Agent 请求",
        MessageId::HelpSummaryExplain => "分析上一个失败命令",
        MessageId::HelpSummaryCancel => "取消正在运行的 Agent 工作",
        MessageId::HelpSummaryDetails => "查看审批或活动详情",
        MessageId::HelpSummaryAudit => "显示审计入口",
        MessageId::HelpSummaryHooks => "显示 Hook 状态",
        MessageId::HelpSummarySelect => "展示一条推荐",
        MessageId::HelpSummaryCopy => "复制一条推荐",
        MessageId::HelpSummaryDebug => "显示会话调试详情",
        MessageId::HelpSummaryClear => "清理本地 shell 状态",
        MessageId::HelpSummaryShell => "返回 shell 输入",
        MessageId::HelpSummarySkill => "显示 skill 路由提示",
        MessageId::HelpSummaryApprovalModeRemoved => "已移除的 approval-mode 别名",
        MessageId::SlashHintTitle => "Slash 命令提示",
        MessageId::SlashHintPrefix => "前缀: {prefix}",
        MessageId::SlashHintCurrentMode => "当前模式: {mode}",
        MessageId::SlashHintFooter => "输入完整命令并回车；/tmp/foo 这类路径仍进入 shell。",
        MessageId::SlashUnknownTitle => "Slash 命令",
        MessageId::SlashUnknownBody => "未知 slash 命令: {command}",
        MessageId::SlashUnknownSuggestionBody => "你是不是想用 {command}？",
        MessageId::SlashUnknownFooter => "使用 /help 查看可用命令。",
        MessageId::SlashInfoAuditTitle => "审计",
        MessageId::SlashInfoAuditApprovalsBody => "审批决策可通过 Details 操作查看。",
        MessageId::SlashInfoAuditActivityBody => "活动 output ref 可通过 Details 操作查看。",
        MessageId::SlashInfoAuditFooter => "审计视图是只读的；不会运行 shell 命令。",
        MessageId::SlashInfoConfigTitle => "配置",
        MessageId::SlashInfoConfigLanguageLine => "语言: {effective} 来源: {source}",
        MessageId::SlashInfoConfigLanguageEffectiveLine => {
            "语言: {effective} 生效，设置: {setting}，来源: {source}"
        }
        MessageId::SlashInfoConfigPathLine => "配置文件: {path}",
        MessageId::SlashInfoConfigDebugActivityLine => {
            "调试活动: {state} (ui.debug 或 COSH_SHELL_DEBUG=1)"
        }
        MessageId::SlashInfoConfigAnalysisStrategyLine => {
            "分析策略: /mode analysis smart|auto|manual"
        }
        MessageId::SlashInfoConfigRenderFallbackLine => {
            "渲染降级: 启动 cosh-shell 前设置 COSH_SHELL_RENDER=plain。"
        }
        MessageId::SlashInfoConfigFooter => {
            "使用 /config language [auto|en-US|zh-CN]。保存的语言会在下次启动时生效。"
        }
        MessageId::SlashInfoSkillTitle => "Skill",
        MessageId::SlashInfoSkillHookRoutingBody => "Hook 路由提示可将 Agent 分析导向某个 skill。",
        MessageId::SlashInfoSkillRegistryBody => "此 shell 会话未配置外部 skill registry。",
        MessageId::SlashInfoSkillFooter => "Skill hooks 仅提供建议，仍会经过治理。",
        MessageId::ConfigInvalidLanguageBody => "无效语言: {language}",
        MessageId::ConfigSupportedLanguagesFooter => "支持: auto, en-US, zh-CN。",
        MessageId::ConfigUnknownKeyBody => "未知配置项: {key}",
        MessageId::ConfigHomeMissingBody => "HOME 未设置，无法持久化配置。",
        MessageId::ConfigHomeMissingFooter => "设置 HOME 或手动编辑配置。",
        MessageId::ConfigUnchangedTitle => "配置未变更",
        MessageId::ConfigNoFileChangedBody => "未修改配置文件。",
        MessageId::ConfigSavedTitle => "配置已保存",
        MessageId::ConfigSavedValueLine => "已保存 ui.{setting} = \"{value}\"。",
        MessageId::ConfigCurrentSessionLanguageLine => "当前会话语言: {language}。",
        MessageId::ConfigSavedFooter => "保存的设置会在下次启动时生效。",
        MessageId::ConfigSaveFailedTitle => "配置保存失败",
        MessageId::ConfigSaveFailedBody => "配置保存失败: {error}",
        MessageId::ConfigSavePromptTitle => "保存配置？",
        MessageId::ConfigFileLine => "文件: {path}",
        MessageId::ConfigPendingChangeLine => "ui.{setting}: {before} -> {after}",
        MessageId::ConfigSaveButton => "保存",
        MessageId::ConfigCancelButton => "取消",
        MessageId::ConfigApplyKeysFooter => "按键: Left/Right 选择 | Enter 应用 | Esc 取消",
        MessageId::ConfigLanguageTitle => "语言",
        MessageId::ConfigLanguageAutoLine => "auto    跟随 LC_ALL/LC_MESSAGES/LANG",
        MessageId::ConfigLanguageEnLine => "en-US   英语",
        MessageId::ConfigLanguageZhLine => "zh-CN   简体中文",
        MessageId::ConfigLanguageKeysFooter => "按键: Left/Right 选择 | Enter 确认 | Esc 取消",
        MessageId::SlashHooksRegisteredTitle => "Hook 状态",
        MessageId::SlashHooksNoHooksBody => "未注册 Hook。",
        MessageId::SlashHooksStatusCountLine => {
            "已注册: {total}; 已启用: {enabled}; 已禁用: {disabled}。"
        }
        MessageId::SlashHooksStatusSourcesLine => {
            "来源: builtin={builtin}; user={user}; project={project}。"
        }
        MessageId::SlashHooksStatusProjectTrustLine => {
            "项目信任: trusted={trusted}; untrusted={untrusted}。"
        }
        MessageId::SlashHooksFooterCount => "已注册 {count} 个 Hook。",
        MessageId::SlashHooksFooterMutedTargets => {
            "已注册 {count} 个 Hook。已静音目标: {targets}。"
        }
        MessageId::SlashHooksTargetMutedTitle => "Hook 目标已静音",
        MessageId::SlashHooksTargetMutedBody => "本会话已静音 Hook 目标 '{target}'。",
        MessageId::SlashHooksTargetMutedFooter => "已静音 finding 仍会记录在 /hooks history。",
        MessageId::SlashHooksTargetUnmutedTitle => "Hook 目标已取消静音",
        MessageId::SlashHooksTargetUnmutedBody => "已取消静音 Hook 目标 '{target}'。",
        MessageId::SlashHooksTargetNotMutedBody => "Hook 目标 '{target}' 未处于静音状态。",
        MessageId::SlashHooksEnabledTitle => "Hook 已启用",
        MessageId::SlashHooksEnabledBody => "Hook '{id}' 已启用。",
        MessageId::SlashHooksDisabledTitle => "Hook 已禁用",
        MessageId::SlashHooksDisabledBody => "Hook '{id}' 已禁用。",
        MessageId::SlashHooksHistoryTitle => "Hook 历史",
        MessageId::SlashHooksHistoryEmptyBody => "本会话未记录 Hook finding。",
        MessageId::SlashHooksHistoryFooter => "最近 finding 只读；Analyze 仍需要用户确认。",
        MessageId::SlashHooksEventsTitle => "Hook 显示事件",
        MessageId::SlashHooksEventsEmptyBody => "本会话未记录 Hook 显示事件。",
        MessageId::SlashHooksEventsFooter => "事件仅属于当前会话，包含策略元数据，不包含命令输出。",
        MessageId::SlashHooksUsageTitle => "用法",
        MessageId::SlashHooksUsageListLine => "/hooks                - 显示 Hook 状态",
        MessageId::SlashHooksUsageHistoryLine => "/hooks history        - 显示最近 Hook finding",
        MessageId::SlashHooksUsageEventsLine => "/hooks events         - 显示最近 Hook 展示事件",
        MessageId::SlashHooksUsageAnalyzeLine => "/hooks analyze <id>   - 分析提示 finding",
        MessageId::SlashHooksUsageIgnoreLine => "/hooks ignore <id>    - 忽略提示 finding",
        MessageId::SlashHooksUsageDetailsLine => "/hooks details <id>   - 显示 Hook finding 详情",
        MessageId::SlashHooksUsageFeedbackLine => "/hooks feedback noisy|useful <id> - 记录反馈",
        MessageId::SlashHooksUsageClearFeedbackLine => "/hooks clear-feedback - 清除 Hook 反馈偏好",
        MessageId::SlashHooksUsageMuteLine => "/hooks mute <target>  - 静音 topic 或 Hook id",
        MessageId::SlashHooksUsageUnmuteLine => "/hooks unmute <target>- 取消静音 topic 或 Hook id",
        MessageId::SlashHooksUsageTrustProjectLine => "/hooks trust-project  - 信任本会话项目 Hook",
        MessageId::SlashHooksUsageUntrustProjectLine => {
            "/hooks untrust-project- 取消信任本会话项目 Hook"
        }
        MessageId::SlashHooksUsageClearProjectTrustLine => {
            "/hooks clear-project-trust - 清除项目 Hook 信任存储"
        }
        MessageId::SlashHooksUsageEnableLine => "/hooks enable <id>    - 启用 Hook",
        MessageId::SlashHooksUsageDisableLine => "/hooks disable <id>   - 禁用 Hook",
        MessageId::SlashHooksProjectTrustedTitle => "项目 Hook 已信任",
        MessageId::SlashHooksProjectUntrustedTitle => "项目 Hook 已取消信任",
        MessageId::SlashHooksProjectTrustNoHooksBody => "本会话未注册项目 Hook。",
        MessageId::SlashHooksProjectTrustedBody => "已将 {count} 个项目 Hook 标记为 trusted。",
        MessageId::SlashHooksProjectUntrustedBody => "已将 {count} 个项目 Hook 标记为 untrusted。",
        MessageId::SlashHooksProjectTrustNoChangeFooter => "信任状态未变更。",
        MessageId::SlashHooksProjectTrustPersistedFooter => "信任已持久化；已禁用 Hook 保持禁用。",
        MessageId::SlashHooksProjectTrustRemovedFooter => {
            "信任已从持久化存储移除；已禁用 Hook 保持禁用。"
        }
        MessageId::SlashHooksProjectTrustPersistenceFailedFooter => {
            "会话状态已变更，但持久化失败: {failures}"
        }
        MessageId::SlashHooksProjectTrustClearedTitle => "项目 Hook 信任已清除",
        MessageId::SlashHooksProjectTrustClearedBody => {
            "已将 {count} 个项目 Hook 标记为 untrusted。"
        }
        MessageId::SlashHooksProjectTrustClearedFooter => {
            "项目 Hook 信任存储已清除；当前会话项目 Hook 已取消信任。"
        }
        MessageId::SlashHooksProjectTrustClearFailedFooter => {
            "当前会话项目 Hook 已标记为 untrusted，但清除持久化信任存储失败: {error}"
        }
        MessageId::SlashHooksFeedbackUsageBody => "/hooks feedback noisy|useful <finding_id>",
        MessageId::SlashHooksFeedbackTitle => "Hook 反馈",
        MessageId::SlashHooksFeedbackFindingNotFoundBody => "本会话未找到 finding '{finding_id}'。",
        MessageId::SlashHooksFeedbackFindingNotFoundFooter => {
            "使用 /hooks history 复制最近的 finding id。"
        }
        MessageId::SlashHooksFeedbackRecordedTitle => "Hook 反馈已记录",
        MessageId::SlashHooksFeedbackRecordedBody => {
            "已为 finding '{finding_id}' 记录反馈 '{feedback}'。"
        }
        MessageId::SlashHooksFeedbackHookLine => "Hook: {hook_id}。",
        MessageId::SlashHooksFeedbackPolicyKeyLine => "策略 key: {key}。",
        MessageId::SlashHooksFeedbackPersistedFooter => "反馈已持久化，仅影响展示策略。",
        MessageId::SlashHooksFeedbackPersistenceFailedFooter => {
            "会话反馈已记录，但持久化失败: {error}"
        }
        MessageId::SlashHooksFeedbackClearedTitle => "Hook 反馈已清除",
        MessageId::SlashHooksFeedbackClearedBody => "已从本会话清除 {count} 条反馈偏好。",
        MessageId::SlashHooksFeedbackClearedFooter => "Hook 反馈偏好已清除。",
        MessageId::SlashHooksFeedbackClearFailedFooter => {
            "会话反馈已清除，但持久化存储清除失败: {error}"
        }
        MessageId::DebugSessionTitle => "会话调试",
        MessageId::DebugAdapterLine => "适配器: {value}",
        MessageId::DebugProviderCommittedSessionLine => "provider 已提交会话: {value}",
        MessageId::DebugActiveRunLine => "活跃运行: {value}",
        MessageId::DebugQueuedRunsLine => "排队运行: {value}",
        MessageId::DebugProviderPendingSessionLine => "provider 待提交会话: {value}",
        MessageId::DebugProviderInitializeSeenLine => "provider initialize 已收到: {value}",
        MessageId::DebugHostExecutedShellResultLine => "host-executed shell 结果: {value}",
        MessageId::DebugSelectedShellExecutionPathLine => "已选择 shell 执行路径: {value}",
        MessageId::DebugLatestRecoveryStatusLine => "最近恢复状态: {value}",
        MessageId::DebugLatestRecoveryReasonLine => "最近恢复原因: {value}",
        MessageId::DebugUnknownTargetBody => "未知 debug 目标: {target}",
        MessageId::DebugUnknownTargetFooter => "使用 /debug session。",
        MessageId::CommandRemovedTitle => "命令已移除",
        MessageId::ApprovalModeRemovedBody => "/approval-mode 不再支持。",
        MessageId::ApprovalModeRemovedFooter => "使用 /mode approval [recommend|auto|trust]。",
        MessageId::RemovedDecisionCommandBody => "{command} 不再作为输入命令支持。",
        MessageId::RemovedApprovalDecisionFooter => {
            "请使用审批卡片按钮；本次输入没有发送到 shell。"
        }
        MessageId::RemovedQuestionAnswerFooter => "请在问题卡片中回答；本次输入没有发送到 shell。",
        MessageId::ModeTitle => "模式",
        MessageId::ModesTitle => "模式",
        MessageId::ModeApprovalLine => "审批: {mode}",
        MessageId::ModeAnalysisLine => "分析: {mode}",
        MessageId::ModeSummaryFooter => {
            "使用 /mode approval [recommend|auto|trust] 或 /mode analysis [smart|auto|manual]。"
        }
        MessageId::ModeRemovedTitle => "模式命令已移除",
        MessageId::ModeRemovedBody => "/mode {mode} 不再支持。",
        MessageId::ModeRemovedFooter => "使用 /mode approval {mode}。",
        MessageId::ModeLanguageBody => "语言是持久化配置，不是运行时模式。",
        MessageId::ModeLanguageFooter => "使用 /config language [auto|en-US|zh-CN]。",
        MessageId::ModeUnknownBody => "未知模式: {mode}",
        MessageId::ModeUnknownFooter => {
            "使用 /mode approval recommend|auto|trust 或 /mode analysis smart|auto|manual。"
        }
        MessageId::ApprovalModeTitle => "审批模式",
        MessageId::ApprovalModeSetBody => "模式已设置为 {mode}。",
        MessageId::ApprovalModeUnknownBody => "未知审批模式: {mode}",
        MessageId::ApprovalModeUsageFooter => "使用 /mode approval recommend|auto|trust。",
        MessageId::ApprovalModeRecommendFooter => "Agent 只解释和建议；不会发出 tool call。",
        MessageId::ApprovalModeAutoFooter => "只读工具会自动批准；高风险请求仍需确认。",
        MessageId::ApprovalModeTrustFooter => {
            "所有工具会自动批准；审计记录仍通过 control protocol 保留。"
        }
        MessageId::ApprovalModeTrustConfirmationTitle => "需要确认 trust 模式",
        MessageId::ApprovalModeTrustConfirmationBody => {
            "trust 模式会在当前会话自动批准 provider tool 请求。"
        }
        MessageId::ApprovalModeTrustConfirmationCommandBody => {
            "运行 /mode approval trust confirm 显式启用。"
        }
        MessageId::ApprovalModeTrustConfirmationFooter => "确认前仍保持 recommend 或 auto 模式。",
        MessageId::ApprovalModeCardTitle => "用户模式",
        MessageId::ApprovalModeCardCurrentLine => "当前: {mode}",
        MessageId::ApprovalModeCardRecommendLine => "{marker}[ recommend ] 只解释和建议",
        MessageId::ApprovalModeCardAutoLine => {
            "{marker}[ auto      ] 只读自动批准；高风险请求仍需确认"
        }
        MessageId::ApprovalModeCardTrustLine => {
            "{marker}[ trust     ] 所有工具自动批准并保留审计记录"
        }
        MessageId::ApprovalModeCardFooter => "按键: Left/Right 选择 | Enter 应用 | Esc 取消",
        MessageId::ApprovalModeRemainsBody => "模式仍为 {mode}。",
        MessageId::ApprovalModeCancelBody => "模式未改变: {mode}。",
        MessageId::ApprovalModeCancelFooter => "没有执行 shell 命令。",
        MessageId::AnalysisModeTitle => "分析模式",
        MessageId::AnalysisModeCurrentBody => "当前: {mode}",
        MessageId::AnalysisModeSetBody => "模式已设置为 {mode}。",
        MessageId::AnalysisModeUnknownBody => "未知分析模式: {mode}",
        MessageId::AnalysisModeUsageFooter => "使用 /mode analysis smart|auto|manual。",
        MessageId::AnalysisModeSmartFooter => "命令失败时评估 hooks；展示发现供你复核。",
        MessageId::AnalysisModeAutoFooter => "命令失败时评估 hooks；自动触发 Agent 分析。",
        MessageId::AnalysisModeManualFooter => "已禁用 hooks 和自动分析；使用 slash 命令手动触发。",
        MessageId::AgentThinking => "正在思考...",
        MessageId::AgentThinkingElapsed => "正在思考... {elapsed}s · {detail}",
        MessageId::AgentRecoveryTitle => "Agent 恢复",
        MessageId::AgentRecoveryFreshTurnBody => "正在使用新的 provider 轮次恢复 shell evidence。",
        MessageId::AgentRecoveryContinuityBody => "Provider 会话连续性可能降低。",
        MessageId::AgentStatusTitle => "Agent",
        MessageId::AgentStillWorking => "仍在处理... {elapsed}s · {detail}",
        MessageId::AgentStatusFooter => "Ctrl+C 取消 · [Cancel]",
        MessageId::AgentStatusStarting => "正在启动",
        MessageId::AgentStatusWaitingBackend => "正在等待 Agent 后端",
        MessageId::AgentStatusStreaming => "正在流式输出",
        MessageId::AgentStatusReceivingResponse => "正在接收 Agent 响应",
        MessageId::AgentStatusSkill => "技能",
        MessageId::AgentStatusLoadingSkill => "正在加载技能 {skill}",
        MessageId::AgentStatusApproval => "审批",
        MessageId::AgentStatusWaitingApprovalTool => "正在等待审批: tool {tool}",
        MessageId::AgentStatusQuestion => "问题",
        MessageId::AgentStatusWaitingUserAnswer => "正在等待用户回答: {question}",
        MessageId::AgentStatusWaitingApprovalCommand => "正在等待审批: {command}",
        MessageId::AgentStatusTool => "tool",
        MessageId::AgentStatusCapturingToolOutput => "正在捕获 {tool_id} 的输出",
        MessageId::AgentStatusToolCompleted => "{tool_id} 已完成，状态 {status}",
        MessageId::AgentStatusCompleted => "已完成",
        MessageId::AgentStatusFailed => "已失败",
        MessageId::AgentStatusCancelled => "已取消",
        MessageId::AgentStatusRunningApprovedProviderTool => "正在运行已批准的 provider tool",
        MessageId::AgentStatusSkillFailed => "{skill} 失败: {error}",
        MessageId::AgentProviderTimeoutDroppedQueuedBody => {
            "provider 超时后已跳过 {dropped} 个排队请求"
        }
        MessageId::AgentCancellationRequestedTitle => "Agent 取消请求已发送",
        MessageId::AgentCancellationRequestedBody => "正在停止 active Agent 运行...",
        MessageId::AgentCancelledReasonLabel => "原因:",
        MessageId::AgentCancelledUserRequestedReason => "用户请求取消",
        MessageId::AgentResponseTitle => "Agent 回复",
        MessageId::AgentGovernanceTitle => "治理",
        MessageId::AgentGovernanceStatusLine => "状态: {phase}",
        MessageId::AgentGovernanceReasonLine => "原因: {reason}",
        MessageId::AgentGovernanceSummaryLine => "摘要: {summary}",
        MessageId::AgentGovernanceErrorLine => "错误: {error}",
        MessageId::AgentGovernanceSkillLoadingLine => "正在加载技能: {skill}",
        MessageId::AgentGovernanceSkillLoadedLine => "技能已加载: {skill}",
        MessageId::AgentGovernanceSkillFailedLine => "技能失败: {skill}",
        MessageId::AgentGovernanceToolOutputLine => "Tool 输出: {tool_id} {stream}",
        MessageId::AgentGovernanceToolCompletedLine => "Tool 已完成: {tool_id}",
        MessageId::AgentGovernanceApprovalRequiredLine => "需要审批: {subject}",
        MessageId::AgentGovernanceShellCommandSubject => "Shell 命令",
        MessageId::AgentGovernanceBashCommandSubject => "Bash 命令",
        MessageId::AgentGovernanceToolSubject => "{tool} tool",
        MessageId::AgentGovernanceBlockedUserApprovalLine => "已阻止: 需要用户审批",
        MessageId::AgentGovernanceQuestionLine => "问题: {question}",
        MessageId::AgentRecommendedCommandsLabel => "推荐命令:",
        MessageId::InterceptNoticeTitle => "AI 请求",
        MessageId::InterceptNoticeBody => "正在把输入交给 Agent: {input}",
        MessageId::InterceptNoticeFooter => "该输入已在进入 Bash 前被拦截。",
        MessageId::FailedCommandCardTitle => "命令失败",
        MessageId::FailedCommandCardBody => "`{command}` 退出码为 {exit_code}; id: {id}",
        MessageId::FailedCommandCardFooter => "[Analyze] [Dismiss] [Details]",
        MessageId::FailedAnalysisCancelledTitle => "Agent 已取消",
        MessageId::FailedAnalysisCancelledBody => "已取消 `{command}` 的待处理分析",
        MessageId::FailedAnalysisCancelNoActiveBody => "当前没有等待取消的 Agent 运行",
        MessageId::FailedAnalysisCancelledFooter => "Shell 保持可用。",
        MessageId::AnalysisSkippedTitle => "已跳过分析",
        MessageId::AnalysisSkippedBody => "已跳过 `{command}` 的重复失败分析",
        MessageId::AnalysisSkippedFooter => "这个命令连续失败次数过多，请稍后再试。",
        MessageId::HookAutoAnalyzedTitle => "Hook 自动分析",
        MessageId::HookAutoAnalyzedBody => "`{command}` 退出码为 {exit_code}",
        MessageId::HookAutoAnalyzedFooter => "Agent 分析正在启动。",
        MessageId::AgentQueuedTitle => "Agent 已排队",
        MessageId::AgentQueuedBodyCommand => "已捕获失败命令: {command}",
        MessageId::AgentQueuedBodyActive => "当前 Agent 运行仍在流式输出。",
        MessageId::AgentQueuedFooter => "当前 Agent 完成后会分析这次失败。",
        MessageId::HookFindingTitle => "Hook 发现",
        MessageId::HookFindingFooter => "使用 /hooks analyze|ignore|details {hint_id}。",
        MessageId::HookFindingMarkdownTitle => "命令 Hook 发现",
        MessageId::HookFindingMarkdownHookLine => "- Hook: `{hook_id}`.",
        MessageId::HookFindingMarkdownSeverityLine => "- 严重级别: `{severity}`.",
        MessageId::HookFindingMarkdownFindingLine => "- 发现: {finding}.",
        MessageId::HookFindingMarkdownOutputRefLine => "- 输出 ID: `{output_ref}`.",
        MessageId::HookFindingMarkdownSuggestionLine => "- 建议: {suggestion}.",
        MessageId::HookFindingMarkdownRelatedTitle => "- 相关发现:",
        MessageId::HookFindingMarkdownRelatedLine => "  - `{hook_id}` [{severity}]: {finding}",
        MessageId::HookFindingMarkdownAgentFollowUpLine => {
            "Agent 后续分析必须先使用 cosh-shell 的有界证据，再给出细节判断。"
        }
        MessageId::HookHintTitle => "Hook 提示",
        MessageId::HookHintNotFoundBody => "本会话没有找到 Hook 提示 '{hint_id}'。",
        MessageId::HookHintNotFoundFooter => "使用 /hooks history 复制最近的发现 id。",
        MessageId::HookHintNoFindingBody => "Hook 提示 '{hint_id}' 没有关联发现。",
        MessageId::HookHintBlockUnavailableBody => "命令块 '{block_id}' 已不可用。",
        MessageId::HookHintIgnoredTitle => "Hook 提示已忽略",
        MessageId::HookHintIgnoredBody => "本会话已忽略 Hook 提示 '{hint_id}'。",
        MessageId::HookHintIgnoredFooter => "后续匹配的发现会被策略降级。",
        MessageId::HookHintUsageTitle => "用法",
        MessageId::HookHintUsageBody => "/hooks analyze|ignore|details <hint_id>",
        MessageId::HookFindingDetailsTitle => "Hook 发现详情",
        MessageId::HookConsultationHookLabel => "Hook",
        MessageId::HookConsultationConfidenceReasonLine => "置信度: {confidence}; 原因: {reason}",
        MessageId::HookConsultationAnalyzeAction => "分析",
        MessageId::HookConsultationIgnoreAction => "忽略",
        MessageId::HookDetailsConfidenceLine => "置信度: {confidence}; 策略原因: {reason}",
        MessageId::HookDetailsUserInterestLine => "用户关注原因: {code}: {description}",
        MessageId::HookDetailsReasonLookupIntent => {
            "命令指向特定进程或搜索目标，因此该发现保持低打扰。"
        }
        MessageId::HookDetailsReasonPipelineIntent => {
            "命令管道可能已经转换输出，因此缺失或不确定的结构不会被视为高置信度。"
        }
        MessageId::HookDetailsReasonScriptIntent => {
            "脚本或批处理输出可能不代表用户当前关注点，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonWrapperLowConfidence => {
            "包装器、远程或容器上下文会让目标视图变得不明确，因此需要先验证。"
        }
        MessageId::HookDetailsReasonInteractiveIntent => {
            "交互式输出不是稳定的诊断快照，因此只显示采样提示。"
        }
        MessageId::HookDetailsReasonActiveRunDeferred => {
            "已有另一个 Agent 运行中，因此这个成功命令发现会等待并在显示前重新检查。"
        }
        MessageId::HookDetailsReasonUserContinuedInput => {
            "用户已经继续输入其他内容，因此这个成功命令发现不会打断。"
        }
        MessageId::HookDetailsReasonNonDiagnosticSuccessCommand => {
            "该命令不像明确的诊断快照，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonFeedbackNoisy => {
            "之前的用户反馈表明类似发现噪声较高，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonIgnoredSameFinding => "用户之前在本会话中忽略过匹配的发现。",
        MessageId::HookDetailsReasonSameCardAlreadyRendered => {
            "这个发现键已经展示过同等或更高严重级别的卡片。"
        }
        MessageId::HookDetailsReasonInterruptionBudget => {
            "最近类似卡片已经使用了本会话的打扰预算。"
        }
        MessageId::HookDetailsReasonLowConfidence => "证据不完整，需要先做只读验证再给出更强判断。",
        MessageId::HookDetailsReasonDiagnosticIntent => "明确的诊断命令且证据充分。",
        MessageId::HookDetailsReasonOtherIntent => "没有识别到明确的诊断意图。",
        MessageId::HookDetailsTopicLine => "主题: {topic}; 实体: {entity}",
        MessageId::HookDetailsSuppressionKeyLine => "抑制键: {key}",
        MessageId::HookDetailsOutputRefLine => "输出捕获: {ref}",
        MessageId::HookDetailsCreatedAtLine => "创建时间: {created_at}",
        MessageId::HookDetailsPromptHintLine => "提示词线索: {hint}",
        MessageId::HookDetailsRecommendedSkillLine => "推荐 skill: {skill}",
        MessageId::HookDetailsReadOnlyCliHintLine => "只读 CLI 提示: {hint}",
        MessageId::HookDetailsFooter => "分析仍需要确认。",
        MessageId::RuntimeDetailsUnavailableTitle => "详情不可用",
        MessageId::RuntimeDetailsUnavailableBody => {
            "{id} 不可用；请对审批或活动 id 使用 Details 操作"
        }
        MessageId::ActivityTitle => "活动",
        MessageId::ActivityDetailsTitle => "活动详情",
        MessageId::ActivityRunLabel => "运行",
        MessageId::ActivityDetailLabel => "详情",
        MessageId::ActivitySkillLabel => "技能",
        MessageId::ActivitySkillUpdatedStatus => "已更新",
        MessageId::ActivityToolLabel => "Tool",
        MessageId::ActivityToolOutputLabel => "Tool 输出",
        MessageId::ActivityShellLabel => "Shell",
        MessageId::ActivityStatusLoading => "加载中",
        MessageId::ActivityStatusLoaded => "已加载",
        MessageId::ActivityStatusFailed => "失败",
        MessageId::ActivityStatusCalled => "已调用",
        MessageId::ActivityStatusRequested => "请求审批",
        MessageId::ActivityStatusCaptured => "已捕获",
        MessageId::ActivityStatusCompleted => "已完成",
        MessageId::ActivityStatusError => "错误",
        MessageId::ActivityStatusInterrupted => "已中断",
        MessageId::ActivitySkillLoadingSummary => "{skill} 加载中",
        MessageId::ActivitySkillLoadedSummary => "{skill} 已加载",
        MessageId::ActivitySkillFailedSummary => "{skill} 失败",
        MessageId::ActivityToolCalledSummary => "{tool} 已调用；[Details] {id}",
        MessageId::ActivityToolRequestedSummary => "{tool} 请求审批；[Details] {id}",
        MessageId::ActivityToolOutputCapturedSummary => "{stream} 已捕获；[Details] {id}",
        MessageId::ActivityProviderNativeShellBypassSummary => {
            "{tool} 已由 provider 自动批准；[Details] {id}"
        }
        MessageId::ActivityToolNeedsForegroundShellSummary => {
            "可能需要前台 shell；[Send to shell] {handoff}；[Details] {id}"
        }
        MessageId::ActivityShellHandoffSentSummary => "{approval} 已发送到 shell",
        MessageId::MarkdownCodeLabel => "代码",
        MessageId::MarkdownCodeWithLanguageLabel => "代码: {language}",
        MessageId::MarkdownTableLabel => "表格",
        MessageId::RecommendationTitle => "推荐",
        MessageId::RecommendationEmptyBody => "没有命令推荐",
        MessageId::RecommendationFooter => "[Copy] [Insert] [Details] - 仅展示",
        MessageId::RecommendationNoSelectableTitle => "没有可选择的推荐",
        MessageId::RecommendationNoSelectableBody => "当前还没有可选择的推荐",
        MessageId::RecommendationUnavailableTitle => "推荐不可用",
        MessageId::RecommendationUnavailableBody => "推荐 {index} 不可用；请选择 1..{total}",
        MessageId::RecommendationSelectedTitle => "已选择推荐",
        MessageId::RecommendationSelectedBody => "已选择推荐 {index}",
        MessageId::RecommendationCopiedTitle => "复制推荐",
        MessageId::RecommendationCopiedBody => "复制推荐 {index}",
        MessageId::RecommendationInsertTitle => "插入推荐",
        MessageId::RecommendationInsertBody => "已准备推荐 {index}，等待手动输入",
        MessageId::RecommendationDetailsTitle => "推荐详情",
        MessageId::RecommendationDetailsBody => "推荐 {index} 的详情",
        MessageId::RecommendationDisplayOnlyBody => "仅展示：命令未执行；复制或重新输入后才会运行",
        MessageId::RecommendationCopyOnlyBody => "仅复制：命令只展示给你复制，没有执行。",
        MessageId::RecommendationInsertOnlyBody => {
            "Insert 只会成为待编辑输入；没有提交，也没有写入子 shell。"
        }
        MessageId::RecommendationDetailsOnlyBody => "仅查看详情：决定输入或复制前先检查命令。",
        MessageId::ToolOutputStdoutCapturedSummary => "stdout 已捕获；[Details] {id}",
        MessageId::ToolOutputStderrCapturedSummary => "stderr 已捕获；[Details] {id}",
        MessageId::ToolSummaryExit => "退出码 {exit}",
        MessageId::ToolSummaryBlocked => "tool 请求被 shell broker guard 阻止",
        MessageId::ToolSummaryTimedOut => "tool 请求超时",
        MessageId::ToolSummaryFailed => "tool 请求失败",
        MessageId::QuestionTitle => "Agent 问题",
        MessageId::QuestionAnswerLabel => "回答",
        MessageId::QuestionSelectOneLabel => "选择一项:",
        MessageId::QuestionSelectMultipleLabel => "选择一项或多项:",
        MessageId::QuestionOtherEmptyLabel => "其他...",
        MessageId::QuestionOtherValueLabel => "其他: {answer}",
        MessageId::QuestionKeysPrefix => "按键: ",
        MessageId::QuestionInstructionMoveTypeSend => "左/右移动 | 输入回答 | Enter 发送",
        MessageId::QuestionInstructionMoveToggleSend => "左/右移动 | Space 切换 | Enter 发送",
        MessageId::QuestionInstructionMoveSend => "左/右移动 | Enter 发送",
        MessageId::QuestionInstructionTypeSend => "输入回答 | Enter 发送",
        MessageId::QuestionInstructionNoAnswer => "没有可选择的回答。",
        MessageId::QuestionNoPendingTitle => "没有待回答问题",
        MessageId::QuestionNoPendingBody => "当前没有等待回答的 Agent 问题。",
        MessageId::ApprovalTitle => "审批",
        MessageId::ApprovalRequiredTitle => "需要审批",
        MessageId::ApprovalResolutionApprovedTitle => "已批准",
        MessageId::ApprovalResolutionAutoApprovedTitle => "已自动批准",
        MessageId::ApprovalResolutionTrustedTitle => "已信任",
        MessageId::ApprovalResolutionDeniedTitle => "已拒绝",
        MessageId::ApprovalResolutionCancelledTitle => "已取消",
        MessageId::ApprovalResolutionBlockedTitle => "已阻止",
        MessageId::ApprovalResolutionDeferredTitle => "已延后",
        MessageId::ApprovalActionAllowOnce => "允许一次",
        MessageId::ApprovalActionAlwaysTrust => "始终信任",
        MessageId::ApprovalActionDeny => "拒绝",
        MessageId::ApprovalActionDetails => "详情",
        MessageId::ApprovalToolInputLabel => "Tool 输入",
        MessageId::ApprovalCommandLabel => "命令",
        MessageId::ApprovalDetailsTitle => "审批详情",
        MessageId::ApprovalDetailsSourceLabel => "来源",
        MessageId::ApprovalDetailsRunLabel => "运行",
        MessageId::ApprovalDetailsExecutionLabel => "执行",
        MessageId::ApprovalDetailsCommandBlockLabel => "命令块",
        MessageId::ApprovalDetailsRedactionLabel => "脱敏",
        MessageId::ApprovalDetailsProviderRequestLabel => "Provider 请求",
        MessageId::ApprovalDetailsToolUseLabel => "Tool 使用",
        MessageId::ApprovalDetailsDefaultDenyLine => "默认: 拒绝",
        MessageId::ApprovalDetailsRequestLabel => "请求",
        MessageId::ApprovalDetailsInputLabel => "输入",
        MessageId::ApprovalDetailsBashCommandSubject => "Bash 命令",
        MessageId::ApprovalDetailsShellCommandSubject => "Shell 命令",
        MessageId::ApprovalDetailsToolSubject => "{tool} tool",
        MessageId::ApprovalDetailsPendingValue => "<待处理>",
        MessageId::ApprovalDetailsNoneValue => "<无>",
        MessageId::ApprovalDetailsNotApplicableValue => "<不适用>",
        MessageId::ApprovalJournalTitle => "审批记录",
        MessageId::ApprovalJournalDecisionCount => "{count} 条决策",
        MessageId::ApprovalJournalEmptyBody => "本 shell 会话还没有审批决策记录。",
        MessageId::ApprovalJournalActorLabel => "执行者",
        MessageId::ApprovalJournalPreviewHashLabel => "预览哈希",
        MessageId::ApprovalJournalSubjectLabel => "对象",
        MessageId::ApprovalJournalPreviewLabel => "预览",
        MessageId::ApprovalRiskSuffix => "风险 {risk}",
        MessageId::ApprovalQueueCompactLine => "队列: {position}/{total} 待处理",
        MessageId::ApprovalQueueFullLine => "队列: 第 {position}/{total} 个待处理",
        MessageId::ApprovalQueueNextSuffix => "；下一个 {next}",
        MessageId::ApprovalSubjectLabel => "对象: ",
        MessageId::ApprovalNextLabel => "下一个: ",
        MessageId::ApprovalKeysPrefix => "按键: ",
        MessageId::ApprovalKeysText => "左/右选择  Enter 确认  d 详情  Esc 取消",
        MessageId::ApprovalExecutableToolPolicy => "策略: 可执行 tool 请求必须先经过用户审批。",
        MessageId::ApprovalExecutableToolPolicyExtra => {
            "MVP 中只有已审批的只读 Bash/shell tool 请求可以运行。"
        }
        MessageId::ApprovalCommandDefaultPolicy => "默认: 拒绝。批准的命令仍会由只读 broker 复查。",
        MessageId::ApprovalRunShellCommandPrompt => "运行 shell 命令？",
        MessageId::ApprovalRunBashCommandPrompt => "运行 Bash 命令？",
        MessageId::ApprovalNotFoundTitle => "审批未找到",
        MessageId::ApprovalNotFoundBody => "{id} 不可用；审批卡片可能已经处理完成。",
        MessageId::ApprovalShellHandoffNotFoundTitle => "Shell handoff 未找到",
        MessageId::ApprovalShellHandoffNotFoundBody => {
            "{id} 不可用；请先对 provider tool failure 使用 Details 操作"
        }
        MessageId::ApprovalShellHandoffBlockedTitle => "Shell handoff 已阻止",
        MessageId::ApprovalShellHandoffBlockedFooter => "命令没有写入前台 shell。",
        MessageId::ApprovalShellHandoffValidationEmptyCommand => "Shell handoff 命令为空。",
        MessageId::ApprovalShellHandoffValidationMultilineCommand => {
            "Shell handoff 命令包含换行；尚未启用多行 handoff。"
        }
        MessageId::ApprovalShellHandoffValidationControlCharacter => {
            "Shell handoff 命令包含被阻止的控制字符。"
        }
        MessageId::ApprovalShellHandoffValidationEmptyPreview => "Shell handoff 预览为空。",
        MessageId::ApprovalShellHandoffValidationEmptyApprovalId => "Shell handoff 审批 id 为空。",
        MessageId::ApprovalShellHandoffValidationEmptyRunId => "Shell handoff run id 为空。",
        MessageId::ApprovalShellHandoffSendingTitle => "正在发送到 shell",
        MessageId::ApprovalShellHandoffSendingBody => "{id} 将在前台 shell 中运行。",
        MessageId::ApprovalShellHandoffTimeoutTitle => "Shell 恢复",
        MessageId::ApprovalShellHandoffTimeoutExceededBody => {
            "命令超过了配置的 shell handoff 超时时间（{seconds}s）。"
        }
        MessageId::ApprovalShellHandoffTimeoutInterruptBody => {
            "已向前台 PTY 发送中断；正在等待 shell evidence。"
        }
        MessageId::ApprovalReceiptKindToolRequest => "tool 请求",
        MessageId::ApprovalReceiptKindShellCommandRequest => "shell 命令请求",
        MessageId::ApprovalReceiptKindBashTool => "Bash tool",
        MessageId::ApprovalReceiptDecisionPending => "待处理",
        MessageId::ApprovalReceiptDecisionApproved => "已批准",
        MessageId::ApprovalReceiptDecisionSentToShell => "已发送到 shell",
        MessageId::ApprovalReceiptDecisionProviderNativeAllowed => "已允许 provider-native 执行",
        MessageId::ApprovalReceiptDecisionApprovedDisplayOnly => "已批准，仅展示",
        MessageId::ApprovalReceiptDecisionDenied => "已拒绝",
        MessageId::ApprovalReceiptDecisionCancelled => "用户已取消",
        MessageId::ApprovalReceiptDecisionBlocked => "已被 cosh-shell 阻止",
        MessageId::ApprovalReceiptSubjectBashSentToShell => "Bash tool: 已发送到 shell",
        MessageId::ApprovalReceiptSubjectBashProviderNative => "Bash tool: provider-native 执行",
        MessageId::ApprovalReceiptBashSentToShellMessage => "Bash tool 已发送到 shell",
        MessageId::ApprovalReceiptProviderNativeAllowedMessage => {
            "已允许 provider-native shell tool 执行"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{I18n, MessageId};
    use crate::config::Language;

    #[test]
    fn all_messages_have_en_and_zh_values() {
        for id in MessageId::ALL {
            assert!(!I18n::new(Language::EnUs).t(*id).trim().is_empty());
            assert!(!I18n::new(Language::ZhCn).t(*id).trim().is_empty());
        }
    }

    #[test]
    fn format_replaces_known_args_and_keeps_missing_args() {
        let i18n = I18n::new(Language::EnUs);
        let text = i18n.format(
            MessageId::StartupAdapterLine,
            &[("adapter", "qwen"), ("shell", "bash")],
        );

        assert!(text.contains("qwen"));
        assert!(text.contains("bash"));
        assert!(text.contains("{mode}"));
    }

    #[test]
    fn zh_catalog_keeps_protocol_tokens_stable() {
        let i18n = I18n::new(Language::ZhCn);

        assert!(i18n
            .t(MessageId::ModeLanguageFooter)
            .contains("/config language"));
        assert!(i18n
            .t(MessageId::RecommendationFooter)
            .contains("[Copy] [Insert] [Details]"));
        assert!(i18n.t(MessageId::ApprovalToolInputLabel).contains("Tool"));
        assert!(i18n.t(MessageId::HelpSummaryConfig).contains("语言"));
        assert!(i18n
            .t(MessageId::AgentRecoveryFreshTurnBody)
            .contains("provider"));
        assert!(i18n
            .t(MessageId::AgentStatusWaitingApprovalTool)
            .contains("tool"));
        assert_eq!(
            i18n.t(MessageId::ApprovalResolutionAutoApprovedTitle),
            "已自动批准"
        );
    }
}
