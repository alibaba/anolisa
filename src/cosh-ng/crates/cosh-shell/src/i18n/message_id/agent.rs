macro_rules! agent_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
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
            AgentStatusThinking,
            AgentStatusPreparingModelSession,
            AgentStatusStartingModelBackend,
            AgentStatusModelInitialized,
            AgentStatusModelStatus,
            AgentStatusAnalysisCompleted,
            AgentStatusAnalysisReturnedError,
            AgentStatusStreaming,
            AgentStatusReceivingResponse,
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
            FailedAnalysisCancelledTitle,
            FailedAnalysisCancelledBody,
            FailedAnalysisCancelNoActiveBody,
            FailedAnalysisCancelledFooter,
            AnalysisSkippedTitle,
            AnalysisSkippedBody,
            AnalysisSkippedFooter,
        );
    };
}

// Keep feature-specific additions in a trailing segment so existing public
// MessageId discriminants remain stable.
macro_rules! tool_argument_status_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            AgentStatusToolArguments,
            AgentStatusGeneratingToolArguments,
        );
    };
}

macro_rules! agent_queue_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            AgentQueuedTitle,
            AgentQueuedBodyCommand,
            AgentQueuedBodyActive,
            AgentQueuedFooter,
        );
    };
}

macro_rules! compaction_queue_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            AgentQueueFullTitle,
            AgentControlQueueFullBody,
        );
    };
}

macro_rules! question_hardening_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            AgentQuestionUnavailableTitle,
            AgentQuestionUnavailableBody,
            AgentAnswerDeliveryUnknownTitle,
            AgentAnswerDeliveryUnknownBody,
        );
    };
}
