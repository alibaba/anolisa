use serde_json::Value;

pub trait ProviderProfile: Send + Sync {
    fn name(&self) -> &str;

    fn max_tokens_field(&self) -> &str {
        "max_tokens"
    }

    fn thinking_field(&self) -> Option<&str> {
        None
    }

    fn adjust_request(&self, _body: &mut Value) {}

    fn auth_header_value(&self, api_key: &str) -> String {
        format!("Bearer {api_key}")
    }
}

pub struct GenericProfile;

impl ProviderProfile for GenericProfile {
    fn name(&self) -> &str {
        "generic"
    }
}

pub struct DashScopeProfile;

impl ProviderProfile for DashScopeProfile {
    fn name(&self) -> &str {
        "dashscope"
    }

    fn thinking_field(&self) -> Option<&str> {
        Some("reasoning_content")
    }
}

pub struct OpenAIProfile;

impl ProviderProfile for OpenAIProfile {
    fn name(&self) -> &str {
        "openai"
    }

    fn max_tokens_field(&self) -> &str {
        "max_completion_tokens"
    }
}

pub struct DeepSeekProfile;

impl ProviderProfile for DeepSeekProfile {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn thinking_field(&self) -> Option<&str> {
        Some("reasoning_content")
    }
}

pub fn profile_from_name(name: &str) -> Box<dyn ProviderProfile> {
    match name {
        "dashscope" => Box::new(DashScopeProfile),
        "openai" => Box::new(OpenAIProfile),
        "deepseek" => Box::new(DeepSeekProfile),
        _ => Box::new(GenericProfile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_profile_defaults() {
        let p = GenericProfile;
        assert_eq!(p.name(), "generic");
        assert_eq!(p.max_tokens_field(), "max_tokens");
        assert!(p.thinking_field().is_none());
        assert_eq!(p.auth_header_value("sk-test"), "Bearer sk-test");
    }

    #[test]
    fn dashscope_profile_thinking_field() {
        let p = DashScopeProfile;
        assert_eq!(p.name(), "dashscope");
        assert_eq!(p.thinking_field(), Some("reasoning_content"));
        assert_eq!(p.max_tokens_field(), "max_tokens");
    }

    #[test]
    fn openai_profile_max_completion_tokens() {
        let p = OpenAIProfile;
        assert_eq!(p.name(), "openai");
        assert_eq!(p.max_tokens_field(), "max_completion_tokens");
        assert!(p.thinking_field().is_none());
    }

    #[test]
    fn deepseek_profile_thinking_field() {
        let p = DeepSeekProfile;
        assert_eq!(p.name(), "deepseek");
        assert_eq!(p.thinking_field(), Some("reasoning_content"));
    }

    #[test]
    fn profile_from_name_routing() {
        assert_eq!(profile_from_name("dashscope").name(), "dashscope");
        assert_eq!(profile_from_name("openai").name(), "openai");
        assert_eq!(profile_from_name("deepseek").name(), "deepseek");
        assert_eq!(profile_from_name("unknown").name(), "generic");
        assert_eq!(profile_from_name("").name(), "generic");
    }
}
