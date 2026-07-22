macro_rules! help_core_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpTitle,
            HelpFooter,
            HelpGroupConfig,
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
