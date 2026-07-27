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
