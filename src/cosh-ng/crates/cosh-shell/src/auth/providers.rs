use crate::runtime::prelude::{AuthFieldInfo, AuthProviderInfo};

/// Builtin provider templates (mirroring cosh-tui's auth.rs).
pub(crate) fn builtin_auth_providers() -> Vec<AuthProviderInfo> {
    vec![
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

/// Returns the builtin base URL for a given provider id, if any.
pub(crate) fn builtin_base_url_for_provider(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "dashscope" => Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        _ => None,
    }
}

/// Returns the default model for a given provider id, if any.
pub(crate) fn default_model_for_provider(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "dashscope" => Some("qwen3.7-plus"),
        _ => None,
    }
}
