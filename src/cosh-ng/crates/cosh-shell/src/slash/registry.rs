use crate::MessageId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandState {
    Public,
    PublicMinimal,
    Contextual,
    Diagnostic,
    Hidden,
    Removed,
}

impl SlashCommandState {
    fn is_exact_control(self) -> bool {
        matches!(
            self,
            Self::Public
                | Self::PublicMinimal
                | Self::Contextual
                | Self::Diagnostic
                | Self::Hidden
                | Self::Removed
        )
    }

    fn is_visible(self) -> bool {
        matches!(self, Self::Public | Self::PublicMinimal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary_id: MessageId,
    pub group: Option<&'static str>,
    pub scope: &'static str,
    pub state: SlashCommandState,
}

pub fn slash_command_registry() -> &'static [SlashCommandSpec] {
    &[
        SlashCommandSpec {
            name: "/help",
            usage: "/help",
            summary_id: MessageId::HelpSummaryHelp,
            group: None,
            scope: "read-only",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/health",
            usage: "/health",
            summary_id: MessageId::HelpSummaryHealth,
            group: Some("Health"),
            scope: "read-only",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/auth",
            usage: "/auth",
            summary_id: MessageId::HelpSummaryAuth,
            group: None,
            scope: "config",
            state: SlashCommandState::Contextual,
        },
        SlashCommandSpec {
            name: "/config",
            usage: "/config language [auto|en-US|zh-CN]",
            summary_id: MessageId::HelpSummaryConfig,
            group: Some("Config"),
            scope: "config",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/session",
            usage: "/session [new|status|list|resume <id>|clear <id>...|clear --all|compact [status|cancel]]",
            summary_id: MessageId::HelpSummarySession,
            group: Some("Sessions"),
            scope: "session",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/new",
            usage: "/new",
            summary_id: MessageId::HelpSummarySession,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/resume",
            usage: "/resume [id]",
            summary_id: MessageId::HelpSummarySession,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/recommendations",
            usage: "/recommendations [on|off|status|privacy|clear]",
            summary_id: MessageId::HelpSummaryRecommendations,
            group: Some("Config"),
            scope: "config",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/mode",
            usage: "/mode approval [recommend|auto|trust]",
            summary_id: MessageId::HelpSummaryModeApproval,
            group: Some("Modes"),
            scope: "session",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/mode",
            usage: "/mode analysis [smart|auto|manual]",
            summary_id: MessageId::HelpSummaryModeAnalysis,
            group: Some("Modes"),
            scope: "session",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/agent",
            usage: "/agent",
            summary_id: MessageId::HelpSummaryAgent,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/explain",
            usage: "/explain",
            summary_id: MessageId::HelpSummaryExplain,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/cancel",
            usage: "/cancel",
            summary_id: MessageId::HelpSummaryCancel,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/details",
            usage: "/details <id>",
            summary_id: MessageId::HelpSummaryDetails,
            group: None,
            scope: "read-only",
            state: SlashCommandState::Contextual,
        },
        SlashCommandSpec {
            name: "/audit",
            usage: "/audit",
            summary_id: MessageId::HelpSummaryAudit,
            group: None,
            scope: "read-only",
            state: SlashCommandState::Contextual,
        },
        SlashCommandSpec {
            name: "/hooks",
            usage: "/hooks",
            summary_id: MessageId::HelpSummaryHooks,
            group: Some("Hooks"),
            scope: "read-only",
            state: SlashCommandState::PublicMinimal,
        },
        SlashCommandSpec {
            name: "/extensions",
            usage: "/extensions [list|detail] [name]",
            summary_id: MessageId::HelpSummaryExtensions,
            group: Some("Registry"),
            scope: "config",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/skills",
            usage: "/skills [list|detail] [name]",
            summary_id: MessageId::HelpSummarySkills,
            group: Some("Registry"),
            scope: "read-only",
            state: SlashCommandState::Public,
        },
        SlashCommandSpec {
            name: "/select",
            usage: "/select N",
            summary_id: MessageId::HelpSummarySelect,
            group: None,
            scope: "display-only",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/copy",
            usage: "/copy N",
            summary_id: MessageId::HelpSummaryCopy,
            group: None,
            scope: "display-only",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/send-to-shell",
            usage: "/send-to-shell <id>",
            summary_id: MessageId::HelpSummaryDetails,
            group: None,
            scope: "shell",
            state: SlashCommandState::Contextual,
        },
        SlashCommandSpec {
            name: "/debug",
            usage: "/debug session",
            summary_id: MessageId::HelpSummaryDebug,
            group: None,
            scope: "debug",
            state: SlashCommandState::Diagnostic,
        },
        SlashCommandSpec {
            name: "/clear",
            usage: "/clear",
            summary_id: MessageId::HelpSummaryClear,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/shell",
            usage: "/shell",
            summary_id: MessageId::HelpSummaryShell,
            group: None,
            scope: "session",
            state: SlashCommandState::Hidden,
        },
        SlashCommandSpec {
            name: "/approval-mode",
            usage: "/approval-mode [recommend|auto|trust]",
            summary_id: MessageId::HelpSummaryApprovalModeRemoved,
            group: None,
            scope: "removed",
            state: SlashCommandState::Removed,
        },
        SlashCommandSpec {
            name: "/allow",
            usage: "/allow <id>",
            summary_id: MessageId::HelpSummaryApprovalModeRemoved,
            group: None,
            scope: "removed",
            state: SlashCommandState::Removed,
        },
        SlashCommandSpec {
            name: "/approve",
            usage: "/approve <id>",
            summary_id: MessageId::HelpSummaryApprovalModeRemoved,
            group: None,
            scope: "removed",
            state: SlashCommandState::Removed,
        },
        SlashCommandSpec {
            name: "/deny",
            usage: "/deny <id>",
            summary_id: MessageId::HelpSummaryApprovalModeRemoved,
            group: None,
            scope: "removed",
            state: SlashCommandState::Removed,
        },
        SlashCommandSpec {
            name: "/answer",
            usage: "/answer <text>",
            summary_id: MessageId::HelpSummaryApprovalModeRemoved,
            group: None,
            scope: "removed",
            state: SlashCommandState::Removed,
        },
    ]
}

pub fn active_slash_commands() -> impl Iterator<Item = &'static str> {
    slash_command_registry()
        .iter()
        .filter(|spec| spec.state.is_visible())
        .map(|spec| spec.name)
}

