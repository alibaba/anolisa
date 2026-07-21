mod en;
mod en_approval;
mod en_session;
mod message_id;
mod message_id_all;
mod zh;
mod zh_session;

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

    #[test]
    fn all_messages_have_en_and_zh_values() {
        for id in MessageId::ALL {
            assert!(!I18n::new(Language::EnUs).t(*id).trim().is_empty());
            assert!(!I18n::new(Language::ZhCn).t(*id).trim().is_empty());
        }
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
}
