use super::MessageId;

pub(super) fn message(id: MessageId) -> &'static str {
    match id {
        MessageId::StartupTitle => "cosh-shell",
        MessageId::StartupAdapterLine => "Adapter: {adapter} · Shell: {shell} · Mode: {mode}",
        MessageId::StartupCwdLine => "cwd: {cwd}",
        MessageId::StartupCommandsLine => "/help · /mode · /hooks",
        MessageId::StartupHooksNoneSummary => "Startup hooks: none configured.",
        MessageId::StartupHooksCompletedSummary => {
            "Startup hooks: built-in read-only checks completed."
        }
        MessageId::StartupHooksFindingsHeading => "Startup findings",
        MessageId::StartupHooksRustProjectFinding => {
            "Rust project detected from `Cargo.toml`; `/skill` can show project-oriented Agent capabilities."
        }
        MessageId::StartupHooksNoFindings => {
            "No startup findings from built-in read-only checks."
        }
        MessageId::StartupHooksReadOnlyNote => {
            "`cosh-shell` only inspected lightweight startup context."
        }
        MessageId::HelpTitle => "Slash commands",
        MessageId::HelpFooter => "Mode: {mode}. Strategy: {strategy}.",
        MessageId::HelpGroupConfig => "Config",
        MessageId::HelpGroupModes => "Modes",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "show command reference",
        MessageId::HelpSummaryAuth => "configure AI provider credentials",
        MessageId::HelpSummaryConfig => "configure UI language",
        MessageId::HelpSummaryModeApproval => "change approval mode",
        MessageId::HelpSummaryModeAnalysis => "change analysis strategy",
        MessageId::HelpSummaryAgent => "start an explicit Agent request",
        MessageId::HelpSummaryExplain => "analyze the last failed command",
        MessageId::HelpSummaryCancel => "cancel active Agent work",
        MessageId::HelpSummaryDetails => "inspect approval/activity details",
        MessageId::HelpSummaryAudit => "show audit entry points",
        MessageId::HelpSummaryHooks => "show hook status",
        MessageId::HelpSummarySelect => "show a displayed recommendation",
        MessageId::HelpSummaryCopy => "copy a displayed recommendation",
        MessageId::HelpSummaryDebug => "show session debug details",
        MessageId::HelpSummaryClear => "clear local shell state",
        MessageId::HelpSummaryShell => "return to shell input",
        MessageId::HelpSummarySkill => "show skill routing hints",
        MessageId::HelpSummaryApprovalModeRemoved => "removed approval-mode alias",
        MessageId::SlashHintTitle => "Slash command hint",
        MessageId::SlashHintPrefix => "Prefix: {prefix}",
        MessageId::SlashHintCurrentMode => "Current mode: {mode}",
        MessageId::SlashHintFooter => {
            "Type a full command and press Enter; paths like /tmp/foo stay in shell."
        }
        MessageId::SlashUnknownTitle => "Slash command",
        MessageId::SlashUnknownBody => "Unknown slash command: {command}",
        MessageId::SlashUnknownSuggestionBody => "Did you mean {command}?",
        MessageId::SlashUnknownFooter => "Use /help to see available commands.",
        MessageId::SlashInfoAuditTitle => "Audit",
        MessageId::SlashInfoAuditApprovalsBody => {
            "Approval decisions are available with Details actions."
        }
        MessageId::SlashInfoAuditActivityBody => {
            "Activity output refs are available with Details actions."
        }
        MessageId::SlashInfoAuditFooter => "Audit views are read-only; no shell command runs.",
        MessageId::SlashInfoConfigTitle => "Config",
        MessageId::SlashInfoConfigLanguageLine => "language: {effective} source: {source}",
        MessageId::SlashInfoConfigLanguageEffectiveLine => {
            "language: {effective} effective, setting: {setting}, source: {source}"
        }
        MessageId::SlashInfoConfigPathLine => "config: {path}",
        MessageId::SlashInfoConfigDebugActivityLine => {
            "debug activity: {state} (ui.debug or COSH_SHELL_DEBUG=1)"
        }
        MessageId::SlashInfoConfigAnalysisStrategyLine => {
            "analysis strategy: /mode analysis smart|auto|manual"
        }
        MessageId::SlashInfoConfigRenderFallbackLine => {
            "render fallback: set COSH_SHELL_RENDER=plain before starting cosh-shell."
        }
        MessageId::SlashInfoConfigFooter => {
            "Use /config language [auto|en-US|zh-CN]. Saved language takes effect next startup."
        }
        MessageId::SlashInfoSkillTitle => "Skill",
        MessageId::SlashInfoSkillHookRoutingBody => {
            "Hook routing hints can route Agent analysis toward a skill."
        }
        MessageId::SlashInfoSkillRegistryBody => {
            "No external skill registry is configured for this shell session."
        }
        MessageId::SlashInfoSkillFooter => {
            "Skill hooks are advisory and still go through governance."
        }
        MessageId::ConfigInvalidLanguageBody => "Invalid language: {language}",
        MessageId::ConfigSupportedLanguagesFooter => "Supported: auto, en-US, zh-CN.",
        MessageId::ConfigUnknownKeyBody => "Unknown config key: {key}",
        MessageId::ConfigHomeMissingBody => "HOME is not set; cannot persist config.",
        MessageId::ConfigHomeMissingFooter => "Set HOME or edit config manually.",
        MessageId::ConfigUnchangedTitle => "Config unchanged",
        MessageId::ConfigNoFileChangedBody => "No config file was changed.",
        MessageId::ConfigSavedTitle => "Config saved",
        MessageId::ConfigSavedValueLine => "Saved ui.{setting} = \"{value}\".",
        MessageId::ConfigCurrentSessionLanguageLine => "Current session language: {language}.",
        MessageId::ConfigSavedFooter => "Saved setting takes effect next startup.",
        MessageId::ConfigSaveFailedTitle => "Config save failed",
        MessageId::ConfigSaveFailedBody => "Config save failed: {error}",
        MessageId::ConfigSavePromptTitle => "Save config?",
        MessageId::ConfigFileLine => "file: {path}",
        MessageId::ConfigPendingChangeLine => "ui.{setting}: {before} -> {after}",
        MessageId::ConfigSaveButton => "Save",
        MessageId::ConfigCancelButton => "Cancel",
        MessageId::ConfigApplyKeysFooter => "Keys: Left/Right select | Enter apply | Esc cancel",
        MessageId::ConfigLanguageTitle => "Language",
        MessageId::ConfigLanguageAutoLine => "auto    Follow LC_ALL/LC_MESSAGES/LANG",
        MessageId::ConfigLanguageEnLine => "en-US   English",
        MessageId::ConfigLanguageZhLine => "zh-CN   Simplified Chinese",
        MessageId::ConfigLanguageKeysFooter => {
            "Keys: Left/Right select | Enter choose | Esc cancel"
        }
        MessageId::SlashHooksRegisteredTitle => "Hook status",
        MessageId::SlashHooksNoHooksBody => "No hooks registered.",
        MessageId::SlashHooksStatusCountLine => {
            "Registered: {total}; enabled: {enabled}; disabled: {disabled}."
        }
        MessageId::SlashHooksStatusSourcesLine => {
            "Sources: builtin={builtin}; user={user}; project={project}."
        }
        MessageId::SlashHooksStatusProjectTrustLine => {
            "Project trust: trusted={trusted}; untrusted={untrusted}."
        }
        MessageId::SlashHooksFooterCount => "{count} hook(s) registered.",
        MessageId::SlashHooksFooterMutedTargets => {
            "{count} hook(s) registered. Muted targets: {targets}."
        }
        MessageId::SlashHooksTargetMutedTitle => "Hook target muted",
        MessageId::SlashHooksTargetMutedBody => "Muted hook target '{target}' for this session.",
        MessageId::SlashHooksTargetMutedFooter => {
            "Muted findings are still recorded in /hooks history."
        }
        MessageId::SlashHooksTargetUnmutedTitle => "Hook target unmuted",
        MessageId::SlashHooksTargetUnmutedBody => "Unmuted hook target '{target}'.",
        MessageId::SlashHooksTargetNotMutedBody => "Hook target '{target}' was not muted.",
        MessageId::SlashHooksEnabledTitle => "Hook enabled",
        MessageId::SlashHooksEnabledBody => "Hook '{id}' enabled.",
        MessageId::SlashHooksDisabledTitle => "Hook disabled",
        MessageId::SlashHooksDisabledBody => "Hook '{id}' disabled.",
        MessageId::SlashHooksHistoryTitle => "Hook history",
        MessageId::SlashHooksHistoryEmptyBody => "No hook findings recorded in this session.",
        MessageId::SlashHooksHistoryFooter => {
            "Recent findings are read-only; Analyze still requires user confirmation."
        }
        MessageId::SlashHooksEventsTitle => "Hook display events",
        MessageId::SlashHooksEventsEmptyBody => "No hook display events recorded in this session.",
        MessageId::SlashHooksEventsFooter => {
            "Events are session-local and contain policy metadata, not command output."
        }
        MessageId::SlashHooksUsageTitle => "Usage",
        MessageId::SlashHooksUsageListLine => "/hooks                - show hook status",
        MessageId::SlashHooksUsageHistoryLine => {
            "/hooks history        - show recent hook findings"
        }
        MessageId::SlashHooksUsageEventsLine => {
            "/hooks events         - show recent hook display events"
        }
        MessageId::SlashHooksUsageAnalyzeLine => {
            "/hooks analyze <id>   - analyze a hint finding"
        }
        MessageId::SlashHooksUsageIgnoreLine => {
            "/hooks ignore <id>    - ignore a hint finding"
        }
        MessageId::SlashHooksUsageDetailsLine => {
            "/hooks details <id>   - show hook finding details"
        }
        MessageId::SlashHooksUsageFeedbackLine => {
            "/hooks feedback noisy|useful <id> - record feedback"
        }
        MessageId::SlashHooksUsageClearFeedbackLine => {
            "/hooks clear-feedback - clear hook feedback preferences"
        }
        MessageId::SlashHooksUsageMuteLine => "/hooks mute <target>  - mute a topic or hook id",
        MessageId::SlashHooksUsageUnmuteLine => {
            "/hooks unmute <target>- unmute a topic or hook id"
        }
        MessageId::SlashHooksUsageTrustProjectLine => {
            "/hooks trust-project  - trust project hooks for this session"
        }
        MessageId::SlashHooksUsageUntrustProjectLine => {
            "/hooks untrust-project- untrust project hooks for this session"
        }
        MessageId::SlashHooksUsageClearProjectTrustLine => {
            "/hooks clear-project-trust - clear project hook trust store"
        }
        MessageId::SlashHooksUsageEnableLine => "/hooks enable <id>    - enable a hook",
        MessageId::SlashHooksUsageDisableLine => "/hooks disable <id>   - disable a hook",
        MessageId::SlashHooksProjectTrustedTitle => "Project hooks trusted",
        MessageId::SlashHooksProjectUntrustedTitle => "Project hooks untrusted",
        MessageId::SlashHooksProjectTrustNoHooksBody => {
            "No project hooks are registered in this session."
        }
        MessageId::SlashHooksProjectTrustedBody => "{count} project hook(s) marked trusted.",
        MessageId::SlashHooksProjectUntrustedBody => "{count} project hook(s) marked untrusted.",
        MessageId::SlashHooksProjectTrustNoChangeFooter => "No trust state changed.",
        MessageId::SlashHooksProjectTrustPersistedFooter => {
            "Trust persisted; disabled hooks remain disabled."
        }
        MessageId::SlashHooksProjectTrustRemovedFooter => {
            "Trust removed from persistent store; disabled hooks remain disabled."
        }
        MessageId::SlashHooksProjectTrustPersistenceFailedFooter => {
            "Session state changed, but persistence failed: {failures}"
        }
        MessageId::SlashHooksProjectTrustClearedTitle => "Project hook trust cleared",
        MessageId::SlashHooksProjectTrustClearedBody => {
            "{count} project hook(s) marked untrusted."
        }
        MessageId::SlashHooksProjectTrustClearedFooter => {
            "Project hook trust store cleared; current session project hooks are untrusted."
        }
        MessageId::SlashHooksProjectTrustClearFailedFooter => {
            "Current session project hooks marked untrusted, but clearing persistent trust store failed: {error}"
        }
        MessageId::SlashHooksFeedbackUsageBody => "/hooks feedback noisy|useful <finding_id>",
        MessageId::SlashHooksFeedbackTitle => "Hook feedback",
        MessageId::SlashHooksFeedbackFindingNotFoundBody => {
            "Finding '{finding_id}' was not found in this session."
        }
        MessageId::SlashHooksFeedbackFindingNotFoundFooter => {
            "Use /hooks history to copy a recent finding id."
        }
        MessageId::SlashHooksFeedbackRecordedTitle => "Hook feedback recorded",
        MessageId::SlashHooksFeedbackRecordedBody => {
            "Feedback '{feedback}' recorded for finding '{finding_id}'."
        }
        MessageId::SlashHooksFeedbackHookLine => "Hook: {hook_id}.",
        MessageId::SlashHooksFeedbackPolicyKeyLine => "Policy key: {key}.",
        MessageId::SlashHooksFeedbackPersistedFooter => {
            "Feedback persisted. It affects display strategy only."
        }
        MessageId::SlashHooksFeedbackPersistenceFailedFooter => {
            "Session feedback recorded, but persistence failed: {error}"
        }
        MessageId::SlashHooksFeedbackClearedTitle => "Hook feedback cleared",
        MessageId::SlashHooksFeedbackClearedBody => {
            "{count} feedback preference(s) cleared from this session."
        }
        MessageId::SlashHooksFeedbackClearedFooter => "Hook feedback preferences cleared.",
        MessageId::SlashHooksFeedbackClearFailedFooter => {
            "Session feedback cleared, but persistent store clear failed: {error}"
        }
        MessageId::DebugSessionTitle => "Session debug",
        MessageId::DebugAdapterLine => "adapter: {value}",
        MessageId::DebugProviderInvocationLine => "provider invocation: {value}",
        MessageId::DebugProviderCommittedSessionLine => "provider committed session: {value}",
        MessageId::DebugActiveRunLine => "active run: {value}",
        MessageId::DebugQueuedRunsLine => "queued runs: {value}",
        MessageId::DebugProviderPendingSessionLine => "provider pending session: {value}",
        MessageId::DebugProviderInitializeSeenLine => "provider initialize seen: {value}",
        MessageId::DebugHostExecutedShellResultLine => "host-executed shell result: {value}",
        MessageId::DebugSelectedShellExecutionPathLine => "selected shell execution path: {value}",
        MessageId::DebugLatestProviderRequestLine => "latest provider request: {value}",
        MessageId::DebugLatestToolUseLine => "latest tool use id: {value}",
        MessageId::DebugLatestRecoveryStatusLine => "latest recovery status: {value}",
        MessageId::DebugLatestRecoveryReasonLine => "latest recovery reason: {value}",
        MessageId::DebugUnknownTargetBody => "Unknown debug target: {target}",
        MessageId::DebugUnknownTargetFooter => "Use /debug session.",
        MessageId::CommandRemovedTitle => "Command removed",
        MessageId::ApprovalModeRemovedBody => "/approval-mode is not supported.",
        MessageId::ApprovalModeRemovedFooter => "Use /mode approval [recommend|auto|trust].",
        MessageId::RemovedDecisionCommandBody => "{command} is no longer a supported input command.",
        MessageId::RemovedApprovalDecisionFooter => {
            "Use the approval card buttons instead; nothing was sent to the shell."
        },
        MessageId::RemovedQuestionAnswerFooter => {
            "Answer from the question card instead; nothing was sent to the shell."
        },
        MessageId::ModeTitle => "Mode",
        MessageId::ModesTitle => "Modes",
        MessageId::ModeApprovalLine => "approval: {mode}",
        MessageId::ModeAnalysisLine => "analysis: {mode}",
        MessageId::ModeSummaryFooter => {
            "Use /mode approval [recommend|auto|trust] or /mode analysis [smart|auto|manual]."
        }
        MessageId::ModeRemovedTitle => "Mode command removed",
        MessageId::ModeRemovedBody => "/mode {mode} is not supported.",
        MessageId::ModeRemovedFooter => "Use /mode approval {mode}.",
        MessageId::ModeLanguageBody => "Language is persistent config, not a runtime mode.",
        MessageId::ModeLanguageFooter => "Use /config language [auto|en-US|zh-CN].",
        MessageId::ModeUnknownBody => "Unknown mode: {mode}",
        MessageId::ModeUnknownFooter => {
            "Use /mode approval recommend|auto|trust or /mode analysis smart|auto|manual."
        }
        MessageId::ApprovalModeTitle => "Approval mode",
        MessageId::ApprovalModeSetBody => "Mode set to {mode}.",
        MessageId::ApprovalModeUnknownBody => "Unknown approval mode: {mode}",
        MessageId::ApprovalModeUsageFooter => "Use /mode approval recommend|auto|trust.",
        MessageId::ApprovalModeRecommendFooter => {
            "Agent explains and suggests; no tool calls are emitted."
        }
        MessageId::ApprovalModeAutoFooter => {
            "Read-only tools auto-approved; risky requests need confirmation."
        }
        MessageId::ApprovalModeTrustFooter => {
            "All tools auto-approved; audit trail preserved via control protocol."
        }
        MessageId::ApprovalModeTrustConfirmationTitle => "Trust confirmation required",
        MessageId::ApprovalModeTrustConfirmationBody => {
            "Trust mode auto-approves provider tool requests for this session."
        }
        MessageId::ApprovalModeTrustConfirmationCommandBody => {
            "Run /mode approval trust confirm to enable it explicitly."
        }
        MessageId::ApprovalModeTrustConfirmationFooter => {
            "Recommend or auto mode remains active until confirmation."
        }
        MessageId::ApprovalModeCardTitle => "User mode",
        MessageId::ApprovalModeCardCurrentLine => "Current: {mode}",
        MessageId::ApprovalModeCardRecommendLine => {
            "{marker}[ recommend ] Explain and suggest only"
        }
        MessageId::ApprovalModeCardAutoLine => {
            "{marker}[ auto      ] Read-only auto-approved; risky needs confirmation"
        }
        MessageId::ApprovalModeCardTrustLine => {
            "{marker}[ trust     ] All tools auto-approved with audit trail"
        }
        MessageId::ApprovalModeCardFooter => "Keys: Left/Right select | Enter apply | Esc cancel",
        MessageId::ApprovalModeRemainsBody => "Mode remains {mode}.",
        MessageId::ApprovalModeCancelBody => "Mode unchanged: {mode}.",
        MessageId::ApprovalModeCancelFooter => "No shell command ran.",
        MessageId::AnalysisModeTitle => "Analysis mode",
        MessageId::AnalysisModeCurrentBody => "Current: {mode}",
        MessageId::AnalysisModeSetBody => "Mode set to {mode}.",
        MessageId::AnalysisModeUnknownBody => "Unknown analysis mode: {mode}",
        MessageId::AnalysisModeUsageFooter => "Use /mode analysis smart|auto|manual.",
        MessageId::AnalysisModeSmartFooter => {
            "Hooks evaluate on failure; findings shown for review."
        }
        MessageId::AnalysisModeAutoFooter => {
            "Hooks evaluate on failure; Agent auto-triggered for failed commands."
        }
        MessageId::AnalysisModeManualFooter => {
            "Hooks and automatic analysis disabled; use slash commands to trigger."
        }
        MessageId::AgentThinking => "Thinking...",
        MessageId::AgentThinkingElapsed => "Thinking... {elapsed}s · {detail}",
        MessageId::AgentRecoveryTitle => "Agent recovery",
        MessageId::AgentRecoveryFreshTurnBody => {
            "Using a fresh provider turn for shell evidence recovery."
        }
        MessageId::AgentRecoveryContinuityBody => "Provider session continuity may be degraded.",
        MessageId::AgentStatusTitle => "Agent",
        MessageId::AgentStillWorking => "Still working... {elapsed}s · {detail}",
        MessageId::AgentStatusFooter => "Ctrl+C cancels · [Cancel]",
        MessageId::AgentStatusStarting => "starting",
        MessageId::AgentStatusWaitingBackend => "waiting for Agent backend",
        MessageId::AgentStatusStreaming => "streaming",
        MessageId::AgentStatusReceivingResponse => "receiving Agent response",
        MessageId::AgentStatusSkill => "skill",
        MessageId::AgentStatusLoadingSkill => "loading skill {skill}",
        MessageId::AgentStatusApproval => "approval",
        MessageId::AgentStatusWaitingApprovalTool => "waiting for approval: tool {tool}",
        MessageId::AgentStatusQuestion => "question",
        MessageId::AgentStatusWaitingUserAnswer => "waiting for user answer: {question}",
        MessageId::AgentStatusWaitingApprovalCommand => "waiting for approval: {command}",
        MessageId::AgentStatusTool => "tool",
        MessageId::AgentStatusCapturingToolOutput => "capturing output from {tool_id}",
        MessageId::AgentStatusToolCompleted => "{tool_id} completed with status {status}",
        MessageId::AgentStatusCompleted => "completed",
        MessageId::AgentStatusFailed => "failed",
        MessageId::AgentStatusCancelled => "cancelled",
        MessageId::AgentStatusRunningApprovedProviderTool => "running approved provider tool",
        MessageId::AgentStatusSkillFailed => "{skill} failed: {error}",
        MessageId::AgentProviderTimeoutDroppedQueuedBody => {
            "{dropped} queued requests skipped after provider timeout"
        }
        MessageId::AgentCancellationRequestedTitle => "Agent cancellation requested",
        MessageId::AgentCancellationRequestedBody => "Stopping active Agent run...",
        MessageId::AgentCancelledReasonLabel => "Reason:",
        MessageId::AgentCancelledUserRequestedReason => "user requested cancellation",
        MessageId::AgentResponseTitle => "Agent",
        MessageId::AgentGovernanceTitle => "Governance",
        MessageId::AgentGovernanceStatusLine => "Status: {phase}",
        MessageId::AgentGovernanceReasonLine => "Reason: {reason}",
        MessageId::AgentGovernanceSummaryLine => "Summary: {summary}",
        MessageId::AgentGovernanceErrorLine => "Error: {error}",
        MessageId::AgentGovernanceSkillLoadingLine => "Skill loading: {skill}",
        MessageId::AgentGovernanceSkillLoadedLine => "Skill loaded: {skill}",
        MessageId::AgentGovernanceSkillFailedLine => "Skill failed: {skill}",
        MessageId::AgentGovernanceToolOutputLine => "Tool output: {tool_id} {stream}",
        MessageId::AgentGovernanceToolCompletedLine => "Tool completed: {tool_id}",
        MessageId::AgentGovernanceApprovalRequiredLine => "Approval required: {subject}",
        MessageId::AgentGovernanceShellCommandSubject => "Shell command",
        MessageId::AgentGovernanceBashCommandSubject => "Bash command",
        MessageId::AgentGovernanceToolSubject => "{tool} tool",
        MessageId::AgentGovernanceBlockedUserApprovalLine => "Blocked: user approval required",
        MessageId::AgentGovernanceQuestionLine => "Question: {question}",
        MessageId::AgentRecommendedCommandsLabel => "recommended commands:",
        MessageId::InterceptNoticeTitle => "AI request",
        MessageId::InterceptNoticeBody => "Sending input to Agent: {input}",
        MessageId::InterceptNoticeFooter => "Shell input was intercepted before Bash ran it.",
        MessageId::FailedCommandCardTitle => "Command failed",
        MessageId::FailedCommandCardBody => {
            "`{command}` exited with code {exit_code}; id: {id}"
        }
        MessageId::FailedCommandCardFooter => "[Analyze] [Dismiss] [Details]",
        MessageId::FailedAnalysisCancelledTitle => "Agent cancelled",
        MessageId::FailedAnalysisCancelledBody => "cancelled pending analysis for `{command}`",
        MessageId::FailedAnalysisCancelNoActiveBody => {
            "no active Agent run is currently waiting for cancellation"
        }
        MessageId::FailedAnalysisCancelledFooter => "Shell remains active.",
        MessageId::AnalysisSkippedTitle => "Analysis skipped",
        MessageId::AnalysisSkippedBody => "skipped repeated failure analysis for `{command}`",
        MessageId::AnalysisSkippedFooter => {
            "Too many consecutive failures for this command. Wait before retrying."
        }
        MessageId::HookAutoAnalyzedTitle => "Hook auto-analyzed",
        MessageId::HookAutoAnalyzedBody => "`{command}` exited with code {exit_code}",
        MessageId::HookAutoAnalyzedFooter => "Agent analysis is starting.",
        MessageId::AgentQueuedTitle => "Agent queued",
        MessageId::AgentQueuedBodyCommand => "Captured failed command: {command}",
        MessageId::AgentQueuedBodyActive => "Current Agent run is still streaming.",
        MessageId::AgentQueuedFooter => {
            "This failure will be analyzed after the current Agent run finishes."
        }
        MessageId::HookFindingTitle => "Hook finding",
        MessageId::HookFindingFooter => "Use /hooks analyze|ignore|details {hint_id}.",
        MessageId::HookFindingMarkdownTitle => "Command hook finding",
        MessageId::HookFindingMarkdownHookLine => "- Hook: `{hook_id}`.",
        MessageId::HookFindingMarkdownSeverityLine => "- Severity: `{severity}`.",
        MessageId::HookFindingMarkdownFindingLine => "- Finding: {finding}.",
        MessageId::HookFindingMarkdownOutputRefLine => "- Output id: `{output_ref}`.",
        MessageId::HookFindingMarkdownSuggestionLine => "- Suggestion: {suggestion}.",
        MessageId::HookFindingMarkdownRelatedTitle => "- Related findings:",
        MessageId::HookFindingMarkdownRelatedLine => "  - `{hook_id}` [{severity}]: {finding}",
        MessageId::HookFindingMarkdownAgentFollowUpLine => {
            "Agent follow-up must use bounded cosh-shell evidence before claiming details."
        }
        MessageId::HookHintTitle => "Hook hint",
        MessageId::HookHintNotFoundBody => "Hook hint '{hint_id}' was not found in this session.",
        MessageId::HookHintNotFoundFooter => "Use /hooks history to copy a recent finding id.",
        MessageId::HookHintNoFindingBody => "Hook hint '{hint_id}' has no finding attached.",
        MessageId::HookHintBlockUnavailableBody => {
            "Command block '{block_id}' is no longer available."
        }
        MessageId::HookHintIgnoredTitle => "Hook hint ignored",
        MessageId::HookHintIgnoredBody => "Ignored hook hint '{hint_id}' for this session.",
        MessageId::HookHintIgnoredFooter => "Future matching findings are downgraded by policy.",
        MessageId::HookHintUsageTitle => "Usage",
        MessageId::HookHintUsageBody => "/hooks analyze|ignore|details <hint_id>",
        MessageId::HookFindingDetailsTitle => "Hook finding details",
        MessageId::HookConsultationHookLabel => "Hook",
        MessageId::HookConsultationConfidenceReasonLine => {
            "Confidence: {confidence}; reason: {reason}"
        }
        MessageId::HookConsultationFindingLine => "Finding: {finding}",
        MessageId::HookConsultationSuggestionLine => "Recommended action: {suggestion}",
        MessageId::HookConsultationAnalyzeAction => "Analyze",
        MessageId::HookConsultationIgnoreAction => "Ignore",
        MessageId::HookDetailsConfidenceLine => "Confidence: {confidence}; policy reason: {reason}",
        MessageId::HookDetailsUserInterestLine => "User-interest reason: {code}: {description}",
        MessageId::HookDetailsReasonLookupIntent => {
            "the command targets a specific process or search, so the finding stays low-interruption"
        }
        MessageId::HookDetailsReasonPipelineIntent => {
            "the command pipeline may have transformed output, so missing or uncertain schema is not treated as high-confidence"
        }
        MessageId::HookDetailsReasonScriptIntent => {
            "script or batch output may not reflect the user's immediate focus, so interruption is reduced"
        }
        MessageId::HookDetailsReasonWrapperLowConfidence => {
            "wrapper or remote/container context makes the target view ambiguous, so verification is required"
        }
        MessageId::HookDetailsReasonInteractiveIntent => {
            "interactive output is not a stable diagnostic snapshot, so only sampling guidance is shown"
        }
        MessageId::HookDetailsReasonActiveRunDeferred => {
            "another Agent run was active, so this success-command finding waits and is rechecked before display"
        }
        MessageId::HookDetailsReasonUserContinuedInput => {
            "the user moved on to another input, so this success-command finding does not interrupt"
        }
        MessageId::HookDetailsReasonNonDiagnosticSuccessCommand => {
            "the command does not look like an explicit diagnostic snapshot, so interruption is reduced"
        }
        MessageId::HookDetailsReasonFeedbackNoisy => {
            "prior user feedback says similar findings are noisy, so interruption is reduced"
        }
        MessageId::HookDetailsReasonIgnoredSameFinding => {
            "the user ignored a matching finding earlier in this session"
        }
        MessageId::HookDetailsReasonSameCardAlreadyRendered => {
            "an equal-or-higher severity card was already shown for this finding key"
        }
        MessageId::HookDetailsReasonInterruptionBudget => {
            "recent similar cards already used the session interruption budget"
        }
        MessageId::HookDetailsReasonLowConfidence => {
            "partial evidence requires read-only verification before stronger claims"
        }
        MessageId::HookDetailsReasonDiagnosticIntent => {
            "explicit diagnostic command with sufficient evidence"
        }
        MessageId::HookDetailsReasonOtherIntent => "no explicit diagnostic intent was identified",
        MessageId::HookDetailsTopicLine => "Topic: {topic}; entity: {entity}",
        MessageId::HookDetailsOriginLine => "Command origin: {origin}",
        MessageId::HookDetailsSuppressionKeyLine => "Suppression key: {key}",
        MessageId::HookDetailsOutputRefLine => "Output capture: {ref}",
        MessageId::HookDetailsCreatedAtLine => "Created at: {created_at}",
        MessageId::HookDetailsPromptHintLine => "Prompt hint: {hint}",
        MessageId::HookDetailsRecommendedSkillLine => "Recommended skill: {skill}",
        MessageId::HookDetailsReadOnlyCliHintLine => "Read-only CLI hint: {hint}",
        MessageId::HookDetailsFooter => "Analyze still requires confirmation.",
        MessageId::RuntimeDetailsUnavailableTitle => "Details unavailable",
        MessageId::RuntimeDetailsUnavailableBody => {
            "{id} is not available; use a Details action with an approval or activity id"
        }
        MessageId::ActivityTitle => "Activity",
        MessageId::ActivityDetailsTitle => "Activity details",
        MessageId::ActivityRunLabel => "Run",
        MessageId::ActivityDetailLabel => "Detail",
        MessageId::ActivitySkillLabel => "Skill",
        MessageId::ActivitySkillUpdatedStatus => "updated",
        MessageId::ActivityToolLabel => "Tool",
        MessageId::ActivityToolOutputLabel => "Tool output",
        MessageId::ActivityShellLabel => "Shell",
        MessageId::ActivityStatusLoading => "loading",
        MessageId::ActivityStatusLoaded => "loaded",
        MessageId::ActivityStatusFailed => "failed",
        MessageId::ActivityStatusCalled => "called",
        MessageId::ActivityStatusRequested => "requested",
        MessageId::ActivityStatusCaptured => "captured",
        MessageId::ActivityStatusCompleted => "completed",
        MessageId::ActivityStatusError => "error",
        MessageId::ActivityStatusInterrupted => "interrupted",
        MessageId::ActivitySkillLoadingSummary => "{skill} loading",
        MessageId::ActivitySkillLoadedSummary => "{skill} loaded",
        MessageId::ActivitySkillFailedSummary => "{skill} failed",
        MessageId::ActivityToolCalledSummary => "{tool} called: {preview}; [Details] {id}",
        MessageId::ActivityToolRequestedSummary => "{tool} requested: {preview}; [Details] {id}",
        MessageId::ActivityToolOutputCapturedSummary => "{stream} captured; [Details] {id}",
        MessageId::ActivityProviderNativeShellBypassSummary => {
            "{tool} auto-approved by provider: {preview}; [Details] {id}"
        }
        MessageId::ActivityToolNeedsForegroundShellSummary => {
            "may require foreground shell; [Send to shell] {handoff}; [Details] {id}"
        }
        MessageId::ActivityShellHandoffSentSummary => "{approval} sent to shell",
        MessageId::MarkdownCodeLabel => "code",
        MessageId::MarkdownCodeWithLanguageLabel => "code: {language}",
        MessageId::MarkdownTableLabel => "table",
        MessageId::RecommendationTitle => "Recommendations",
        MessageId::RecommendationEmptyBody => "No command recommendations",
        MessageId::RecommendationFooter => "[Copy] [Insert] [Details] - display-only",
        MessageId::RecommendationNoSelectableTitle => "No selectable recommendation",
        MessageId::RecommendationNoSelectableBody => {
            "No selectable recommendation is available yet"
        }
        MessageId::RecommendationUnavailableTitle => "Recommendation unavailable",
        MessageId::RecommendationUnavailableBody => {
            "Recommendation {index} is not available; choose 1..{total}"
        }
        MessageId::RecommendationSelectedTitle => "Recommendation selected",
        MessageId::RecommendationSelectedBody => "Selected recommendation {index}",
        MessageId::RecommendationCopiedTitle => "Recommendation copy",
        MessageId::RecommendationCopiedBody => "Copy recommendation {index}",
        MessageId::RecommendationInsertTitle => "Recommendation insert",
        MessageId::RecommendationInsertBody => "Prepared recommendation {index} for manual input",
        MessageId::RecommendationDetailsTitle => "Recommendation details",
        MessageId::RecommendationDetailsBody => "Details for recommendation {index}",
        MessageId::RecommendationDisplayOnlyBody => {
            "Display-only: command was not executed; copy or re-enter it to run"
        }
        MessageId::RecommendationCopyOnlyBody => {
            "Copy-only: command was shown for copying; it was not executed."
        }
        MessageId::RecommendationInsertOnlyBody => {
            "Insert is pending editable input only; nothing was submitted or written to the child shell."
        }
        MessageId::RecommendationDetailsOnlyBody => {
            "Details-only: inspect the command before deciding whether to type or copy it."
        }
        MessageId::ToolOutputStdoutCapturedSummary => "stdout captured; [Details] {id}",
        MessageId::ToolOutputStderrCapturedSummary => "stderr captured; [Details] {id}",
        MessageId::ToolSummaryExit => "exit {exit}",
        MessageId::ToolSummaryBlocked => "tool request blocked by shell broker guard",
        MessageId::ToolSummaryTimedOut => "tool request timed out",
        MessageId::ToolSummaryFailed => "tool request failed",
        MessageId::QuestionTitle => "Agent question",
        MessageId::QuestionAnswerLabel => "Answer",
        MessageId::QuestionSelectOneLabel => "Select one:",
        MessageId::QuestionSelectMultipleLabel => "Select one or more:",
        MessageId::QuestionOtherEmptyLabel => "Other...",
        MessageId::QuestionOtherValueLabel => "Other: {answer}",
        MessageId::QuestionKeysPrefix => "Keys: ",
        MessageId::QuestionInstructionMoveTypeSend => "Left/Right move | type answer | Enter send",
        MessageId::QuestionInstructionMoveToggleSend => {
            "Left/Right move | Space toggle | Enter send"
        }
        MessageId::QuestionInstructionMoveSend => "Left/Right move | Enter send",
        MessageId::QuestionInstructionTypeSend => "Type answer | Enter send",
        MessageId::QuestionInstructionNoAnswer => "No selectable answer is available.",
        MessageId::QuestionNoPendingTitle => "No pending question",
        MessageId::QuestionNoPendingBody => "There is no Agent question waiting for an answer.",
        _ => super::en_approval::message(id),
    }
}
