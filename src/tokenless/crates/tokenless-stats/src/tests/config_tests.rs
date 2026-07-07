#[test]
fn test_default_config() {
    let config = TokenlessConfig::default();
    assert!(config.is_stats_enabled());
}

#[test]
fn test_load_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let config = TokenlessConfig::load_with_env_and_path(None, Some(&path));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_load_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let _ = std::fs::write(&path, "not json");
    let config = TokenlessConfig::load_with_env_and_path(None, Some(&path));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_env_override_enabled() {
    let config = TokenlessConfig::load_with_env(Some("1"));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_env_override_disabled() {
    let config = TokenlessConfig::load_with_env(Some("0"));
    assert!(!config.is_stats_enabled());
}

#[test]
fn test_env_override_true_string() {
    let config = TokenlessConfig::load_with_env(Some("true"));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_env_override_overrides_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    // Write file config with stats_enabled=false
    let _ = std::fs::write(&path, "{\"stats_enabled\":false}");
    // Env override to enable
    let config = TokenlessConfig::load_with_env_and_path(Some("1"), Some(&path));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_sls_enabled_default_true() {
    let config = TokenlessConfig::default();
    assert!(config.is_sls_enabled());
    assert!(config.is_stats_enabled());
}

#[test]
fn test_load_sls_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let config = TokenlessConfig::load_with_envs_and_path(None, None, None, Some(&path));
    assert!(config.is_sls_enabled());
    assert!(config.is_stats_enabled());
}

#[test]
fn test_sls_env_override_enabled() {
    let config = TokenlessConfig::load_with_envs(Some("1"), None);
    assert!(config.is_stats_enabled());
    assert!(config.is_sls_enabled());

    let config = TokenlessConfig::load_with_envs(None, Some("1"));
    assert!(config.is_stats_enabled());
    assert!(config.is_sls_enabled());
}

#[test]
fn test_sls_env_override_disabled() {
    // stats_env="0" disables stats, sls stays default true
    let config = TokenlessConfig::load_with_envs(Some("0"), None);
    assert!(!config.is_stats_enabled());
    assert!(config.is_sls_enabled());

    // sls_env="0" explicitly disables sls
    let config = TokenlessConfig::load_with_envs(None, Some("0"));
    assert!(config.is_stats_enabled());
    assert!(!config.is_sls_enabled());
}

#[test]
fn test_sls_env_override_overrides_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    // Write file config with sls_enabled=true
    let _ = std::fs::write(&path, "{\"stats_enabled\":true,\"sls_enabled\":true}");
    // Env override to disable sls
    let config = TokenlessConfig::load_with_envs_and_path(None, Some("0"), None, Some(&path));
    assert!(config.is_stats_enabled());
    assert!(!config.is_sls_enabled());
}

#[test]
fn test_both_env_overrides() {
    let config = TokenlessConfig::load_with_envs(Some("0"), Some("1"));
    assert!(!config.is_stats_enabled());
    assert!(config.is_sls_enabled());

    let config = TokenlessConfig::load_with_envs(Some("1"), Some("0"));
    assert!(config.is_stats_enabled());
    assert!(!config.is_sls_enabled());
}

#[test]
fn test_empty_env_treated_as_unset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    // File config has stats_enabled=true
    let _ = std::fs::write(&path, "{\"stats_enabled\":true}");
    // Empty string should fall through to file config (true), not override to false
    let config = TokenlessConfig::load_with_envs_and_path(Some(""), None, None, Some(&path));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_compression_default_true() {
    let config = TokenlessConfig::default();
    assert!(config.is_compression_enabled());
}

#[test]
fn test_compression_env_override() {
    let config = TokenlessConfig::load_with_envs_and_path(None, None, Some("0"), None);
    assert!(!config.is_compression_enabled());

    let config = TokenlessConfig::load_with_envs_and_path(None, None, Some("1"), None);
    assert!(config.is_compression_enabled());
}

#[test]
fn test_compression_env_overrides_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let _ = std::fs::write(&path, "{\"compression_enabled\":false}");
    let config = TokenlessConfig::load_with_envs_and_path(None, None, Some("1"), Some(&path));
    assert!(config.is_compression_enabled());
}

#[test]
fn test_compression_file_config_honored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let _ = std::fs::write(&path, "{\"compression_enabled\":false}");
    let config = TokenlessConfig::load_with_envs_and_path(None, None, None, Some(&path));
    assert!(!config.is_compression_enabled());
}

#[test]
fn test_compression_empty_env_treated_as_unset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let _ = std::fs::write(&path, "{\"compression_enabled\":false}");
    let config = TokenlessConfig::load_with_envs_and_path(None, None, Some(""), Some(&path));
    assert!(!config.is_compression_enabled());
}

#[test]
fn test_parse_env_bool_yes_variant() {
    let config = TokenlessConfig::load_with_env(Some("yes"));
    assert!(config.is_stats_enabled());
    let config = TokenlessConfig::load_with_env(Some("YES"));
    assert!(config.is_stats_enabled());
}

#[test]
fn test_parse_env_bool_false_string() {
    let config = TokenlessConfig::load_with_env(Some("false"));
    assert!(!config.is_stats_enabled());
}

#[test]
fn test_save_and_reload_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".tokenless/config.json");
    let config = TokenlessConfig {
        stats_enabled: false,
        sls_enabled: true,
        compression_enabled: false,
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let json = serde_json::to_string_pretty(&config).unwrap();
    std::fs::write(&path, json).unwrap();

    let reloaded = TokenlessConfig::load_with_envs_and_path(None, None, None, Some(&path));
    assert!(!reloaded.stats_enabled);
    assert!(reloaded.sls_enabled);
    assert!(!reloaded.compression_enabled);
}

#[test]
fn test_default_all_enabled() {
    let config = TokenlessConfig::default();
    assert!(config.is_stats_enabled());
    assert!(config.is_sls_enabled());
    assert!(config.is_compression_enabled());
}

#[test]
fn test_serde_round_trip() {
    let config = TokenlessConfig {
        stats_enabled: false,
        sls_enabled: false,
        compression_enabled: true,
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: TokenlessConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.stats_enabled, false);
    assert_eq!(deserialized.sls_enabled, false);
    assert_eq!(deserialized.compression_enabled, true);
}
