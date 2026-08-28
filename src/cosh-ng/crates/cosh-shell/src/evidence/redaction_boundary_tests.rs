use crate::evidence::redact_sensitive_text;

#[test]
fn redacts_sensitive_patterns_adjacent_to_cjk_text() {
    for secret in [
        "ghp_abcdefghijklmnopqrstuvwxyz123456",
        "sk-abcdefghijklmnopqrstuvwxyz",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature",
        "AKIA1234567890ABCDEF",
        "LTAI5tExampleAccessKey",
    ] {
        let input = format!("前缀{secret}后缀");
        let (redacted, changed) = redact_sensitive_text(&input);

        assert!(changed, "{input}");
        assert!(!redacted.contains(secret), "{redacted}");
    }

    for (input, secret) in [
        (
            "请求头authorization: abc123def456ghi789",
            "abc123def456ghi789",
        ),
        ("令牌bearer abc123def456ghi789", "abc123def456ghi789"),
        ("看一下https://user:p4ssw0rd@example.com/x", "p4ssw0rd"),
    ] {
        let (redacted, changed) = redact_sensitive_text(input);

        assert!(changed, "{input}");
        assert!(!redacted.contains(secret), "{redacted}");
    }
}
