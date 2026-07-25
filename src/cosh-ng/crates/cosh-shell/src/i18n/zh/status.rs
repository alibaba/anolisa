use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::HelpGroupStatus => "状态",
        MessageId::HelpSummaryStatus => "显示版本和运行时信息",
        MessageId::HelpSummaryAbout => "显示版本信息（/status 的别名）",
        MessageId::HelpSummaryStats => "显示会话用量统计",
        MessageId::SlashStatusTitle => "状态",
        MessageId::SlashStatusVersionLine => "版本: {version}",
        MessageId::SlashStatusAdapterLine => "后端: {adapter}",
        MessageId::SlashStatusShellLine => "Shell: {shell}",
        MessageId::SlashStatusApprovalLine => "审批: {mode}",
        MessageId::SlashStatusAnalysisLine => "分析: {mode}",
        MessageId::SlashStatusLanguageLine => "语言: {language}",
        MessageId::SlashStatusCwdLine => "cwd: {cwd}",
        MessageId::SlashStatusFooter => "使用 /about 查看相同信息，/stats 查看会话用量。",
        MessageId::SlashStatsTitle => "会话统计",
        MessageId::SlashStatsDurationLine => "会话时长: {duration}",
        MessageId::SlashStatsNoSessionBody => "当前没有活跃会话可以报告统计信息。",
        MessageId::SlashStatsFooter => "使用 /status 查看版本和运行时信息。",
        _ => return None,
    })
}
