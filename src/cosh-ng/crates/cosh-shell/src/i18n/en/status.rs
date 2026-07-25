use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::HelpGroupStatus => "Status",
        MessageId::HelpSummaryStatus => "show version and runtime info",
        MessageId::HelpSummaryAbout => "show version info (alias for /status)",
        MessageId::HelpSummaryStats => "show session usage statistics",
        MessageId::SlashStatusTitle => "Status",
        MessageId::SlashStatusVersionLine => "version: {version}",
        MessageId::SlashStatusAdapterLine => "backend: {adapter}",
        MessageId::SlashStatusShellLine => "shell: {shell}",
        MessageId::SlashStatusApprovalLine => "approval: {mode}",
        MessageId::SlashStatusAnalysisLine => "analysis: {mode}",
        MessageId::SlashStatusLanguageLine => "language: {language}",
        MessageId::SlashStatusCwdLine => "cwd: {cwd}",
        MessageId::SlashStatusFooter => "Use /about for the same view, /stats for session usage.",
        MessageId::SlashStatsTitle => "Session Stats",
        MessageId::SlashStatsDurationLine => "session duration: {duration}",
        MessageId::SlashStatsNoSessionBody => "No active session to report stats for.",
        MessageId::SlashStatsFooter => "Use /status for version and runtime info.",
        _ => return None,
    })
}
