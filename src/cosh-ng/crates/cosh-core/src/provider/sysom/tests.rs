use super::*;

#[test]
fn region_id_strips_single_zone_suffix() {
    assert_eq!(
        region_id_from_zone_id("cn-hangzhou-j").as_deref(),
        Some("cn-hangzhou")
    );
    assert_eq!(
        region_id_from_zone_id("cn-beijing").as_deref(),
        Some("cn-beijing")
    );
    assert_eq!(region_id_from_zone_id(""), None);
}

#[test]
fn generate_console_url_uses_region_and_instance_id() {
    assert_eq!(
        generate_console_url("i-test123", "cn-hangzhou"),
        "https://alinux.console.aliyun.com/cn-hangzhou/guide/cosh?instance=i-test123"
    );
}

#[test]
fn build_request_preserves_user_provided_secrets() {
    let provider = SysomProvider {
        endpoint: DEFAULT_ENDPOINT.to_string(),
        credentials: std::sync::RwLock::new(SysomCredentials {
            access_key_id: "test-id".to_string(),
            access_key_secret: "test-secret".to_string(),
            security_token: None,
        }),
        is_sts: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        instance_id: None,
    };
    let secret = "short-provider-secret";
    let messages = vec![Message::user(&format!("api_key={secret}"))];

    let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
    let payload = body.to_string();

    assert!(payload.contains(secret), "{payload}");
    assert!(!payload.contains("<redacted>"), "{payload}");
}

fn test_provider() -> SysomProvider {
    SysomProvider {
        endpoint: DEFAULT_ENDPOINT.to_string(),
        credentials: std::sync::RwLock::new(SysomCredentials {
            access_key_id: "test-id".to_string(),
            access_key_secret: "test-secret".to_string(),
            security_token: None,
        }),
        is_sts: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        instance_id: None,
    }
}

/// Parses the inner request the SysOM wrapper actually sends.
fn wire_inner(body: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(
        body["llmParamString"]
            .as_str()
            .expect("llmParamString is a string"),
    )
    .expect("inner request is JSON")
}

#[test]
fn extra_params_cannot_raise_the_wire_output_cap() {
    // Regression (#2240): the compaction budget reserves `O` from the same
    // resolver that produced `max_tokens`, so a wire request must never be
    // allowed to spend more than the reserve — not even via extra_params.
    let provider = test_provider();
    let config = GenerateConfig {
        max_tokens: 16_384,
        extra_params: Some(serde_json::json!({
            "max_tokens": 65_536u32,
            "max_completion_tokens": 65_536u32,
        })),
        ..GenerateConfig::default()
    };

    let inner = wire_inner(&provider.build_request_body(&[], &[], &config));

    assert_eq!(inner["max_tokens"], 16_384);
    assert_eq!(inner["max_completion_tokens"], 16_384);
}

#[test]
fn extra_params_may_still_lower_the_wire_output_cap() {
    // Asking for less than the reserve is always safe and is preserved.
    let provider = test_provider();
    let config = GenerateConfig {
        max_tokens: 16_384,
        extra_params: Some(serde_json::json!({"max_tokens": 512u32})),
        ..GenerateConfig::default()
    };

    let inner = wire_inner(&provider.build_request_body(&[], &[], &config));

    assert_eq!(inner["max_tokens"], 512);
}
