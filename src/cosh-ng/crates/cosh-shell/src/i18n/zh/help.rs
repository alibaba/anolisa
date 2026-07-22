use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::HelpTitle => "Slash 命令",
        MessageId::HelpFooter => "模式: {mode}. 策略: {strategy}.",
        MessageId::HelpGroupConfig => "配置",
        MessageId::HelpGroupHealth => "健康",
        MessageId::HelpGroupModes => "模式",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "显示命令参考",
        MessageId::HelpSummaryAuth => "配置 AI 服务商凭证",
        MessageId::HelpSummaryConfig => "配置界面语言",
        MessageId::HelpSummaryRecommendations => {
            "管理个性化提示词推荐；分析会向服务商发送有界活动，本地 clear 不控制服务商侧保留"
        }
        MessageId::HelpSummaryModeApproval => "切换审批模式",
        MessageId::HelpSummaryModeAnalysis => "选择建议模式、自动分析或关闭主动介入",
        MessageId::HelpSummaryAgent => "发起明确的 Agent 请求",
        MessageId::HelpSummaryExplain => "分析上一个失败命令",
        MessageId::HelpSummaryCancel => "取消正在运行的 Agent 工作",
        MessageId::HelpSummaryDetails => "查看审批或活动详情",
        MessageId::HelpSummaryAudit => "显示审计入口",
        MessageId::HelpSummaryHooks => "显示 Hook 状态",
        MessageId::HelpSummaryHealth => "按需运行健康检查",
        MessageId::HelpSummarySelect => "展示一条推荐",
        MessageId::HelpSummaryCopy => "复制一条推荐",
        MessageId::HelpSummaryDebug => "显示会话调试详情",
        MessageId::HelpSummaryClear => "清理本地 shell 状态",
        MessageId::HelpSummaryShell => "返回 shell 输入",
        MessageId::HelpSummaryApprovalModeRemoved => "已移除的 approval-mode 别名",
        MessageId::SlashHintTitle => "Slash 命令提示",
        MessageId::SlashHintPrefix => "前缀: {prefix}",
        MessageId::SlashHintCurrentMode => "当前模式: {mode}",
        MessageId::SlashHintFooter => "输入完整命令并回车；/tmp/foo 这类路径仍进入 shell。",
        MessageId::SlashUnknownTitle => "Slash 命令",
        MessageId::SlashUnknownBody => "未知 slash 命令: {command}",
        MessageId::SlashUnknownSuggestionBody => "你是不是想用 {command}？",
        MessageId::SlashUnknownFooter => "使用 /help 查看可用命令。",
        MessageId::SlashQuotedArgumentsUnsupported => {
            "不支持带引号的参数。本例请改用 /mode approval trust confirm。"
        }
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
        MessageId::HelpGroupSessions => "会话",
        MessageId::HelpSummarySession => "查找、恢复和清理智能体会话",
        MessageId::HelpGroupRegistry => "Registry",
        MessageId::HelpSummaryExtensions => "列出/管理 cosh-core 扩展",
        MessageId::HelpSummarySkills => "列出/查看 cosh-core 技能",
        MessageId::SlashExtensionsTitle => "扩展",
        MessageId::SlashSkillsTitle => "技能",
        MessageId::SlashRegistryUnavailable => "此功能需要 cosh-core 后端支持。",
        MessageId::SlashHooksShellSection => "Shell Hooks",
        MessageId::SlashHooksAgentSection => "Agent Hooks",
        MessageId::SlashHooksAgentUnavailable => "(cosh-core 后端不可用)",
        MessageId::SlashExtensionsEmptyBody => "未安装扩展。",
        MessageId::SlashSkillsEmptyBody => "未发现技能。",
        _ => return None,
    })
}
