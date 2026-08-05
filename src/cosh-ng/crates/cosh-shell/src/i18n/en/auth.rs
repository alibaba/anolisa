use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    match id {
        MessageId::AuthSelectProviderQuestion => {
            Some("\u{1f511} Authentication Required \u{2014} Select your AI provider:")
        }
        _ => None,
    }
}
