use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    match id {
        MessageId::AuthSelectProviderQuestion => Some("\u{1f511} 需要认证 \u{2014} 选择 AI 服务："),
        _ => None,
    }
}
