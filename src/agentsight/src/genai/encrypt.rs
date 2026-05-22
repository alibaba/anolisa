//! 消息混合加密模块
//!
//! 使用 RSA-OAEP(SHA-256) + AES-256-GCM 混合加密方案保护敏感消息字段。
//! 每次加密生成随机 AES-256 密钥和 nonce，用公钥加密 AES 密钥，
//! 最终输出 base64 编码的二进制密文。
//!
//! 公钥管理策略：代码内嵌默认公钥，环境变量 `MESSAGE_ENCRYPT_PUBLIC_KEY` 可覆盖。

use openssl::rsa::{Rsa, Padding};
use openssl::pkey::Public;
use openssl::symm::{Cipher, encrypt_aead};
use openssl::rand::rand_bytes;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// 环境变量名（设置后覆盖默认公钥）
pub const ENCRYPT_PUBLIC_KEY_ENV_VAR: &str = "MESSAGE_ENCRYPT_PUBLIC_KEY";

/// 编译时内嵌的默认 RSA 公钥（开箱即用，无需配置环境变量）
/// 生产环境可通过环境变量 MESSAGE_ENCRYPT_PUBLIC_KEY 覆盖此默认值
const DEFAULT_PUBLIC_KEY_PEM: &str = r#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAzK4VhG29nW7eydBm3fzh
HDVJQ5RQpqOkIhairUWIjH/QS5s9OnPmRTM7vipTvku4yRD6AfJycPIjR0jZXVpd
EVTsz/K4E4qTm6o1w7ciuTvc56Gt9AHR86OURj9VRcZz058NVZRpYEtQqH9sVjJP
JwjS5YhpKJef6leQztexxKpMHCMVm2cedCJFUCJDd0bF9NUN04sdr49H/D6U/B09
oz/VhPlHSn6dMp9yMJtN0YE+X51KQxVqIyuVZ/xgr34AWeweiyLNJTyLFnY5zFIL
pVe9hOgtU1LkSTW9C41bPOiODD89068dUpYGDrXIzumC8ik54ITNhDVScLS9Beua
hwIDAQAB
-----END PUBLIC KEY-----"#;

/// AES-256 密钥长度（32 字节）
const AES_KEY_LEN: usize = 32;

/// AES-GCM nonce 长度（12 字节）
const NONCE_LEN: usize = 12;

/// AES-GCM 认证标签长度（16 字节）
const TAG_LEN: usize = 16;

/// 消息加密器，持有解析后的 RSA 公钥
pub struct MessageEncryptor {
    rsa: Rsa<Public>,
}

impl MessageEncryptor {
    /// 创建加密器
    ///
    /// 优先读取环境变量 `MESSAGE_ENCRYPT_PUBLIC_KEY` 中的 PEM 公钥；
    /// 若未设置，使用代码内嵌的默认公钥。
    /// 解析失败时记录警告并返回 None（回退到明文模式）。
    pub fn new() -> Option<Self> {
        let pem_str = std::env::var(ENCRYPT_PUBLIC_KEY_ENV_VAR)
            .unwrap_or_else(|_| DEFAULT_PUBLIC_KEY_PEM.to_string());

        match Rsa::public_key_from_pem(pem_str.as_bytes()) {
            Ok(rsa) => {
                log::info!("MessageEncryptor initialized (RSA-{} + AES-256-GCM)", rsa.size() * 8);
                Some(MessageEncryptor { rsa })
            }
            Err(e) => {
                log::warn!("Failed to parse RSA public key, encryption disabled: {}", e);
                None
            }
        }
    }

    /// 执行混合加密
    ///
    /// 输出格式（base64 编码）：
    /// `[2字节 encrypted_key 长度(big-endian)] [encrypted_key] [12字节 nonce] [ciphertext + 16字节 tag]`
    pub fn encrypt(&self, plaintext: &str) -> Result<String, String> {
        // 1. 生成随机 AES-256 密钥
        let mut aes_key = vec![0u8; AES_KEY_LEN];
        rand_bytes(&mut aes_key).map_err(|e| format!("rand_bytes for AES key failed: {}", e))?;

        // 2. 生成随机 12 字节 nonce
        let mut nonce = vec![0u8; NONCE_LEN];
        rand_bytes(&mut nonce).map_err(|e| format!("rand_bytes for nonce failed: {}", e))?;

        // 3. AES-256-GCM 加密明文
        let mut tag = vec![0u8; TAG_LEN];
        let ciphertext = encrypt_aead(
            Cipher::aes_256_gcm(),
            &aes_key,
            Some(&nonce),
            &[],  // AAD (Additional Authenticated Data) - 不使用
            plaintext.as_bytes(),
            &mut tag,
        ).map_err(|e| format!("AES-256-GCM encryption failed: {}", e))?;

        // 4. RSA-OAEP(SHA-256) 加密 AES 密钥
        let mut encrypted_key = vec![0u8; self.rsa.size() as usize];
        let encrypted_key_len = self.rsa.public_encrypt(
            &aes_key,
            &mut encrypted_key,
            Padding::PKCS1_OAEP,
        ).map_err(|e| format!("RSA-OAEP encryption failed: {}", e))?;
        encrypted_key.truncate(encrypted_key_len);

        // 5. 组装二进制输出：[2字节长度] [encrypted_key] [nonce] [ciphertext] [tag]
        let key_len_bytes = (encrypted_key_len as u16).to_be_bytes();
        let mut output = Vec::with_capacity(
            2 + encrypted_key_len + NONCE_LEN + ciphertext.len() + TAG_LEN
        );
        output.extend_from_slice(&key_len_bytes);
        output.extend_from_slice(&encrypted_key);
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        output.extend_from_slice(&tag);

        // 6. Base64 编码
        Ok(BASE64.encode(&output))
    }

