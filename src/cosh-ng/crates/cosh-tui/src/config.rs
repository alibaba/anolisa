//! Configuration loading from ~/.copilot-shell/settings.json
//!
//! This module reads the same `settings.json` that copilot-shell (TS) uses,
//! with the V2 nested format:
//!
//! ```json
//! {
//!   "security": {
//!     "auth": {
//!       "selectedType": "openai",
//!       "apiKey": "enc:...",
//!       "baseUrl": "https://...",
//!       "openaiModel": "qwen3.6-plus",
//!       "aliyunModel": "..."
//!     }
//!   },
//!   "model": {
//!     "name": "qwen3.6-plus"
//!   }
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use aes_gcm::{
    aead::{Aead, KeyInit},
    AesGcm,
    aes::Aes256,
};
use aes_gcm::aead::generic_array::typenum::{U16};
use scrypt::{scrypt, Params};

/// AES-256-GCM with 16-byte nonce (matching Node.js crypto.createCipheriv behavior).
type Aes256Gcm16 = AesGcm<Aes256, U16, U16>;

const ENCRYPTED_PREFIX: &str = "enc:";
const CREDENTIAL_PASSWORD: &str = "copilot-credential-encrypt";

/// Root config directory: `~/.copilot-shell/`
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot-shell")
}

/// Top-level settings.json structure (V2 format, matching copilot-shell TS).
///
/// All auth/API-key fields live under `security.auth.*`.
/// The model name lives under `model.name`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub model: ModelConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub general: GeneralConfig,
    // Preserve unknown top-level fields so we round-trip cleanly
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `security.*` section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityConfig {
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub folder_trust: FolderTrustConfig,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `security.auth.*` — authentication settings.
///
/// Field names match the copilot-shell TS settingsSchema exactly:
/// - `selectedType`: the active auth provider ("openai", "aliyun", etc.)
/// - `apiKey`:       encrypted or plaintext API key
/// - `baseUrl`:      OpenAI-compatible API base URL
/// - `openaiModel`:  last-used model for openai auth
/// - `aliyunModel`:  last-used model for aliyun auth
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    #[serde(default)]
    pub selected_type: Option<String>,
    #[serde(default)]
    pub enforced_type: Option<String>,
    #[serde(default)]
    pub use_external: Option<bool>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_model: Option<String>,
    #[serde(default)]
    pub aliyun_model: Option<String>,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `security.folderTrust.*`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderTrustConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub feature_enabled: Option<bool>,
}

/// `model.*` section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub max_session_turns: Option<i64>,
    #[serde(default)]
    pub session_token_limit: Option<i64>,
    #[serde(default)]
    pub skip_next_speaker_check: Option<bool>,
    #[serde(default)]
    pub skip_loop_detection: Option<bool>,
    #[serde(default)]
    pub skip_startup_context: Option<bool>,
    #[serde(default)]
    pub generation_config: Option<GenerationConfig>,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `model.generationConfig.*`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub disable_cache_control: Option<bool>,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `ui.*` section (relevant subset for TUI)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub vim_mode: Option<bool>,
    #[serde(default)]
    pub hide_tips: Option<bool>,
    #[serde(default)]
    pub accessibility: AccessibilityConfig,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `ui.accessibility.*`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityConfig {
    #[serde(default)]
    pub disable_loading_phrases: Option<bool>,
    #[serde(default)]
    pub screen_reader: Option<bool>,
}

/// `general.*` section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeneralConfig {
    #[serde(default)]
    pub vim_mode: Option<bool>,
    #[serde(default)]
    pub checkpointing: Option<CheckpointingConfig>,
    // Preserve unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `general.checkpointing.*`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointingConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/// Load settings.json from ~/.copilot-shell/
pub fn load_settings() -> Settings {
    let path = config_dir().join("settings.json");
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match serde_json::from_str::<Settings>(&content) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[cosh-tui] Warning: failed to parse {}: {}. Using defaults.", path.display(), e);
                        Settings::default()
                    }
                }
            }
            Err(e) => {
                eprintln!("[cosh-tui] Warning: failed to read {}: {}. Using defaults.", path.display(), e);
                Settings::default()
            }
        }
    } else {
        Settings::default()
    }
}

