use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

use asc_daemon_protocol::BearerAuth;
use base64::Engine as _;
use subtle::ConstantTimeEq as _;

/// Loaded bearer token verifier. The secret is never printable.
pub struct TokenVerifier {
    secret: Vec<u8>,
}

impl TokenVerifier {
    /// Loads a token from a regular current-user-owned `0600` file.
    ///
    /// # Errors
    /// Fails closed on missing, unsafe, or invalid credentials.
    pub fn load(path: &Path) -> Result<Self, AuthFileError> {
        let metadata = fs::symlink_metadata(path).map_err(|_| AuthFileError)?;
        let current_uid = fs::metadata("/proc/self").map_err(|_| AuthFileError)?.uid();
        if !metadata.file_type().is_file()
            || metadata.mode() & 0o777 != 0o600
            || metadata.uid() != current_uid
        {
            return Err(AuthFileError);
        }
        let secret = fs::read_to_string(path).map_err(|_| AuthFileError)?;
        let secret = secret.trim().as_bytes().to_vec();
        if !(32..=256).contains(&secret.len()) {
            return Err(AuthFileError);
        }
        Ok(Self { secret })
    }

    pub(crate) fn verify(&self, auth: Option<&BearerAuth>) -> bool {
        let Some(auth) = auth else {
            return false;
        };
        if auth.scheme != "bearer" || auth.token.len() != self.secret.len() {
            return false;
        }
        bool::from(auth.token.as_bytes().ct_eq(&self.secret))
    }
}

/// Unsafe or unreadable management credential.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("policy admin token file is missing or unsafe")]
pub struct AuthFileError;

/// Generates one management token for installer/init use.
///
/// The destination is created with `O_EXCL`; existing credentials are never overwritten.
///
/// # Errors
/// Returns a safe error if directory creation, entropy, or atomic write fails.
pub fn prepare_auth(path: &Path) -> Result<(), PrepareAuthError> {
    if let Some(parent) = path.parent() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(parent).map_err(|_| PrepareAuthError)?;
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| PrepareAuthError)?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| PrepareAuthError)?;
    file.write_all(token.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| PrepareAuthError)
}

/// Credential preparation failure.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("policy admin token could not be generated atomically")]
pub struct PrepareAuthError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_debug_is_redacted() {
        let auth: BearerAuth = serde_json::from_value(serde_json::json!({
            "scheme": "bearer",
            "token": "not-printed"
        }))
        .unwrap();
        let output = format!("{auth:?}");
        assert!(!output.contains("not-printed"));
        assert!(output.contains("[REDACTED]"));
    }
}
