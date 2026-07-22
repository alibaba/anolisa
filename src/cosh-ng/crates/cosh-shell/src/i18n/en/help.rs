use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::HelpTitle => "Slash commands",
        MessageId::HelpFooter => "Mode: {mode}. Strategy: {strategy}.",
        MessageId::HelpGroupConfig => "Config",
        MessageId::HelpGroupModes => "Modes",
        MessageId::HelpGroupHooks => "Hooks",
        MessageId::HelpSummaryHelp => "show command reference",
        MessageId::HelpSummaryAuth => "configure AI provider credentials",
        MessageId::HelpSummaryConfig => "configure UI language",
        MessageId::HelpSummaryRecommendations => {
            "manage recommendations; analysis sends bounded activity to the provider, and local clear does not control provider retention"
        }
        MessageId::HelpSummaryModeApproval => "change approval mode",
        MessageId::HelpSummaryModeAnalysis => {
            "choose suggested mode, automatic analysis, or no proactive assistance"
        }
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
        MessageId::HelpGroupSessions => "Sessions",
        MessageId::HelpSummarySession => "discover, resume, and clear Agent sessions",
        MessageId::HelpGroupRegistry => "Registry",
        MessageId::HelpSummaryExtensions => "list/manage cosh-core extensions",
        MessageId::HelpSummarySkills => "list/inspect cosh-core skills",
        MessageId::SlashExtensionsTitle => "Extensions",
        MessageId::SlashSkillsTitle => "Skills",
        MessageId::SlashRegistryUnavailable => {
            "This feature requires cosh-core backend."
        }
        MessageId::SlashHooksShellSection => "Shell Hooks",
        MessageId::SlashHooksAgentSection => "Agent Hooks",
        MessageId::SlashHooksAgentUnavailable => "(cosh-core backend unavailable)",
        MessageId::SlashExtensionsEmptyBody => "No extensions installed.",
        MessageId::SlashSkillsEmptyBody => "No skills found.",
        _ => return None,
    })
}
