use crate::i18n::{I18n, MessageId};

pub(super) fn display_agent_error(error: &str, i18n: &I18n) -> String {
    if error == "analysis returned an error" {
        i18n.t(MessageId::AgentStatusAnalysisReturnedError)
            .to_string()
    } else {
        error.to_string()
    }
}
