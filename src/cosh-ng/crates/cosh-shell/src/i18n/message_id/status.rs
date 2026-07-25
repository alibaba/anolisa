macro_rules! status_command_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            HelpSummaryStatus,
            HelpSummaryAbout,
            HelpSummaryStats,
            SlashStatusTitle,
            SlashStatusVersionLine,
            SlashStatusAdapterLine,
            SlashStatusShellLine,
            SlashStatusApprovalLine,
            SlashStatusAnalysisLine,
            SlashStatusLanguageLine,
            SlashStatusCwdLine,
            SlashStatusFooter,
            SlashStatsTitle,
            SlashStatsDurationLine,
            SlashStatsNoSessionBody,
            SlashStatsFooter,
            HelpGroupStatus,
        );
    };
}
