mod en;
mod message_id;
mod zh;

use crate::config::Language;

pub use message_id::MessageId;

#[derive(Debug, Clone, Copy)]
pub struct I18n {
    language: Language,
}

impl I18n {
    pub fn new(language: Language) -> Self {
        Self { language }
    }

    pub fn t(&self, id: MessageId) -> &'static str {
        message(self.language, id)
    }

    pub fn format(&self, id: MessageId, args: &[(&str, &str)]) -> String {
        let mut text = self.t(id).to_string();
        for (key, value) in args {
            text = text.replace(&format!("{{{key}}}"), value);
        }
        text
    }

    pub fn language(&self) -> Language {
        self.language
    }
}

fn message(language: Language, id: MessageId) -> &'static str {
    match language {
        Language::EnUs => en::message(id),
        Language::ZhCn => zh::message(id),
    }
}

#[cfg(test)]
mod tests {
    use super::{I18n, MessageId};
    use crate::config::Language;
    use std::fs;
    use std::path::Path;

    const EXPECTED_CATALOG_DOMAINS: &[&str] = &[
        "activity",
        "agent",
        "approval",
        "config",
        "debug",
        "health",
        "help",
        "hook_details",
        "hooks",
        "insight",
        "modes",
        "question",
        "recommendation",
        "session",
        "startup",
    ];

    fn catalog_modules(directory: &str) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/i18n")
            .join(directory);
        let mut modules = fs::read_dir(path)
            .expect("read i18n catalog directory")
            .map(|entry| entry.expect("read i18n catalog entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .map(|path| {
                path.file_stem()
                    .expect("i18n catalog module stem")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        modules.sort();
        modules
    }

    #[test]
    fn language_catalog_modules_match_message_id_domains() {
        let domains = EXPECTED_CATALOG_DOMAINS
            .iter()
            .map(|domain| (*domain).to_owned())
            .collect::<Vec<_>>();

        assert_eq!(catalog_modules("message_id"), domains);
        assert_eq!(catalog_modules("en"), domains);
        assert_eq!(catalog_modules("zh"), domains);
    }

    #[test]
    fn all_messages_have_en_and_zh_values() {
        for id in MessageId::ALL {
            assert!(!I18n::new(Language::EnUs).t(*id).trim().is_empty());
            assert!(!I18n::new(Language::ZhCn).t(*id).trim().is_empty());
        }
    }

    #[test]
    fn message_id_keeps_fieldless_enum_compatibility() {
        for (ordinal, id) in MessageId::ALL.iter().copied().enumerate() {
            assert_eq!(id as usize, ordinal);
        }
        assert_eq!(MessageId::AgentControlQueueFullBody as usize, 750);
        assert_eq!(MessageId::SlashInvalidArgumentsTitle as usize, 751);
        assert_eq!(MessageId::SlashQuotedArgumentsUnsupported as usize, 752);
    }

    #[test]
    fn format_replaces_known_args_and_keeps_missing_args() {
        let i18n = I18n::new(Language::EnUs);
        let text = i18n.format(
            MessageId::StartupAdapterLine,
            &[("adapter", "qwen"), ("shell", "bash"), ("approval", "auto")],
        );

        assert!(text.contains("qwen"));
        assert!(text.contains("bash"));
        assert!(text.contains("{analysis}"));
    }

    #[test]
    fn zh_catalog_keeps_protocol_tokens_stable() {
        let i18n = I18n::new(Language::ZhCn);

        assert!(i18n
            .t(MessageId::ModeLanguageFooter)
            .contains("/config language"));
        assert!(i18n
            .t(MessageId::RecommendationFooter)
            .contains("未执行任何命令"));
        assert!(!i18n
            .t(MessageId::RecommendationFooter)
            .contains("[Details]"));
        assert!(i18n.t(MessageId::ApprovalToolInputLabel).contains("Tool"));
        assert!(i18n.t(MessageId::HelpSummaryConfig).contains("语言"));
        assert!(i18n
            .t(MessageId::AgentRecoveryFreshTurnBody)
            .contains("provider"));
        assert!(i18n
            .t(MessageId::AgentStatusWaitingApprovalTool)
            .contains("tool"));
        assert_eq!(
            i18n.t(MessageId::ApprovalResolutionAutoApprovedTitle),
            "已自动批准"
        );
    }

    #[test]
    fn quoted_argument_error_is_localized() {
        let en = I18n::new(Language::EnUs);
        assert_eq!(
            en.t(MessageId::SlashInvalidArgumentsTitle),
            "Invalid slash arguments"
        );
        assert_eq!(
            en.t(MessageId::SlashQuotedArgumentsUnsupported),
            "Quoted arguments are not supported. Use /mode approval trust confirm instead."
        );

        let zh = I18n::new(Language::ZhCn);
        assert_eq!(
            zh.t(MessageId::SlashInvalidArgumentsTitle),
            "Slash 参数错误"
        );
        assert_eq!(
            zh.t(MessageId::SlashQuotedArgumentsUnsupported),
            "不支持带引号的参数。本例请改用 /mode approval trust confirm。"
        );
    }
}
