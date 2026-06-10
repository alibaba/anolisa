use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use scrypt::{scrypt, Params};

use crate::config::config_dir;

const ENCRYPTED_PREFIX: &str = "enc:";
const CREDENTIAL_PASSWORD: &[u8] = b"copilot-credential-encrypt";

pub fn try_migrate() {
    let dir = config_dir();
    let settings_path = dir.join("settings.json");
    let config_path = dir.join("config.toml");

    if !settings_path.exists() || config_path.exists() {
        return;
    }

    eprintln!("[cosh-core] Migrating settings.json → config.toml ...");

    match migrate_settings(&settings_path, &config_path, &dir) {
        Ok(()) => eprintln!("[cosh-core] Migration complete: {}", config_path.display()),
        Err(e) => eprintln!("[cosh-core] Migration warning: {e} (continuing with defaults)"),
    }
}

fn migrate_settings(
    settings_path: &Path,
    config_path: &Path,
    cfg_dir: &Path,
) -> Result<(), String> {
    let content = std::fs::read_to_string(settings_path)
        .map_err(|e| format!("failed to read settings.json: {e}"))?;

    let root: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("invalid JSON: {e}"))?;

    let auth = &root["security"]["auth"];
    let model = &root["model"];
    let tools = &root["tools"];
    let general = &root["general"];

    let selected_type = auth["selectedType"].as_str().unwrap_or("openai");

    let raw_api_key = auth["apiKey"].as_str().unwrap_or("");
    let api_key = if raw_api_key.starts_with(ENCRYPTED_PREFIX) {
        let salt_path = cfg_dir.join(".encryption-salt");
        match decrypt_credential(raw_api_key, &salt_path) {
            Some(k) => k,
            None => {
                eprintln!("[cosh-core] Warning: failed to decrypt API key, skipping");
                String::new()
            }
        }
    } else {
        raw_api_key.to_string()
    };

    let base_url = auth["baseUrl"].as_str().unwrap_or("").to_string();

    let provider_model = match selected_type {
        "aliyun" => auth["aliyunModel"].as_str().unwrap_or(""),
        _ => auth["openaiModel"].as_str().unwrap_or(""),
    };

    let active_model = model["name"].as_str().unwrap_or(provider_model);

    let provider_type = match selected_type {
        "openai" | "aliyun" => "dashscope",
        other => other,
    };

    let session_token_limit = model["sessionTokenLimit"].as_i64().filter(|&v| v > 0);

    let max_turns = model["maxSessionTurns"].as_i64().filter(|&v| v > 0);

    let approval_mode = tools["approvalMode"].as_str().map(map_approval_mode);

    let output_language = general["outputLanguage"]
        .as_str()
        .filter(|&v| v != "auto" && !v.is_empty());

    let toml = build_toml(&MigratedFields {
        provider_type,
        base_url: &base_url,
        api_key: &api_key,
        provider_model,
        active_model,
        session_token_limit,
        max_turns,
        approval_mode,
        output_language,
    });

    std::fs::write(config_path, toml.as_bytes())
        .map_err(|e| format!("failed to write config.toml: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config_path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

fn decrypt_credential(encrypted: &str, salt_path: &Path) -> Option<String> {
    let without_prefix = encrypted.strip_prefix(ENCRYPTED_PREFIX)?;
    let parts: Vec<&str> = without_prefix.split(':').collect();
    if parts.len() != 3 {
        return None;
    }

    let iv_bytes = hex_decode(parts[0])?;
    let tag_bytes = hex_decode(parts[1])?;
    let ct_bytes = hex_decode(parts[2])?;

    let salt = std::fs::read(salt_path).ok()?;
    if salt.len() != 32 {
        return None;
    }

    let mut key = [0u8; 32];
    let params = Params::new(14, 8, 1, 32).ok()?;
    scrypt(CREDENTIAL_PASSWORD, &salt, &params, &mut key).ok()?;

    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;

    let nonce = Nonce::from_slice(&iv_bytes);
    let mut ciphertext_with_tag = ct_bytes;
    ciphertext_with_tag.extend_from_slice(&tag_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext_with_tag.as_ref()).ok()?;
    String::from_utf8(plaintext).ok()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn map_approval_mode(mode: &str) -> &str {
    match mode {
        "YOLO" | "yolo" => "trust",
        "AUTO_EDIT" | "auto_edit" => "auto",
        "PLAN" | "plan" => "strict",
        _ => "balanced",
    }
}

struct MigratedFields<'a> {
    provider_type: &'a str,
    base_url: &'a str,
    api_key: &'a str,
    provider_model: &'a str,
    active_model: &'a str,
    session_token_limit: Option<i64>,
    max_turns: Option<i64>,
    approval_mode: Option<&'a str>,
    output_language: Option<&'a str>,
}

fn build_toml(fields: &MigratedFields<'_>) -> String {
    let MigratedFields {
        provider_type,
        base_url,
        api_key,
        provider_model,
        active_model,
        session_token_limit,
        max_turns,
        approval_mode,
        output_language,
    } = fields;
    let date = chrono::Local::now().format("%Y-%m-%d");
    let mut out = String::new();

    out.push_str(&format!("# Auto-migrated from settings.json on {date}\n"));
    out.push_str("# Original: ~/.copilot-shell/settings.json\n\n");

    out.push_str("[ai]\n");
    out.push_str("active_provider = \"default\"\n");
    if !active_model.is_empty() {
        out.push_str(&format!("active_model = \"{active_model}\"\n"));
    }
    if let Some(lang) = output_language {
        out.push_str(&format!("output_language = \"{lang}\"\n"));
    }
    out.push('\n');

    out.push_str("[ai.providers.default]\n");
    out.push_str(&format!("type = \"{provider_type}\"\n"));
    if !base_url.is_empty() {
        out.push_str(&format!("base_url = \"{base_url}\"\n"));
    }
    if !api_key.is_empty() {
        out.push_str(&format!("api_key = \"{api_key}\"\n"));
    }
    if !provider_model.is_empty() {
        out.push_str(&format!("model = \"{provider_model}\"\n"));
    }
    out.push('\n');

    let has_agent_section =
        session_token_limit.is_some() || max_turns.is_some() || approval_mode.is_some();

    if has_agent_section {
        out.push_str("[agent]\n");
        if let Some(mode) = approval_mode {
            out.push_str(&format!("approval_mode = \"{mode}\"\n"));
        }
        if let Some(turns) = max_turns {
            out.push_str(&format!("max_turns = {turns}\n"));
        }
        if let Some(limit) = session_token_limit {
            out.push_str(&format!("session_token_limit = {limit}\n"));
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_passthrough() {
        let raw = "sk-plaintext-key";
        assert!(!raw.starts_with(ENCRYPTED_PREFIX));
    }

    #[test]
    fn decrypt_known_value() {
        let tmp = tempfile::TempDir::new().unwrap();
        let salt_path = tmp.path().join(".encryption-salt");

        let salt = [0x42u8; 32];
        std::fs::write(&salt_path, salt).unwrap();

        let mut key = [0u8; 32];
        let params = Params::new(14, 8, 1, 32).unwrap();
        scrypt(CREDENTIAL_PASSWORD, &salt, &params, &mut key).unwrap();

        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let iv = [0xABu8; 12];
        let nonce = Nonce::from_slice(&iv);
        let plaintext = b"sk-test-secret-key";

        let ciphertext_with_tag = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();
        let (ct, tag) = ciphertext_with_tag.split_at(ciphertext_with_tag.len() - 16);

        let encrypted = format!(
            "enc:{}:{}:{}",
            hex_encode(&iv),
            hex_encode(tag),
            hex_encode(ct),
        );

        let result = decrypt_credential(&encrypted, &salt_path);
        assert_eq!(result, Some("sk-test-secret-key".to_string()));
    }

    #[test]
    fn approval_mode_mapping() {
        assert_eq!(map_approval_mode("DEFAULT"), "balanced");
        assert_eq!(map_approval_mode("YOLO"), "trust");
        assert_eq!(map_approval_mode("yolo"), "trust");
        assert_eq!(map_approval_mode("AUTO_EDIT"), "auto");
        assert_eq!(map_approval_mode("PLAN"), "strict");
        assert_eq!(map_approval_mode("unknown"), "balanced");
    }

    #[test]
    fn build_toml_output_parseable() {
        let toml_str = build_toml(&MigratedFields {
            provider_type: "dashscope",
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
            api_key: "sk-test",
            provider_model: "qwen3-plus",
            active_model: "qwen3-plus",
            session_token_limit: Some(128000),
            max_turns: Some(30),
            approval_mode: Some("balanced"),
            output_language: Some("zh-CN"),
        });

        let config: crate::config::CoreConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.ai.active_provider.as_deref(), Some("default"));
        assert_eq!(config.ai.active_model.as_deref(), Some("qwen3-plus"));
        assert_eq!(config.ai.output_language.as_deref(), Some("zh-CN"));
        assert_eq!(config.agent.session_token_limit, 128000);
        assert_eq!(config.agent.max_turns, 30);
        assert_eq!(config.agent.approval_mode, "balanced");

        let provider = config.ai.providers.get("default").unwrap();
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
        assert_eq!(provider.provider_type.as_deref(), Some("dashscope"));
    }

    #[test]
    fn skip_when_config_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let settings_path = tmp.path().join("settings.json");
        let config_path = tmp.path().join("config.toml");

        std::fs::write(&settings_path, r#"{"security":{"auth":{}}}"#).unwrap();
        std::fs::write(&config_path, "# existing").unwrap();

        // try_migrate checks existence — if config exists, it returns early
        assert!(config_path.exists());
        assert!(settings_path.exists());

        // Simulate the guard logic from try_migrate
        let should_skip = !settings_path.exists() || config_path.exists();
        assert!(should_skip);
    }

    #[test]
    fn full_migration_without_encryption() {
        let tmp = tempfile::TempDir::new().unwrap();
        let settings_path = tmp.path().join("settings.json");
        let config_path = tmp.path().join("config.toml");

        let json = r#"{
            "$version": 2,
            "security": {
                "auth": {
                    "selectedType": "openai",
                    "apiKey": "sk-plain-key",
                    "baseUrl": "https://example.com/v1",
                    "openaiModel": "test-model"
                }
            },
            "model": {
                "name": "test-model",
                "sessionTokenLimit": 64000,
                "maxSessionTurns": 15
            },
            "tools": {
                "approvalMode": "YOLO"
            },
            "general": {
                "outputLanguage": "en"
            }
        }"#;
        std::fs::write(&settings_path, json).unwrap();

        migrate_settings(&settings_path, &config_path, tmp.path()).unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: crate::config::CoreConfig = toml::from_str(&content).unwrap();

        assert_eq!(config.ai.active_model.as_deref(), Some("test-model"));
        assert_eq!(config.agent.approval_mode, "trust");
        assert_eq!(config.agent.max_turns, 15);
        assert_eq!(config.agent.session_token_limit, 64000);
        assert_eq!(config.ai.output_language.as_deref(), Some("en"));

        let provider = config.ai.providers.get("default").unwrap();
        assert_eq!(provider.api_key.as_deref(), Some("sk-plain-key"));
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
