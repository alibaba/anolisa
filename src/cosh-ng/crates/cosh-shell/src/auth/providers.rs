use crate::runtime::prelude::{AuthFieldInfo, AuthProviderInfo};

/// Display label for the Aliyun provider, highlighting that it is free to use.
///
/// Kept as a single constant so the builtin template, the control-protocol
/// normalizer, and `label_for_provider_type` all present identical text.
pub(crate) const ALIYUN_PROVIDER_LABEL: &str = "Aliyun Authentication（免费可用）";

/// Builtin provider templates (mirroring cosh-core's auth.rs).
///
/// Aliyun is listed first so the free option is the default choice on first-run
/// authentication; DashScope and OpenAI Compatible follow.
pub(crate) fn builtin_auth_providers() -> Vec<AuthProviderInfo> {
    vec![
        AuthProviderInfo {
            id: "aliyun".into(),
            label: ALIYUN_PROVIDER_LABEL.into(),
            fields: vec![
                AuthFieldInfo {
                    name: "access_key_id".into(),
                    label: "Access Key ID".into(),
                    hint: Some("https://ram.console.aliyun.com/manage/ak".into()),
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthFieldInfo {
                    name: "access_key_secret".into(),
                    label: "Access Key Secret".into(),
                    hint: None,
                    secret: true,
                    required: true,
                    placeholder: None,
                },
                AuthFieldInfo {
                    name: "model".into(),
                    label: "Model".into(),
                    hint: Some("默认: qwen3.7-plus".into()),
                    secret: false,
                    required: false,
                    placeholder: Some("qwen3.7-plus".into()),
                },
            ],
        },
        AuthProviderInfo {
            id: "dashscope".into(),
            label: "DashScope (百炼)".into(),
            fields: vec![
                AuthFieldInfo {
                    name: "api_key".into(),
                    label: "API Key".into(),
                    hint: Some("https://dashscope.console.aliyun.com/apiKey".into()),
                    secret: true,
                    required: true,
                    placeholder: Some("sk-...".into()),
                },
                AuthFieldInfo {
                    name: "model".into(),
                    label: "Model".into(),
                    hint: Some("默认: qwen3.7-plus, e.g. qwen3.7-max, deepseek-v4-pro".into()),
                    secret: false,
                    required: false,
                    placeholder: Some("qwen3.7-plus".into()),
                },
            ],
        },
        AuthProviderInfo {
            id: "openai_compat".into(),
            label: "OpenAI Compatible".into(),
            fields: vec![
                AuthFieldInfo {
                    name: "base_url".into(),
                    label: "Base URL".into(),
                    hint: Some("e.g. https://api.openai.com/v1".into()),
                    secret: false,
                    required: true,
                    placeholder: Some("https://api.openai.com/v1".into()),
                },
                AuthFieldInfo {
                    name: "api_key".into(),
                    label: "API Key".into(),
                    hint: Some("sk-...".into()),
                    secret: true,
                    required: true,
                    placeholder: Some("sk-...".into()),
                },
                AuthFieldInfo {
                    name: "model".into(),
                    label: "Model".into(),
                    hint: Some("e.g. qwen3.7-max, deepseek-v4-pro".into()),
                    secret: false,
                    required: true,
                    placeholder: None,
                },
            ],
        },
    ]
}

/// Normalize a provider list arriving from cosh-core's control protocol so the
/// shell always presents Aliyun first with the free-availability hint.
///
/// cosh-core may still emit the legacy order (dashscope / openai_compat /
/// aliyun); rather than change cosh-core this round, the shell reorders the list
/// (stable — non-Aliyun providers keep their relative order) and patches the
/// Aliyun label. Ordering keys on the stable `aliyun` provider id, never the
/// label text.
pub(crate) fn normalize_provider_order(
    mut providers: Vec<AuthProviderInfo>,
) -> Vec<AuthProviderInfo> {
    for provider in providers.iter_mut() {
        if provider.id == "aliyun" {
            provider.label = ALIYUN_PROVIDER_LABEL.to_string();
        }
    }
    providers.sort_by_key(|provider| u8::from(provider.id != "aliyun"));
    providers
}

/// Returns the builtin base URL for a given provider id, if any.
pub(crate) fn builtin_base_url_for_provider(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "dashscope" => Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        "aliyun" => None,
        _ => None,
    }
}

/// Returns the default model for a given provider id, if any.
pub(crate) fn default_model_for_provider(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "dashscope" => Some("qwen3.7-plus"),
        "aliyun" => Some("qwen3.7-plus"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, label: &str) -> AuthProviderInfo {
        AuthProviderInfo {
            id: id.into(),
            label: label.into(),
            fields: Vec::new(),
        }
    }

    #[test]
    fn builtin_lists_aliyun_first_with_free_hint() {
        let providers = builtin_auth_providers();
        assert_eq!(providers[0].id, "aliyun");
        assert!(
            providers[0].label.contains("免费可用"),
            "aliyun label should advertise free availability, got {:?}",
            providers[0].label
        );
    }

    #[test]
    fn builtin_keeps_dashscope_and_openai_compat() {
        let providers = builtin_auth_providers();
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["aliyun", "dashscope", "openai_compat"]);
    }

    #[test]
    fn normalize_reorders_legacy_list_aliyun_first() {
        // Legacy order emitted by cosh-core's control protocol.
        let legacy = vec![
            provider("dashscope", "DashScope (百炼)"),
            provider("openai_compat", "OpenAI Compatible"),
            provider("aliyun", "Aliyun Authentication"),
        ];
        let normalized = normalize_provider_order(legacy);
        let ids: Vec<&str> = normalized.iter().map(|p| p.id.as_str()).collect();
        // Aliyun promoted to front; other providers keep their relative order.
        assert_eq!(ids, ["aliyun", "dashscope", "openai_compat"]);
        assert!(normalized[0].label.contains("免费可用"));
    }
}
