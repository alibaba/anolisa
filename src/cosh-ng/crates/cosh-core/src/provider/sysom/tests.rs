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
