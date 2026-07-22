use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
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
        MessageId::ApprovalAssessmentSummaryLine => {
            "评估: 影响 {impact}；决策 {decision}；置信度 {confidence}"
        }
        MessageId::ApprovalAssessmentReasonLine => "原因: {reason}",
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
        MessageId::ApprovalHookHeading => "Hook 审查",
        _ => return None,
    })
}
