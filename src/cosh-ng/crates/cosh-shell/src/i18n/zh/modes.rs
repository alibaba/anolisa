use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ApprovalModeRemovedBody => "/approval-mode 不再支持。",
        MessageId::ApprovalModeRemovedFooter => "使用 /mode approval [recommend|auto|trust]。",
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
        MessageId::AnalysisModeSmartFooter => {
            "命令失败或系统诊断输出有价值时评估；展示洞察供你复核。"
        }
        MessageId::AnalysisModeAutoFooter => {
            "仅对少量高置信故障自动触发 Agent 分析；其他情况仍先提示。"
        }
        MessageId::AnalysisModeManualFooter => {
            "已关闭被动建议和自动分析；使用 slash 命令手动触发。"
        }
        MessageId::AnalysisModeCardSmartLine => "{marker}[ smart  ] 建议模式（推荐）",
        MessageId::AnalysisModeCardAutoLine => {
            "{marker}[ auto   ] 自动分析（命令失败后可能自动启动 Agent）"
        }
        MessageId::AnalysisModeCardManualLine => "{marker}[ manual ] 关闭主动介入",
        MessageId::AnalysisModeCardFooter => {
            "按键: Left/Right 或 Tab/Shift-Tab 选择 | Enter 应用 | Esc 取消"
        }
        MessageId::AnalysisModeRemainsBody => "模式仍为 {mode}。",
        MessageId::AnalysisModeCancelBody => "模式未改变: {mode}。",
        MessageId::AnalysisModeCancelFooter => "没有执行 shell 命令。",
        _ => return None,
    })
}