/// Save settings.json back to disk with restricted permissions (0600).
///
/// Atomic: writes to `settings.json.tmp` then `rename()` over the target.
/// A crash mid-write therefore leaves the previous good copy intact, instead
/// of leaving a half-written file that fails to parse on next launch (which
/// would silently lose the user's API key configuration).
pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join("settings.json");
    let tmp = dir.join("settings.json.tmp");
    let body = serde_json::to_string_pretty(settings)?;
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Lock down BEFORE rename, so the target file is never world-readable.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the persisted salt from ~/.copilot-shell/.encryption-salt
fn read_salt() -> Option<Vec<u8>> {
    let salt_path = config_dir().join(".encryption-salt");
    let salt = std::fs::read(salt_path).ok()?;
    if salt.len() == 32 {
        Some(salt)
    } else {
        None
    }
}

/// Derive the AES-256 key using scrypt (matching Node.js crypto.scryptSync).
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let params = Params::new(14, 8, 1, 32)
        .map_err(|e| format!("scrypt params error: {}", e))?;
    let mut key = [0u8; 32];
    scrypt(password.as_bytes(), salt, &params, &mut key)
        .map_err(|e| format!("scrypt key derivation failed: {}", e))?;
    Ok(key)
}

/// Decode a hex string to bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    #[allow(clippy::manual_is_multiple_of)]
    if s.len() % 2 != 0 {
        return Err("hex string has odd length".to_string());
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|e| format!("hex decode error: {}", e))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Decrypt a credential encrypted by copilot-shell TS.
///
/// Format: enc:<iv_hex>:<authTag_hex>:<ciphertext_hex>
/// Uses AES-256-GCM with key derived via scrypt.
pub fn decrypt_credential(value: &str) -> Option<String> {
    if !value.starts_with(ENCRYPTED_PREFIX) {
        return Some(value.to_string());
    }
    let without_prefix = &value[ENCRYPTED_PREFIX.len()..];
    let parts: Vec<&str> = without_prefix.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let salt = read_salt()?;
    let key = derive_key(CREDENTIAL_PASSWORD, &salt).ok()?;
    let iv = hex_decode(parts[0]).ok()?;
    let auth_tag = hex_decode(parts[1]).ok()?;
    let ciphertext = hex_decode(parts[2]).ok()?;

    let cipher = Aes256Gcm16::new_from_slice(&key).ok()?;
    let nonce = aes_gcm::aead::Nonce::<Aes256Gcm16>::from_slice(&iv);

    // AES-GCM Rust crate expects ciphertext || auth_tag
    let mut payload = ciphertext;
    payload.extend_from_slice(&auth_tag);

    let plaintext = cipher.decrypt(nonce, payload.as_ref()).ok()?;
    String::from_utf8(plaintext).ok()
}

/// Resolve the effective API key from settings.
///
/// Priority:
/// 1. `security.auth.apiKey` (decrypted if it has "enc:" prefix)
/// 2. Environment variable `DASHSCOPE_API_KEY`
/// 3. Environment variable `OPENAI_API_KEY`
pub fn resolve_api_key(settings: &Settings) -> Option<String> {
    if let Some(ref key) = settings.security.auth.api_key {
        let decrypted = decrypt_credential(key);
        if decrypted.is_some() {
            return decrypted;
        }
        // If decryption failed but value doesn't look encrypted, return as-is
        if !key.starts_with(ENCRYPTED_PREFIX) {
            return Some(key.clone());
        }
        // encrypted but failed to decrypt — fall through to env vars
    }
    std::env::var("DASHSCOPE_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok()
}

/// Resolve the effective base URL from settings.
///
/// Priority:
/// 1. `security.auth.baseUrl`
/// 2. Environment variable `OPENAI_BASE_URL`
/// 3. Default: `https://dashscope.aliyuncs.com/compatible-mode/v1`
pub fn resolve_base_url(settings: &Settings) -> String {
    settings.security.auth.base_url.clone()
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string())
}