    /// 辅助方法：有加密器则加密，加密失败或无加密器时返回原文
    pub fn maybe_encrypt(encryptor: Option<&Self>, text: &str) -> String {
        match encryptor {
            Some(enc) => match enc.encrypt(text) {
                Ok(encrypted) => encrypted,
                Err(e) => {
                    log::warn!("Encryption failed, falling back to plaintext: {}", e);
                    text.to_string()
                }
            },
            None => text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::rsa::Rsa;
    use openssl::symm::{Cipher, decrypt_aead};

    #[test]
    fn test_new_with_default_key() {
        // 不设置环境变量，应使用默认公钥成功创建
        unsafe { std::env::remove_var(ENCRYPT_PUBLIC_KEY_ENV_VAR); }
        let enc = MessageEncryptor::new();
        assert!(enc.is_some(), "Should create encryptor with default key");
    }

    #[test]
    fn test_encrypt_produces_different_output() {
        unsafe { std::env::remove_var(ENCRYPT_PUBLIC_KEY_ENV_VAR); }
        let enc = MessageEncryptor::new().unwrap();
        let plaintext = "hello world, this is a secret message";
        let encrypted = enc.encrypt(plaintext).unwrap();

        // 加密结果应该是有效的 base64 且与原文不同
        assert_ne!(encrypted, plaintext);
        assert!(!encrypted.is_empty());
        // base64 解码应成功
        let decoded = BASE64.decode(&encrypted).unwrap();
        assert!(decoded.len() > 2 + NONCE_LEN + TAG_LEN);
    }

    // 该测试依赖本地 tests/test_private_key.pem（与默认公钥配对的私钥）。
    // 出于安全考虑私钥文件不提交到仓库，仅在本地手动生成密钥对后运行：
    //   cargo test --lib genai::encrypt::tests::test_encrypt_decrypt_roundtrip -- --ignored
    #[test]
    #[ignore]
    fn test_encrypt_decrypt_roundtrip() {
        unsafe { std::env::remove_var(ENCRYPT_PUBLIC_KEY_ENV_VAR); }
        let enc = MessageEncryptor::new().unwrap();
        let plaintext = "测试消息：gen_ai.input.messages 内容加密验证";

        let encrypted = enc.encrypt(plaintext).unwrap();

        // 用测试私钥解密
        let private_key_pem = std::fs::read(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_private_key.pem")
        ).expect("test_private_key.pem should exist in tests/");
        let private_rsa = Rsa::private_key_from_pem(&private_key_pem).unwrap();

        // 解析密文结构
        let raw = BASE64.decode(&encrypted).unwrap();
        let key_len = u16::from_be_bytes([raw[0], raw[1]]) as usize;
        let encrypted_key = &raw[2..2 + key_len];
        let nonce = &raw[2 + key_len..2 + key_len + NONCE_LEN];
        let ciphertext_and_tag = &raw[2 + key_len + NONCE_LEN..];
        let (ciphertext, tag) = ciphertext_and_tag.split_at(ciphertext_and_tag.len() - TAG_LEN);

        // RSA 解密 AES 密钥
        let mut aes_key = vec![0u8; private_rsa.size() as usize];
        let aes_key_len = private_rsa.private_decrypt(
            encrypted_key, &mut aes_key, Padding::PKCS1_OAEP
        ).unwrap();
        let aes_key = &aes_key[..aes_key_len];

        // AES-256-GCM 解密
        let decrypted = decrypt_aead(
            Cipher::aes_256_gcm(),
            aes_key,
            Some(nonce),
            &[],
            ciphertext,
            tag,
        ).unwrap();

        assert_eq!(String::from_utf8(decrypted).unwrap(), plaintext);
    }

    #[test]
    fn test_maybe_encrypt_without_encryptor() {
        let text = "plain text content";
        let result = MessageEncryptor::maybe_encrypt(None, text);
        assert_eq!(result, text);
    }

    #[test]
    fn test_maybe_encrypt_with_encryptor() {
        unsafe { std::env::remove_var(ENCRYPT_PUBLIC_KEY_ENV_VAR); }
        let enc = MessageEncryptor::new().unwrap();
        let text = "secret content";
        let result = MessageEncryptor::maybe_encrypt(Some(&enc), text);
        assert_ne!(result, text);
    }
}