pub fn exact_slash_control_commands() -> impl Iterator<Item = &'static str> {
    slash_command_registry()
        .iter()
        .filter(|spec| spec.state.is_exact_control())
        .map(|spec| spec.name)
}

pub fn visible_slash_commands() -> impl Iterator<Item = &'static SlashCommandSpec> {
    slash_command_registry()
        .iter()
        .filter(|spec| spec.state.is_visible() && spec.group.is_some())
}

pub fn active_slash_hint_commands() -> impl Iterator<Item = &'static str> {
    slash_command_registry()
        .iter()
        .filter(|spec| spec.state.is_visible())
        .map(|spec| spec.name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::input::{InputClassifier, InputDecision, InterceptReason};

    use super::{
        active_slash_commands, active_slash_hint_commands, exact_slash_control_commands,
        slash_command_registry, visible_slash_commands, SlashCommandState,
    };

    #[test]
    fn removed_decision_commands_are_registered_but_not_discoverable() {
        for name in ["/approve", "/deny", "/answer", "/allow"] {
            let spec = slash_command_registry()
                .iter()
                .find(|spec| spec.name == name)
                .expect("removed decision command spec");

            assert_eq!(spec.state, SlashCommandState::Removed);
            assert!(exact_slash_control_commands().any(|candidate| candidate == name));
            assert!(!active_slash_commands().any(|candidate| candidate == name));
            assert!(!active_slash_hint_commands().any(|candidate| candidate == name));
            assert!(!visible_slash_commands().any(|candidate| candidate.name == name));
        }
    }

    #[test]
    fn approval_mode_is_removed_not_active() {
        let approval_mode = slash_command_registry()
            .iter()
            .find(|spec| spec.name == "/approval-mode")
            .expect("approval-mode removed spec");

        assert_eq!(approval_mode.state, SlashCommandState::Removed);
        assert!(!active_slash_commands().any(|name| name == "/approval-mode"));
    }

    #[test]
    fn public_discovery_excludes_card_first_and_diagnostic_commands() {
        let visible = visible_slash_commands()
            .map(|spec| spec.usage)
            .collect::<Vec<_>>();

        assert!(visible.contains(&"/config language [auto|en-US|zh-CN]"));
        assert!(visible
            .iter()
            .any(|usage| usage.starts_with("/session [new|status|list|resume")));
        assert!(visible.contains(&"/mode approval [recommend|auto|trust]"));
        assert!(visible.contains(&"/mode analysis [smart|auto|manual]"));
        assert!(visible.contains(&"/hooks"));
        assert!(visible.contains(&"/recommendations [on|off|status|privacy|clear]"));
        assert!(!visible.iter().any(|usage| usage.starts_with("/agent")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/explain")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/cancel")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/details")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/audit")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/select")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/copy")));
        assert!(!visible.iter().any(|usage| usage.starts_with("/debug")));
    }

    #[test]
    fn recommendations_is_public_local_config_control() {
        let spec = slash_command_registry()
            .iter()
            .find(|spec| spec.name == "/recommendations")
            .expect("recommendations spec");

        assert_eq!(spec.group, Some("Config"));
        assert_eq!(spec.scope, "config");
        assert_eq!(spec.state, SlashCommandState::Public);
        assert!(exact_slash_control_commands().any(|name| name == "/recommendations"));
    }

    #[test]
    fn public_hint_commands_are_public_or_public_minimal_only() {
        for name in active_slash_hint_commands() {
            let spec = slash_command_registry()
                .iter()
                .find(|spec| spec.name == name)
                .expect("hint command in registry");
            assert!(matches!(
                spec.state,
                SlashCommandState::Public | SlashCommandState::PublicMinimal
            ));
        }
        for hidden in [
            "/agent",
            "/explain",
            "/cancel",
            "/details",
            "/audit",
            "/select",
            "/copy",
            "/send-to-shell",
            "/debug",
            "/resume",
            "/new",
            "/skill",
            "/approval-mode",
            "/allow",
            "/approve",
            "/deny",
            "/answer",
        ] {
            assert!(
                !active_slash_hint_commands().any(|candidate| candidate == hidden),
                "{hidden} must not be suggested"
            );
        }
    }

    #[test]
    fn input_classifier_intercepts_every_exact_registry_command() {
        let classifier = InputClassifier::default();
        for name in exact_slash_control_commands() {
            assert_eq!(
                classifier.classify(&format!("{name} arg")),
                InputDecision::Intercept {
                    input: format!("{name} arg"),
                    reason: InterceptReason::Slash,
                },
                "{name} must be intercepted before Bash"
            );
        }
        assert_eq!(
            classifier.classify("/tmp/tool --help"),
            InputDecision::SendToShell("/tmp/tool --help".to_string())
        );
    }

    #[test]
    fn shell_marker_exact_tokens_match_registry() {
        let registry = exact_slash_control_commands().collect::<BTreeSet<_>>();
        let marker = include_str!("../shell_host/marker.rs");
        let marker_tokens = marker
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with('/'))
            .flat_map(|line| line.trim_end_matches(')').split('|'))
            .map(str::trim)
            .filter(|token| {
                token
                    .as_bytes()
                    .get(1)
                    .is_some_and(|byte| byte.is_ascii_alphabetic())
            })
            .collect::<BTreeSet<_>>();

        for token in &registry {
            assert!(
                marker_tokens.contains(token),
                "shell marker is missing registry token {token}"
            );
        }
        for token in &marker_tokens {
            assert!(
                registry.contains(token),
                "shell marker has unregistered slash token {token}"
            );
        }
    }
}