/// Resolve the effective model name from settings.
///
/// Priority:
/// 1. `model.name`
/// 2. `security.auth.openaiModel` (if selectedType is "openai")
/// 3. `security.auth.aliyunModel` (if selectedType is "aliyun")
/// 4. Default: `qwen-max`
pub fn resolve_model_name(settings: &Settings) -> String {
    if let Some(ref name) = settings.model.name {
        return name.clone();
    }
    match settings.security.auth.selected_type.as_deref() {
        Some("openai") => settings.security.auth.openai_model.clone(),
        Some("aliyun") => settings.security.auth.aliyun_model.clone(),
        _ => None,
    }.unwrap_or_else(|| "qwen-max".to_string())
}

/// Resolve the auth type (provider) from settings.
///
/// Returns `security.auth.selectedType`, defaulting to "openai".
pub fn resolve_auth_type(settings: &Settings) -> String {
    settings.security.auth.selected_type.clone()
        .unwrap_or_else(|| "openai".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_real_settings_json() {
        let home = dirs::home_dir().expect("no home dir");
        let path = home.join(".copilot-shell/settings.json");
        if !path.exists() {
            eprintln!("Skipping: {} does not exist", path.display());
            return;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let settings: Settings = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

        // Verify the fields that the real copilot-shell writes
        assert_eq!(settings.security.auth.selected_type.as_deref(), Some("openai"));
        assert!(settings.security.auth.api_key.is_some(), "apiKey should be present");
        assert!(settings.security.auth.base_url.is_some(), "baseUrl should be present");
        assert_eq!(settings.model.name.as_deref(), Some("qwen3.6-plus"));
    }

    #[test]
    fn test_resolve_api_key_from_settings() {
        let mut settings = Settings::default();
        settings.security.auth.api_key = Some("sk-test123".to_string());
        assert_eq!(resolve_api_key(&settings), Some("sk-test123".to_string()));
    }

    #[test]
    fn test_resolve_base_url_from_settings() {
        let mut settings = Settings::default();
        settings.security.auth.base_url = Some("https://example.com/v1".to_string());
        assert_eq!(resolve_base_url(&settings), "https://example.com/v1");
    }

    #[test]
    fn test_resolve_base_url_default() {
        let settings = Settings::default();
        assert_eq!(resolve_base_url(&settings), "https://dashscope.aliyuncs.com/compatible-mode/v1");
    }

    #[test]
    fn test_resolve_model_name_from_model_section() {
        let mut settings = Settings::default();
        settings.model.name = Some("qwen3.6-plus".to_string());
        assert_eq!(resolve_model_name(&settings), "qwen3.6-plus");
    }

    #[test]
    fn test_resolve_model_name_from_openai_model() {
        let mut settings = Settings::default();
        settings.security.auth.selected_type = Some("openai".to_string());
        settings.security.auth.openai_model = Some("deepseek-v4".to_string());
        assert_eq!(resolve_model_name(&settings), "deepseek-v4");
    }

    #[test]
    fn test_resolve_model_name_default() {
        let settings = Settings::default();
        assert_eq!(resolve_model_name(&settings), "qwen-max");
    }

    #[test]
    fn test_resolve_auth_type() {
        let mut settings = Settings::default();
        settings.security.auth.selected_type = Some("aliyun".to_string());
        assert_eq!(resolve_auth_type(&settings), "aliyun");

        let default_settings = Settings::default();
        assert_eq!(resolve_auth_type(&default_settings), "openai");
    }

    #[test]
    fn test_parse_v2_settings_structure() {
        let json = r#"{
            "security": {
                "auth": {
                    "selectedType": "openai",
                    "apiKey": "enc:abc123",
                    "baseUrl": "https://api.example.com/v1",
                    "openaiModel": "qwen3.6-plus"
                }
            },
            "model": {
                "name": "qwen3.6-plus"
            }
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.security.auth.selected_type.as_deref(), Some("openai"));
        assert_eq!(settings.security.auth.api_key.as_deref(), Some("enc:abc123"));
        assert_eq!(settings.security.auth.base_url.as_deref(), Some("https://api.example.com/v1"));
        assert_eq!(settings.security.auth.openai_model.as_deref(), Some("qwen3.6-plus"));
        assert_eq!(settings.model.name.as_deref(), Some("qwen3.6-plus"));
    }
}
