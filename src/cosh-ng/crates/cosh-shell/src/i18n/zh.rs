use super::MessageId;

pub(super) fn message(id: MessageId) -> &'static str {
    match id {
        MessageId::StartupTitle => "cosh-shell",
        MessageId::StartupAdapterLine => {
            "后端: {adapter} · Shell: {shell} · 审批: {approval} · 分析: {analysis}"
        }
        MessageId::StartupCwdLine => "cwd: {cwd}",
        MessageId::StartupCommandsLine => "/help · /mode · /hooks",
        MessageId::StartupHooksNoneSummary => "启动 hooks: 未配置。",
        MessageId::StartupHooksCompletedSummary => "启动 hooks: 内置只读检查已完成。",
        MessageId::StartupHooksFindingsHeading => "启动检查结果",
        MessageId::StartupHooksRustProjectFinding => {
            "检测到 `Cargo.toml` Rust 项目；`/skill` 可查看面向项目的 Agent 能力。"
        }
        MessageId::StartupHooksNoFindings => "内置只读检查未发现启动项。",
        MessageId::StartupHooksReadOnlyNote => "`cosh-shell` 只检查了轻量启动上下文。",
        MessageId::StartupSwitchHint => {
            "\u{1f4a1} 运行 \"cosh-switch\" 可在 cosh-ng 与 copilot-shell 之间切换"
        }
        MessageId::HelpTitle => "Slash 命令",
        MessageId::HelpFooter => "模式: {mode}. 策略: {strategy}.",
        MessageId::HelpGroupConfig => "配置",
        MessageId::HelpGroupModes => "模式",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "显示命令参考",
        MessageId::HelpSummaryAuth => "配置 AI 服务商凭证",
        MessageId::HelpSummaryConfig => "配置界面语言",
        MessageId::HelpSummaryModeApproval => "切换审批模式",
        MessageId::HelpSummaryModeAnalysis => "选择建议模式、自动分析或关闭主动介入",
        MessageId::HelpSummaryAgent => "发起明确的 Agent 请求",
        MessageId::HelpSummaryExplain => "分析上一个失败命令",
        MessageId::HelpSummaryCancel => "取消正在运行的 Agent 工作",
        MessageId::HelpSummaryDetails => "查看审批或活动详情",
        MessageId::HelpSummaryAudit => "显示审计入口",
        MessageId::HelpSummaryHooks => "显示 Hook 状态",
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
        MessageId::ConfigInvalidLanguageBody => "无效语言: {language}",
        MessageId::ConfigSupportedLanguagesFooter => "支持: auto, en-US, zh-CN。",
        MessageId::ConfigUnknownKeyBody => "未知配置项: {key}",
        MessageId::ConfigHomeMissingBody => "HOME 未设置，无法持久化配置。",
        MessageId::ConfigHomeMissingFooter => "设置 HOME 或手动编辑配置。",
        MessageId::ConfigUnchangedTitle => "配置未变更",
        MessageId::ConfigNoFileChangedBody => "未修改配置文件。",
        MessageId::ConfigSavedTitle => "配置已保存",
        MessageId::ConfigSavedValueLine => "已保存 ui.{setting} = \"{value}\"。",
        MessageId::ConfigCurrentSessionLanguageLine => "当前会话语言: {language}。",
        MessageId::ConfigSavedFooter => "保存的设置会在下次启动时生效。",
        MessageId::ConfigSaveFailedTitle => "配置保存失败",
        MessageId::ConfigSaveFailedBody => "配置保存失败: {error}",
        MessageId::ConfigSavePromptTitle => "保存配置？",
        MessageId::ConfigFileLine => "文件: {path}",
        MessageId::ConfigPendingChangeLine => "ui.{setting}: {before} -> {after}",
        MessageId::ConfigSaveButton => "保存",
        MessageId::ConfigCancelButton => "取消",
        MessageId::ConfigApplyKeysFooter => "按键: Left/Right 选择 | Enter 应用 | Esc 取消",
        MessageId::ConfigLanguageTitle => "语言",
        MessageId::ConfigLanguageAutoLine => "auto    跟随 LC_ALL/LC_MESSAGES/LANG",
        MessageId::ConfigLanguageEnLine => "en-US   英语",
        MessageId::ConfigLanguageZhLine => "zh-CN   简体中文",
        MessageId::ConfigLanguageKeysFooter => "按键: Left/Right 选择 | Enter 确认 | Esc 取消",
        MessageId::SlashHooksRegisteredTitle => "Hook 状态",
        MessageId::SlashHooksNoHooksBody => "未注册 Hook。",
        MessageId::SlashHooksStatusCountLine => {
            "已注册: {total}; 已启用: {enabled}; 已禁用: {disabled}。"
        }
        MessageId::SlashHooksStatusSourcesLine => {
            "来源: builtin={builtin}; user={user}; project={project}。"
        }
        MessageId::SlashHooksStatusProjectTrustLine => {
            "项目信任: trusted={trusted}; untrusted={untrusted}。"
        }
        MessageId::SlashHooksFooterCount => "已注册 {count} 个 Hook。",
        MessageId::SlashHooksFooterMutedTargets => {
            "已注册 {count} 个 Hook。已静音目标: {targets}。"
        }
        MessageId::SlashHooksTargetMutedTitle => "Hook 目标已静音",
        MessageId::SlashHooksTargetMutedBody => "本会话已静音 Hook 目标 '{target}'。",
        MessageId::SlashHooksTargetMutedFooter => "已静音 finding 仍会记录在 /hooks history。",
        MessageId::SlashHooksTargetUnmutedTitle => "Hook 目标已取消静音",
        MessageId::SlashHooksTargetUnmutedBody => "已取消静音 Hook 目标 '{target}'。",
        MessageId::SlashHooksTargetNotMutedBody => "Hook 目标 '{target}' 未处于静音状态。",
        MessageId::SlashHooksEnabledTitle => "Hook 已启用",
        MessageId::SlashHooksEnabledBody => "Hook '{id}' 已启用。",
        MessageId::SlashHooksDisabledTitle => "Hook 已禁用",
        MessageId::SlashHooksDisabledBody => "Hook '{id}' 已禁用。",
        MessageId::SlashHooksHistoryTitle => "Hook 历史",
        MessageId::SlashHooksHistoryEmptyBody => "本会话未记录 Hook finding。",
        MessageId::SlashHooksHistoryFooter => "最近 finding 只读；Analyze 仍需要用户确认。",
        MessageId::SlashHooksEventsTitle => "Hook 显示事件",
        MessageId::SlashHooksEventsEmptyBody => "本会话未记录 Hook 显示事件。",
        MessageId::SlashHooksEventsFooter => "事件仅属于当前会话，包含策略元数据，不包含命令输出。",
        MessageId::SlashHooksUsageTitle => "用法",
        MessageId::SlashHooksUsageListLine => "/hooks                - 显示 Hook 状态",
        MessageId::SlashHooksUsageHistoryLine => "/hooks history        - 显示最近 Hook finding",
        MessageId::SlashHooksUsageEventsLine => "/hooks events         - 显示最近 Hook 展示事件",
        MessageId::SlashHooksUsageAnalyzeLine => "/hooks analyze <id>   - 分析提示 finding",
        MessageId::SlashHooksUsageIgnoreLine => "/hooks ignore <id>    - 忽略提示 finding",
        MessageId::SlashHooksUsageDetailsLine => "/hooks details <id>   - 显示 Hook finding 详情",
        MessageId::SlashHooksUsageFeedbackLine => "/hooks feedback noisy|useful <id> - 记录反馈",
        MessageId::SlashHooksUsageClearFeedbackLine => "/hooks clear-feedback - 清除 Hook 反馈偏好",
        MessageId::SlashHooksUsageMuteLine => "/hooks mute <target>  - 静音 topic 或 Hook id",
        MessageId::SlashHooksUsageUnmuteLine => "/hooks unmute <target>- 取消静音 topic 或 Hook id",
        MessageId::SlashHooksUsageTrustProjectLine => "/hooks trust-project  - 信任本会话项目 Hook",
        MessageId::SlashHooksUsageUntrustProjectLine => {
            "/hooks untrust-project- 取消信任本会话项目 Hook"
        }
        MessageId::SlashHooksUsageClearProjectTrustLine => {
            "/hooks clear-project-trust - 清除项目 Hook 信任存储"
        }
        MessageId::SlashHooksUsageEnableLine => "/hooks enable <id>    - 启用 Hook",
        MessageId::SlashHooksUsageDisableLine => "/hooks disable <id>   - 禁用 Hook",
        MessageId::SlashHooksProjectTrustedTitle => "项目 Hook 已信任",
        MessageId::SlashHooksProjectUntrustedTitle => "项目 Hook 已取消信任",
        MessageId::SlashHooksProjectTrustNoHooksBody => "本会话未注册项目 Hook。",
        MessageId::SlashHooksProjectTrustedBody => "已将 {count} 个项目 Hook 标记为 trusted。",
        MessageId::SlashHooksProjectUntrustedBody => "已将 {count} 个项目 Hook 标记为 untrusted。",
        MessageId::SlashHooksProjectTrustNoChangeFooter => "信任状态未变更。",
        MessageId::SlashHooksProjectTrustPersistedFooter => "信任已持久化；已禁用 Hook 保持禁用。",
        MessageId::SlashHooksProjectTrustRemovedFooter => {
            "信任已从持久化存储移除；已禁用 Hook 保持禁用。"
        }
        MessageId::SlashHooksProjectTrustPersistenceFailedFooter => {
            "会话状态已变更，但持久化失败: {failures}"
        }
        MessageId::SlashHooksProjectTrustClearedTitle => "项目 Hook 信任已清除",
        MessageId::SlashHooksProjectTrustClearedBody => {
            "已将 {count} 个项目 Hook 标记为 untrusted。"
        }
        MessageId::SlashHooksProjectTrustClearedFooter => {
            "项目 Hook 信任存储已清除；当前会话项目 Hook 已取消信任。"
        }
        MessageId::SlashHooksProjectTrustClearFailedFooter => {
            "当前会话项目 Hook 已标记为 untrusted，但清除持久化信任存储失败: {error}"
        }
        MessageId::SlashHooksFeedbackUsageBody => "/hooks feedback noisy|useful <finding_id>",
        MessageId::SlashHooksFeedbackTitle => "Hook 反馈",
        MessageId::SlashHooksFeedbackFindingNotFoundBody => "本会话未找到 finding '{finding_id}'。",
        MessageId::SlashHooksFeedbackFindingNotFoundFooter => {
            "使用 /hooks history 复制最近的 finding id。"
        }
        MessageId::SlashHooksFeedbackRecordedTitle => "Hook 反馈已记录",
        MessageId::SlashHooksFeedbackRecordedBody => {
            "已为 finding '{finding_id}' 记录反馈 '{feedback}'。"
        }
        MessageId::SlashHooksFeedbackHookLine => "Hook: {hook_id}。",
        MessageId::SlashHooksFeedbackPolicyKeyLine => "策略 key: {key}。",
        MessageId::SlashHooksFeedbackPersistedFooter => "反馈已持久化，仅影响展示策略。",
        MessageId::SlashHooksFeedbackPersistenceFailedFooter => {
            "会话反馈已记录，但持久化失败: {error}"
        }
        MessageId::SlashHooksFeedbackClearedTitle => "Hook 反馈已清除",
        MessageId::SlashHooksFeedbackClearedBody => "已从本会话清除 {count} 条反馈偏好。",
        MessageId::SlashHooksFeedbackClearedFooter => "Hook 反馈偏好已清除。",
        MessageId::SlashHooksFeedbackClearFailedFooter => {
            "会话反馈已清除，但持久化存储清除失败: {error}"
        }
        MessageId::DebugSessionTitle => "会话调试",
        MessageId::DebugAdapterLine => "适配器: {value}",
        MessageId::DebugProviderInvocationLine => "provider 调用路径: {value}",
        MessageId::DebugProviderCommittedSessionLine => "provider 已提交会话: {value}",
        MessageId::DebugActiveRunLine => "活跃运行: {value}",
        MessageId::DebugQueuedRunsLine => "排队运行: {value}",
        MessageId::DebugProviderPendingSessionLine => "provider 待提交会话: {value}",
        MessageId::DebugProviderInitializeSeenLine => "provider initialize 已收到: {value}",
        MessageId::DebugHostExecutedShellResultLine => "host-executed shell 结果: {value}",
        MessageId::DebugSelectedShellExecutionPathLine => "已选择 shell 执行路径: {value}",
        MessageId::DebugLatestProviderRequestLine => "最近 provider request: {value}",
        MessageId::DebugLatestToolUseLine => "最近 tool use id: {value}",
        MessageId::DebugLatestRecoveryStatusLine => "最近恢复状态: {value}",
        MessageId::DebugLatestRecoveryReasonLine => "最近恢复原因: {value}",
        MessageId::DebugEvidenceAccessLine => "evidence 访问方式: {value}",
        MessageId::DebugEvidenceToolRegisteredLine => "evidence tool 已注册: {value}",
        MessageId::DebugEvidenceNamespaceLine => "当前 evidence namespace: {value}",
        MessageId::DebugEvidenceLedgerCountLine => "evidence ledger 命令数: {value}",
        MessageId::DebugLatestShellOutputReadLine => "最近 shell evidence action: {value}",
        MessageId::DebugUnknownTargetBody => "未知 debug 目标: {target}",
        MessageId::DebugUnknownTargetFooter => "使用 /debug session。",
        MessageId::CommandRemovedTitle => "命令已移除",
        MessageId::ApprovalModeRemovedBody => "/approval-mode 不再支持。",
        MessageId::ApprovalModeRemovedFooter => "使用 /mode approval [recommend|auto|trust]。",
        MessageId::RemovedDecisionCommandBody => "{command} 不再作为输入命令支持。",
        MessageId::RemovedApprovalDecisionFooter => {
            "请使用审批卡片按钮；本次输入没有发送到 shell。"
        }
        MessageId::RemovedQuestionAnswerFooter => "请在问题卡片中回答；本次输入没有发送到 shell。",
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
        MessageId::AgentThinking => "正在思考...",
        MessageId::AgentThinkingElapsed => "正在思考... {elapsed}s · {detail}",
        MessageId::AgentRecoveryTitle => "Agent 恢复",
        MessageId::AgentRecoveryFreshTurnBody => "正在使用新的 provider 轮次恢复 shell evidence。",
        MessageId::AgentRecoveryContinuityBody => "Provider 会话连续性可能降低。",
        MessageId::AgentStatusTitle => "Agent",
        MessageId::AgentStillWorking => "仍在处理... {elapsed}s · {detail}",
        MessageId::AgentStatusFooter => "Ctrl+C 取消 · [Cancel]",
        MessageId::AgentStatusStarting => "正在启动",
        MessageId::AgentStatusWaitingBackend => "正在等待 Agent 后端",
        MessageId::AgentStatusThinking => "正在思考",
        MessageId::AgentStatusPreparingModelSession => "正在准备模型会话",
        MessageId::AgentStatusStartingModelBackend => "正在启动模型后端",
        MessageId::AgentStatusModelInitialized => "模型已初始化 {model}",
        MessageId::AgentStatusModelStatus => "模型状态: {status}",
        MessageId::AgentStatusAnalysisCompleted => "分析完成",
        MessageId::AgentStatusAnalysisReturnedError => "分析返回错误",
        MessageId::AgentStatusStreaming => "正在流式输出",
        MessageId::AgentStatusReceivingResponse => "正在接收 Agent 响应",
        MessageId::AgentStatusApproval => "审批",
        MessageId::AgentStatusWaitingApprovalTool => "正在等待审批: tool {tool}",
        MessageId::AgentStatusQuestion => "问题",
        MessageId::AgentStatusWaitingUserAnswer => "正在等待用户回答: {question}",
        MessageId::AgentStatusWaitingApprovalCommand => "正在等待审批: {command}",
        MessageId::AgentStatusTool => "tool",
        MessageId::AgentStatusCapturingToolOutput => "正在捕获 {tool_id} 的输出",
        MessageId::AgentStatusToolCompleted => "{tool_id} 已完成，状态 {status}",
        MessageId::AgentStatusCompleted => "已完成",
        MessageId::AgentStatusFailed => "已失败",
        MessageId::AgentStatusCancelled => "已取消",
        MessageId::AgentStatusRunningApprovedProviderTool => "正在运行已批准的 provider tool",
        MessageId::AgentProviderTimeoutDroppedQueuedBody => {
            "provider 超时后已跳过 {dropped} 个排队请求"
        }
        MessageId::AgentCancellationRequestedTitle => "Agent 取消请求已发送",
        MessageId::AgentCancellationRequestedBody => "正在停止 active Agent 运行...",
        MessageId::AgentCancelledReasonLabel => "原因:",
        MessageId::AgentCancelledUserRequestedReason => "用户请求取消",
        MessageId::AgentResponseTitle => "Agent 回复",
        MessageId::AgentGovernanceTitle => "治理",
        MessageId::AgentGovernanceStatusLine => "状态: {phase}",
        MessageId::AgentGovernanceReasonLine => "原因: {reason}",
        MessageId::AgentGovernanceSummaryLine => "摘要: {summary}",
        MessageId::AgentGovernanceErrorLine => "错误: {error}",
        MessageId::AgentGovernanceToolOutputLine => "Tool 输出: {tool_id} {stream}",
        MessageId::AgentGovernanceToolCompletedLine => "Tool 已完成: {tool_id}",
        MessageId::AgentGovernanceApprovalRequiredLine => "需要审批: {subject}",
        MessageId::AgentGovernanceShellCommandSubject => "Shell 命令",
        MessageId::AgentGovernanceBashCommandSubject => "Bash 命令",
        MessageId::AgentGovernanceToolSubject => "{tool} tool",
        MessageId::AgentGovernanceBlockedUserApprovalLine => "已阻止: 需要用户审批",
        MessageId::AgentGovernanceQuestionLine => "问题: {question}",
        MessageId::AgentRecommendedCommandsLabel => "推荐命令:",
        MessageId::InterceptNoticeTitle => "AI 请求",
        MessageId::InterceptNoticeBody => "正在把输入交给 Agent: {input}",
        MessageId::InterceptNoticeFooter => "该输入已在进入 Bash 前被拦截。",
        MessageId::FailedAnalysisCancelledTitle => "Agent 已取消",
        MessageId::FailedAnalysisCancelledBody => "已取消 `{command}` 的待处理分析",
        MessageId::FailedAnalysisCancelNoActiveBody => "当前没有等待取消的 Agent 运行",
        MessageId::FailedAnalysisCancelledFooter => "Shell 保持可用。",
        MessageId::AnalysisSkippedTitle => "已跳过分析",
        MessageId::AnalysisSkippedBody => "已跳过 `{command}` 的重复失败分析",
        MessageId::AnalysisSkippedFooter => "这个命令连续失败次数过多，请稍后再试。",
        MessageId::HookAutoAnalyzedTitle => "Hook 自动分析",
        MessageId::HookAutoAnalyzedBody => "`{command}` 退出码为 {exit_code}",
        MessageId::HookAutoAnalyzedFooter => "Agent 分析正在启动。",
        MessageId::InsightLabel => "洞察：",
        MessageId::InsightCommandTypoSummary => "发现可能的命令拼写错误",
        MessageId::InsightPermissionDeniedSummary => "命令因权限不足被拒绝",
        MessageId::InsightBuildOrTestFailureSummary => "构建或测试失败",
        MessageId::InsightRuntimeExceptionSummary => "程序发生未捕获异常",
        MessageId::InsightAbnormalSignalSummary => "命令因异常信号而终止",
        MessageId::InsightMemoryPressureSummary => "当前内存压力需要关注",
        MessageId::InsightHighMemoryProcessSummary => "{process} 的内存占用异常偏高",
        MessageId::InsightHighMemoryProcessGenericSummary => "有进程的内存占用异常偏高",
        MessageId::InsightMemoryRootCauseSummary => "内存压力可能与 {process} 有关",
        MessageId::InsightMemoryRootCauseGenericSummary => "内存压力可能与高内存进程有关",
        MessageId::InsightPermissionDeniedPrompt => {
            "分析这次权限失败，先判断限制来源并给出最小权限建议"
        }
        MessageId::InsightBuildOrTestFailurePrompt => "分析这次构建或测试失败，定位首个可行动错误",
        MessageId::InsightRuntimeExceptionPrompt => {
            "分析这次未捕获异常，确认直接原因及是否需要修复"
        }
        MessageId::InsightAbnormalSignalPrompt => {
            "分析命令异常终止，确认信号事实并给出一个安全检查"
        }
        MessageId::InsightMemoryPressurePrompt => "基于当前输出分析内存压力，必要时定位主要进程",
        MessageId::InsightHighMemoryProcessPrompt => {
            "基于当前输出判断 {process} 是否为主要内存来源"
        }
        MessageId::InsightHighMemoryProcessGenericPrompt => "基于当前输出判断主要内存进程",
        MessageId::InsightMemoryRootCausePrompt => {
            "基于当前输出确认 {process} 是否为内存压力主要来源"
        }
        MessageId::InsightMemoryRootCauseGenericPrompt => "基于当前输出确认内存压力的主要进程来源",
        MessageId::InsightShellRewriteFirstUseHint => "Tab 填入后按 Enter 执行；继续输入可忽略",
        MessageId::InsightAgentPromptFirstUseHint => "Tab 填入后按 Enter 提交；继续输入可忽略",
        MessageId::AgentQueuedTitle => "Agent 已排队",
        MessageId::AgentQueuedBodyCommand => "已捕获失败命令: {command}",
        MessageId::AgentQueuedBodyActive => "当前 Agent 运行仍在流式输出。",
        MessageId::AgentQueuedFooter => "当前 Agent 完成后会分析这次失败。",
        MessageId::HookFindingTitle => "Hook 发现",
        MessageId::HookFindingFooter => "使用 /hooks 查看 Hook 发现。",
        MessageId::HookFindingMarkdownTitle => "命令 Hook 发现",
        MessageId::HookFindingMarkdownHookLine => "- Hook: `{hook_id}`.",
        MessageId::HookFindingMarkdownSeverityLine => "- 严重级别: `{severity}`.",
        MessageId::HookFindingMarkdownFindingLine => "- 发现: {finding}.",
        MessageId::HookFindingMarkdownSuggestionLine => "- 建议: {suggestion}.",
        MessageId::HookFindingMarkdownRelatedTitle => "- 相关发现:",
        MessageId::HookFindingMarkdownRelatedLine => "  - `{hook_id}` [{severity}]: {finding}",
        MessageId::HookFindingMarkdownAgentFollowUpLine => {
            "Agent 后续分析必须先使用 cosh-shell 的有界证据，再给出细节判断。"
        }
        MessageId::HookHintTitle => "Hook 提示",
        MessageId::HookHintNotFoundBody => "本会话没有找到 Hook 提示 '{hint_id}'。",
        MessageId::HookHintNotFoundFooter => "使用 /hooks history 复制最近的发现 id。",
        MessageId::HookHintNoFindingBody => "Hook 提示 '{hint_id}' 没有关联发现。",
        MessageId::HookHintBlockUnavailableBody => "命令块 '{block_id}' 已不可用。",
        MessageId::HookHintIgnoredTitle => "Hook 提示已忽略",
        MessageId::HookHintIgnoredBody => "本会话已忽略 Hook 提示 '{hint_id}'。",
        MessageId::HookHintIgnoredFooter => "后续匹配的发现会被策略降级。",
        MessageId::HookHintUsageTitle => "用法",
        MessageId::HookHintUsageBody => "/hooks analyze|ignore|details <hint_id>",
        MessageId::HookFindingDetailsTitle => "Hook 发现详情",
        MessageId::HookConsultationHookLabel => "Hook",
        MessageId::HookConsultationConfidenceReasonLine => "置信度: {confidence}; 原因: {reason}",
        MessageId::HookConsultationFindingLine => "发现: {finding}",
        MessageId::HookConsultationSuggestionLine => "建议动作: {suggestion}",
        MessageId::HookConsultationAnalyzeAction => "分析",
        MessageId::HookConsultationIgnoreAction => "忽略",
        MessageId::HookDetailsConfidenceLine => "置信度: {confidence}; 策略原因: {reason}",
        MessageId::HookDetailsUserInterestLine => "用户关注原因: {code}: {description}",
        MessageId::HookDetailsReasonLookupIntent => {
            "命令指向特定进程或搜索目标，因此该发现保持低打扰。"
        }
        MessageId::HookDetailsReasonPipelineIntent => {
            "命令管道可能已经转换输出，因此缺失或不确定的结构不会被视为高置信度。"
        }
        MessageId::HookDetailsReasonScriptIntent => {
            "脚本或批处理输出可能不代表用户当前关注点，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonWrapperLowConfidence => {
            "包装器、远程或容器上下文会让目标视图变得不明确，因此需要先验证。"
        }
        MessageId::HookDetailsReasonInteractiveIntent => {
            "交互式输出不是稳定的诊断快照，因此只显示采样提示。"
        }
        MessageId::HookDetailsReasonActiveRunDeferred => {
            "已有另一个 Agent 运行中，因此这个成功命令发现会等待并在显示前重新检查。"
        }
        MessageId::HookDetailsReasonUserContinuedInput => {
            "用户已经继续输入其他内容，因此这个成功命令发现不会打断。"
        }
        MessageId::HookDetailsReasonNonDiagnosticSuccessCommand => {
            "该命令不像明确的诊断快照，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonFeedbackNoisy => {
            "之前的用户反馈表明类似发现噪声较高，因此会降低打扰。"
        }
        MessageId::HookDetailsReasonIgnoredSameFinding => "用户之前在本会话中忽略过匹配的发现。",
        MessageId::HookDetailsReasonSameCardAlreadyRendered => {
            "这个发现键已经展示过同等或更高严重级别的卡片。"
        }
        MessageId::HookDetailsReasonInterruptionBudget => {
            "最近类似卡片已经使用了本会话的打扰预算。"
        }
        MessageId::HookDetailsReasonLowConfidence => "证据不完整，需要先做只读验证再给出更强判断。",
        MessageId::HookDetailsReasonDiagnosticIntent => "明确的诊断命令且证据充分。",
        MessageId::HookDetailsReasonOtherIntent => "没有识别到明确的诊断意图。",
        MessageId::HookDetailsTopicLine => "主题: {topic}; 实体: {entity}",
        MessageId::HookDetailsOriginLine => "命令来源: {origin}",
        MessageId::HookDetailsSuppressionKeyLine => "抑制键: {key}",
        MessageId::HookDetailsOutputRefLine => "输出捕获: {ref}",
        MessageId::HookDetailsCreatedAtLine => "创建时间: {created_at}",
        MessageId::HookDetailsPromptHintLine => "提示词线索: {hint}",
        MessageId::HookDetailsRecommendedSkillLine => "推荐 skill: {skill}",
        MessageId::HookDetailsReadOnlyCliHintLine => "只读 CLI 提示: {hint}",
        MessageId::HookDetailsFooter => "分析仍需要确认。",
        MessageId::RuntimeDetailsUnavailableTitle => "详情不可用",
        MessageId::RuntimeDetailsUnavailableBody => {
            "{id} 不可用；请对审批或活动 id 使用 Details 操作"
        }
        MessageId::ActivityTitle => "活动",
        MessageId::ActivityDetailsTitle => "活动详情",
        MessageId::ActivityRunLabel => "运行",
        MessageId::ActivityDetailLabel => "详情",
        MessageId::ActivitySkillLabel => "技能",
        MessageId::ActivitySkillUpdatedStatus => "已更新",
        MessageId::ActivityToolLabel => "Tool",
        MessageId::ActivityToolOutputLabel => "Tool 输出",
        MessageId::ActivityShellLabel => "Shell",
        MessageId::ActivityStatusLoading => "加载中",
        MessageId::ActivityStatusLoaded => "已加载",
        MessageId::ActivityStatusFailed => "失败",
        MessageId::ActivityStatusCalled => "已调用",
        MessageId::ActivityStatusRequested => "请求审批",
        MessageId::ActivityStatusCaptured => "已捕获",
        MessageId::ActivityStatusCompleted => "已完成",
        MessageId::ActivityStatusError => "错误",
        MessageId::ActivityStatusInterrupted => "已中断",
        MessageId::ActivityToolCalledSummary => "{tool} 已调用：{preview}",
        MessageId::ActivityToolRequestedSummary => "{tool} 请求审批：{preview}",
        MessageId::ActivityToolOutputCapturedSummary => "{stream} 已捕获",
        MessageId::ActivityProviderNativeShellBypassSummary => {
            "{tool} 已由 provider 自动批准：{preview}"
        }
        MessageId::ActivityToolNeedsForegroundShellSummary => {
            "可能需要前台 shell；[Send to shell] {handoff}"
        }
        MessageId::ActivityShellHandoffSentSummary => "{approval} 已发送到 shell",
        MessageId::ToolCardReadFileLabel => "读取",
        MessageId::ToolCardWriteFileLabel => "写入",
        MessageId::ToolCardEditFileLabel => "编辑",
        MessageId::ToolCardSearchFilesLabel => "搜索",
        MessageId::ToolCardFindFilesLabel => "查找文件",
        MessageId::ToolCardListDirectoryLabel => "列目录",
        MessageId::ToolCardShellLabel => "Shell",
        MessageId::ToolCardWebFetchLabel => "网页读取",
        MessageId::ToolCardWebSearchLabel => "网页搜索",
        MessageId::ToolCardSkillLabel => "技能",
        MessageId::ToolCardAgentLabel => "Agent",
        MessageId::ToolCardMemoryLabel => "记忆",
        MessageId::ToolCardEvidenceLabel => "Shell 证据",
        MessageId::ToolCardCustomToolLabel => "自定义工具",
        MessageId::ToolCardCalledStatus => "已调用",
        MessageId::ToolCardRequestedStatus => "请求审批",
        MessageId::ToolCardAutoApprovedStatus => "自动批准",
        MessageId::ToolCardCapturedStatus => "已捕获",
        MessageId::ToolCardCompletedStatus => "已完成",
        MessageId::ToolCardFailedStatus => "失败",
        MessageId::ToolCardDuplicateStatus => "重复请求",
        MessageId::ToolCardInterruptedStatus => "已中断",
        MessageId::ToolCardReadOnlyIntent => "只读操作",
        MessageId::ToolCardWriteIntent => "将修改工作区状态",
        MessageId::ToolCardExecuteIntent => "将运行命令",
        MessageId::ToolCardNetworkIntent => "将访问网络",
        MessageId::ToolCardContextIntent => "将更新上下文",
        MessageId::ToolCardCustomIntent => "自定义工具调用",
        MessageId::ToolCardApprovalRequiredAction => "需要审批",
        MessageId::ToolCardWriteCompletedResult => "写入完成",
        MessageId::ToolCardEditCompletedResult => "编辑完成",
        MessageId::ToolCardSkillAvailableResult => "指令已可用",
        MessageId::ToolCardShellEvidenceDeliveredResult => "正文未重复输出",
        MessageId::ToolCardShellEvidenceListResult => "命令历史已交给 Agent",
        MessageId::ToolCardShellEvidenceReadResult => "Shell 输出片段已交给 Agent",
        MessageId::ToolCardShellEvidenceAlreadyDeliveredResult => "最近输出已交付，正文不重复显示",
        MessageId::ToolCardShellEvidenceFailedResult => "Shell 证据不可用",
        MessageId::ToolCardShellEvidenceDuplicateResult => "Provider 重复发送了相同 Shell 证据请求",
        MessageId::ToolCardShellEvidenceMetadataMetric => "证据元数据: {count} 行",
        MessageId::ToolCardOutputCapturedResult => "输出已捕获",
        MessageId::ToolCardLinesReturnedResult => "返回 {count} 行",
        MessageId::ToolCardStdoutMetric => "stdout: {count} 行",
        MessageId::ToolCardStderrMetric => "stderr: {count} 行",
        MessageId::ToolCardTruncatedMetric => "已截断",
        MessageId::MarkdownCodeLabel => "代码",
        MessageId::MarkdownCodeWithLanguageLabel => "代码: {language}",
        MessageId::MarkdownTableLabel => "表格",
        MessageId::RecommendationTitle => "推荐",
        MessageId::RecommendationNextStepTitle => "建议下一步",
        MessageId::AnalysisResultTitle => "分析结果",
        MessageId::RecommendationEmptyBody => "没有命令推荐",
        MessageId::RecommendationFooter => "仅展示：未执行任何命令",
        MessageId::RecommendationNoSelectableTitle => "没有可选择的推荐",
        MessageId::RecommendationNoSelectableBody => "当前还没有可选择的推荐",
        MessageId::RecommendationUnavailableTitle => "推荐不可用",
        MessageId::RecommendationUnavailableBody => "推荐 {index} 不可用；请选择 1..{total}",
        MessageId::RecommendationSelectedTitle => "已选择推荐",
        MessageId::RecommendationSelectedBody => "已选择推荐 {index}",
        MessageId::RecommendationCopiedTitle => "复制推荐",
        MessageId::RecommendationCopiedBody => "复制推荐 {index}",
        MessageId::RecommendationInsertTitle => "插入推荐",
        MessageId::RecommendationInsertBody => "已准备推荐 {index}，等待手动输入",
        MessageId::RecommendationDetailsTitle => "推荐详情",
        MessageId::RecommendationDetailsBody => "推荐 {index} 的详情",
        MessageId::RecommendationDisplayOnlyBody => "仅展示：命令未执行；复制或重新输入后才会运行",
        MessageId::RecommendationCopyOnlyBody => "仅复制：命令只展示给你复制，没有执行。",
        MessageId::RecommendationInsertOnlyBody => {
            "Insert 只会成为待编辑输入；没有提交，也没有写入子 shell。"
        }
        MessageId::RecommendationDetailsOnlyBody => "仅查看详情：决定输入或复制前先检查命令。",
        MessageId::HealthBannerTitle => "健康检查",
        MessageId::HealthStartupLabel => "健康",
        MessageId::HealthBannerTryLabel => "可输入",
        MessageId::HealthBannerFindingLabel => "发现",
        MessageId::HealthBannerEvidenceLabel => "证据",
        MessageId::HealthBannerMoreFindingsLabel => "另有 {count} 个问题",
        MessageId::HealthBannerUnavailableLabel => "不可用",
        MessageId::HealthBannerFindingsSection => "发现的问题",
        MessageId::HealthBannerSuggestedPromptSection => "建议下一步",
        MessageId::HealthBannerSuggestedPromptIntro => "以下提示词可直接输入给 Agent：",
        MessageId::HealthSeverityOk => "正常",
        MessageId::HealthSeverityWarning => "警告",
        MessageId::HealthSeverityCritical => "严重",
        MessageId::HealthSeverityDegraded => "降级",
        MessageId::HealthSeverityUnavailable => "不可用",
        MessageId::HealthMetricCpu => "CPU",
        MessageId::HealthMetricCpuLoadPerCore => "1分钟负载",
        MessageId::HealthMetricLoad1mShort => "1分钟",
        MessageId::HealthMetricLoad5mShort => "5分钟",
        MessageId::HealthMetricLoadValue => "{load} / {cores}核（{ratio}倍）",
        MessageId::HealthMetricCpuPerCoreUnit => "倍",
        MessageId::HealthMetricCpuUsed => "CPU 已用",
        MessageId::HealthMetricHost => "主机",
        MessageId::HealthMetricMemory => "内存",
        MessageId::HealthMetricMemoryAvailable => "内存可用",
        MessageId::HealthMetricMemoryUsed => "内存已用",
        MessageId::HealthMetricSwap => "Swap",
        MessageId::HealthMetricSwapUsed => "Swap 已用",
        MessageId::HealthMetricDisk => "磁盘",
        MessageId::HealthMetricDiskUsed => "磁盘已用",
        MessageId::HealthMetricDiskMountUsed => "磁盘 {mount} 已用",
        MessageId::HealthMetricPressure => "负载",
        MessageId::HealthMetricLevels => "资源",
        MessageId::HealthMetricSignal => "信号",
        MessageId::HealthMetricService => "服务",
        MessageId::HealthEvidenceDiskAvailable => "可用 {gib} GiB",
        MessageId::HealthEvidenceMount => "挂载点 {mount}",
        MessageId::HealthEvidenceOomKilledProcess => "杀掉 {process}",
        MessageId::HealthEvidenceOomCgroup => "cgroup {cgroup}",
        MessageId::HealthEvidenceOomOneHourCount => "1h OOM {count}",
        MessageId::HealthEvidenceOomTwentyFourHourCount => "24h OOM {count}",
        MessageId::HealthEvidenceOomVictimKilledAgo => "{age}前杀掉 {subject}",
        MessageId::HealthEvidenceOomVictimKilled => "杀掉 {subject}",
        MessageId::HealthEvidenceOomAge => "OOM 发生于 {age}前",
        MessageId::HealthOomScopeMemcg => "cgroup 内存限制触发",
        MessageId::HealthOomScopeHost => "整机内存不足触发",
        MessageId::HealthOomScopeCpuset => "cpuset/NUMA 范围内存不足",
        MessageId::HealthOomScopeMemoryPolicy => "内存策略范围不足",
        MessageId::HealthOomScopeUnknown => "OOM 触发范围未识别",
        MessageId::HealthTryAnalyzeMemoryPressure => "分析内存压力",
        MessageId::HealthTryCheckSwapPressure => "检查换页压力",
        MessageId::HealthTryCheckRecentOom => "分析最近一次 OOM 原因",
        MessageId::HealthTryInspectDiskUsage => "检查磁盘占用",
        MessageId::HealthTryInspectServiceStatus => "检查服务状态",
        MessageId::HealthTryInspectHighLoad => "分析高负载",
        MessageId::HealthTryInspectProcessMemory => "检查 {process} 内存",
        MessageId::HealthTryReviewUnavailableChecks => "查看缺失检查",
        MessageId::HealthPromptAnalyzeMemoryPressure => {
            "分析内存压力，找出可能影响当前 shell 的主要占用来源。"
        }
        MessageId::HealthPromptCheckSwapPressure => "检查是否存在换页压力，并找出主要相关进程。",
        MessageId::HealthPromptCheckRecentOom => {
            "帮我分析最近一次 OOM 的原因，重点看被杀进程、cgroup 和当时内存水位。"
        }
        MessageId::HealthPromptInspectDiskUsage => "检查高风险挂载点占用，并给出安全清理目标。",
        MessageId::HealthPromptInspectServiceStatus => {
            "检查配置服务状态，并分析最近可能的失败原因。"
        }
        MessageId::HealthPromptInspectHighLoad => "分析当前高负载，判断主要来自 CPU 还是 IO 压力。",
        MessageId::HealthPromptInspectProcessMemory => {
            "帮我分析最近一次 OOM 为什么杀掉 {process}，重点看 cgroup 和内存上限。"
        }
        MessageId::HealthPromptReviewUnavailableChecks => {
            "说明启动健康检查为什么不可用，以及如何恢复这些检查。"
        }
        MessageId::HealthInsightMemoryAvailableLow => {
            "可用内存偏低；命令可能变慢，新进程也可能启动失败"
        }
        MessageId::HealthInsightDiskHigh => "最高风险挂载点磁盘水位偏高；写入或构建可能很快失败",
        MessageId::HealthInsightRecentOom => {
            "最近一次 OOM 已发生；应回溯被杀进程、cgroup 和当时内存水位"
        }
        MessageId::HealthInsightCpuLoadHigh => "最近多个窗口负载都偏高；命令响应可能变慢",
        MessageId::HealthInsightSwapPressure => {
            "Swap 使用偏高并伴随内存压力；频繁换页可能拖慢命令响应"
        }
        MessageId::HealthInsightServiceState => {
            "服务单元 {service} 当前 {observed}，预期 {expected}"
        }
        MessageId::HealthInsightGeneric => "启动健康检查发现了值得排查的信号",
        MessageId::HealthUnavailablePermissionDenied => "权限不足",
        MessageId::HealthUnavailableCommandMissing => "命令缺失",
        MessageId::HealthUnavailableTimeout => "检查超时",
        MessageId::HealthUnavailableUnsupported => "平台不支持",
        MessageId::HealthUnavailableParseError => "解析失败",
        MessageId::HealthFindingPlatformUnsupported => "平台不支持",
        MessageId::HealthFindingCoreCollectorUnavailable => "核心检查不可用",
        MessageId::HealthFindingCpuLoadHigh => "系统负载偏高",
        MessageId::HealthFindingMemoryAvailableLow => "可用内存偏低",
        MessageId::HealthFindingSwapPressure => "换页压力有上下文",
        MessageId::HealthFindingDiskHigh => "磁盘水位偏高",
        MessageId::HealthFindingRecentOom => "近期 OOM 信号",
        MessageId::HealthFindingKernelPanic => "近期内核 panic",
        MessageId::HealthFindingServiceFailed => "服务失败",
        MessageId::HealthFindingServiceInactive => "服务未运行",
        MessageId::HealthTryReasonMemoryLow => "可用内存偏低",
        MessageId::HealthTryReasonSwapWithContext => "swap 偏高且有压力上下文",
        MessageId::HealthTryReasonRecentOom => "近期 OOM 值得回溯原因",
        MessageId::HealthTryReasonDiskHigh => "磁盘空间紧张",
        MessageId::HealthTryReasonServiceState => "配置服务状态异常",
        MessageId::HealthTryReasonHighLoad => "最近负载持续偏高",
        MessageId::HealthTryReasonMissingCoreCheck => "核心健康检查缺失",
        MessageId::ToolOutputStdoutCapturedSummary => "stdout 已捕获",
        MessageId::ToolOutputStderrCapturedSummary => "stderr 已捕获",
        MessageId::ToolSummaryExit => "退出码 {exit}",
        MessageId::ToolSummaryBlocked => "tool 请求被 shell broker guard 阻止",
        MessageId::ToolSummaryTimedOut => "tool 请求超时",
        MessageId::ToolSummaryFailed => "tool 请求失败",
        MessageId::QuestionTitle => "Agent 问题",
        MessageId::QuestionDefaultPrompt => "Agent 需要你的输入",
        MessageId::QuestionAnswerLabel => "回答",
        MessageId::QuestionSelectOneLabel => "选择一项:",
        MessageId::QuestionSelectMultipleLabel => "选择一项或多项:",
        MessageId::QuestionOtherEmptyLabel => "其他...",
        MessageId::QuestionKeysPrefix => "按键: ",
        MessageId::QuestionInstructionMoveTypeSend => "左/右移动 | 输入回答 | Enter 发送",
        MessageId::QuestionInstructionMoveToggleSend => "左/右移动 | Space 切换 | Enter 发送",
        MessageId::QuestionInstructionMoveSend => "左/右移动 | Enter 发送",
        MessageId::QuestionInstructionTypeSend => "输入回答 | Enter 发送",
        MessageId::QuestionInstructionNoAnswer => "没有可选择的回答。",
        MessageId::QuestionNoPendingTitle => "没有待回答问题",
        MessageId::QuestionNoPendingBody => "当前没有等待回答的 Agent 问题。",
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
        MessageId::ApprovalHookFallbackMessage => "Hook 需要您的审批才能继续执行。",
        // Registry slash commands
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
    }
}
