macro_rules! help_core_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpTitle,
            HelpFooter,
            HelpGroupConfig,
            HelpGroupHealth,
            HelpGroupModes,
            HelpGroupHooks,
            HelpSummaryHelp,
            HelpSummaryAuth,
            HelpSummaryConfig,
            HelpSummaryRecommendations,
            HelpSummaryModeApproval,
            HelpSummaryModeAnalysis,
            HelpSummaryAgent,
            HelpSummaryExplain,
            HelpSummaryCancel,
            HelpSummaryDetails,
            HelpSummaryAudit,
            HelpSummaryHooks,
            HelpSummaryHealth,
            HelpSummarySelect,
            HelpSummaryCopy,
            HelpSummaryDebug,
            HelpSummaryClear,
            HelpSummaryShell,
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
        );
    };
}

macro_rules! help_session_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpGroupSessions,
            HelpSummarySession,
        );
    };
}

macro_rules! help_registry_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpGroupRegistry,
            HelpSummaryExtensions,
            HelpSummarySkills,
            SlashExtensionsTitle,
            SlashSkillsTitle,
            SlashRegistryUnavailable,
            SlashHooksShellSection,
            SlashHooksAgentSection,
            SlashHooksAgentUnavailable,
            SlashExtensionsEmptyBody,
            SlashSkillsEmptyBody,
        );
    };
}

macro_rules! slash_parse_error_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            SlashInvalidArgumentsTitle,
            SlashQuotedArgumentsUnsupported,
        );
    };
}

macro_rules! status_query_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpGroupStatus,
            HelpSummaryStatus,
            HelpSummaryStats,
            SlashValueUnavailable,
            SlashValueNotStarted,
            SlashValueIdle,
            SlashValueActive,
            SlashStatusTitle,
            SlashStatusVersionLine,
            SlashStatusBackendLine,
            SlashStatusProviderLine,
            SlashStatusModelLine,
            SlashStatusSessionLine,
            SlashStatusOsLine,
            SlashStatusModesLine,
            SlashStatusProviderUnavailableLine,
            SlashStatusFooter,
            SlashStatsTitle,
            SlashStatsModelTitle,
            SlashStatsToolsTitle,
            SlashStatsModelLine,
            SlashStatsBackendLine,
            SlashStatsRunStateLine,
            SlashStatsToolTotalsLine,
            SlashStatsNoToolCalls,
            SlashStatsToolRow,
            SlashStatsTelemetryUnavailable,
            SlashStatsUsageLine,
            SlashStatsFooter,
        );
    };
}

// Trailing segment: MCP slash-command message IDs appended after all existing
// segments so every pre-existing MessageId discriminant stays stable, per the
// stable-runtime-api trailing-segment contract.
macro_rules! mcp_slash_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpSummaryMcp,
            SlashMcpTitle,
            SlashMcpEmptyBody,
        );
    };
}
