macro_rules! legacy_approval_mode_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            ApprovalModeRemovedBody,
            ApprovalModeRemovedFooter,
        );
    };
}

macro_rules! mode_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
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
            AnalysisModeCardSmartLine,
            AnalysisModeCardAutoLine,
            AnalysisModeCardManualLine,
            AnalysisModeCardFooter,
            AnalysisModeRemainsBody,
            AnalysisModeCancelBody,
            AnalysisModeCancelFooter,
        );
    };
}

// The #1961 plan-mode workflow segment is appended after every earlier
// segment so pre-existing discriminants never shift.
macro_rules! plan_mode_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpSummaryModePlan,
            HelpSummaryPlan,
            ModePlanLine,
            PlanModeTitle,
            PlanModeEnabledBody,
            PlanModeDisabledBody,
            PlanModeStatusOnBody,
            PlanModeStatusOffBody,
            PlanModeEnabledFooter,
            PlanModeDisabledFooter,
            PlanModeAlreadyOnBody,
            PlanModeAlreadyOffBody,
            PlanModeUnknownBody,
            PlanModeUsageFooter,
        );
    };
}
