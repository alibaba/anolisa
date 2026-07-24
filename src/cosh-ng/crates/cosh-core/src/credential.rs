//! Compatibility codec for credentials stored in copilot-shell configuration files.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use aes_gcm::aead::generic_array::typenum::U16;
use aes_gcm::{
    aead::{Aead, KeyInit},
    aes::Aes256,
    AesGcm,
};
use getrandom::getrandom;
use scrypt::{scrypt, Params};

/// AES-256-GCM with a 16-byte nonce, matching the legacy Node.js implementation.
type Aes256Gcm16 = AesGcm<Aes256, U16>;

pub(crate) const ENCRYPTED_PREFIX: &str = "enc:";
const CREDENTIAL_PASSWORD: &[u8] = b"copilot-credential-encrypt";
const SALT_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 16;
const TAG_LENGTH: usize = 16;

pub(crate) struct CredentialCodec {
    cipher: Aes256Gcm16,
}

impl CredentialCodec {
    pub(crate) fn for_encryption(salt_path: &Path) -> Result<Self, String> {
        let salt = load_or_create_salt(salt_path)?;
        Self::from_salt(&salt)
    }

    pub(crate) fn for_decryption(salt_path: &Path) -> Result<Self, String> {
        let salt = read_salt(salt_path)?;
        Self::from_salt(&salt)
    }

    pub(crate) fn encrypt(&self, value: &str) -> Result<String, String> {
        let mut iv = [0_u8; NONCE_LENGTH];
        getrandom(&mut iv)
            .map_err(|error| format!("failed to generate credential nonce: {error}"))?;
        let nonce = aes_gcm::aead::generic_array::GenericArray::from_slice(&iv);
        let ciphertext_with_tag = self
            .cipher
            .encrypt(nonce, value.as_bytes())
            .map_err(|_| "failed to encrypt credential".to_string())?;
        let (ciphertext, tag) = ciphertext_with_tag.split_at(
            ciphertext_with_tag
                .len()
                .checked_sub(TAG_LENGTH)
                .ok_or_else(|| "failed to encrypt credential".to_string())?,
        );

        Ok(format!(
            "{ENCRYPTED_PREFIX}{}:{}:{}",
            hex::encode(iv),
            hex::encode(tag),
            hex::encode(ciphertext),
        ))
    }

    pub(crate) fn decrypt(&self, value: &str) -> Result<String, String> {
        let without_prefix = value
            .strip_prefix(ENCRYPTED_PREFIX)
            .ok_or_else(|| "credential is not encrypted".to_string())?;
        let mut parts = without_prefix.split(':');
        let iv = decode_part(parts.next(), "nonce")?;
        let tag = decode_part(parts.next(), "authentication tag")?;
        let ciphertext = decode_part(parts.next(), "ciphertext")?;
        if parts.next().is_some() || iv.len() != NONCE_LENGTH || tag.len() != TAG_LENGTH {
            return Err("credential has an invalid encrypted format".to_string());
        }

        let nonce = aes_gcm::aead::generic_array::GenericArray::from_slice(&iv);
        let mut ciphertext_with_tag = ciphertext;
        ciphertext_with_tag.extend_from_slice(&tag);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext_with_tag.as_ref())
            .map_err(|_| "credential decryption failed".to_string())?;

        String::from_utf8(plaintext)
            .map_err(|_| "credential plaintext is not valid UTF-8".to_string())
    }

    fn from_salt(salt: &[u8]) -> Result<Self, String> {
        Ok(Self {
            cipher: cipher_for_salt(salt)?,
        })
    }
}

/// Replaces an existing malformed salt after an explicit credential reset.
///
/// A valid salt is never replaced here: it may still unlock credentials that
/// are merely unavailable because their ciphertext was damaged.
pub(crate) fn rotate_invalid_salt(salt_path: &Path) -> Result<(), String> {
    match fs::read(salt_path) {
        Ok(salt) if salt.len() == SALT_LENGTH => Ok(()),
        Ok(_) => {
            let mut salt = [0_u8; SALT_LENGTH];
            getrandom(&mut salt)
                .map_err(|error| format!("failed to generate credential salt: {error}"))?;
            write_private_atomic(salt_path, &salt)
                .map_err(|error| format!("failed to replace invalid credential salt: {error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to read credential salt: {error}")),
    }
}

/// True when a salt file exists but is not a valid 32-byte salt.
///
/// Such a salt blocks encryption on save, yet an explicit reset can recover it
/// by rotating it (see [`rotate_invalid_salt`]). A missing salt is not
/// malformed: it is created on demand.
pub(crate) fn salt_needs_repair(salt_path: &Path) -> bool {
    match fs::metadata(salt_path) {
        Ok(metadata) => metadata.len() != SALT_LENGTH as u64,
        Err(_) => false,
    }
}

pub(crate) fn encrypt_credential(value: &str, salt_path: &Path) -> Result<String, String> {
    CredentialCodec::for_encryption(salt_path)?.encrypt(value)
}

pub(crate) fn decrypt_credential(value: &str, salt_path: &Path) -> Result<String, String> {
    CredentialCodec::for_decryption(salt_path)?.decrypt(value)
}

fn decode_part(part: Option<&str>, name: &str) -> Result<Vec<u8>, String> {
    let part = part.ok_or_else(|| "credential has an invalid encrypted format".to_string())?;
    hex::decode(part).map_err(|_| format!("credential {name} is not valid hexadecimal"))
}

fn cipher_for_salt(salt: &[u8]) -> Result<Aes256Gcm16, String> {
    let mut key = [0_u8; 32];
    let params = Params::new(14, 8, 1, 32)
        .map_err(|error| format!("failed to configure credential key derivation: {error}"))?;
    scrypt(CREDENTIAL_PASSWORD, salt, &params, &mut key)
        .map_err(|error| format!("failed to derive credential key: {error}"))?;
    Aes256Gcm16::new_from_slice(&key)
        .map_err(|_| "failed to initialize credential cipher".to_string())
}

fn read_salt(salt_path: &Path) -> Result<Vec<u8>, String> {
    let salt =
        fs::read(salt_path).map_err(|error| format!("failed to read credential salt: {error}"))?;
    validate_salt(salt)
}

fn validate_salt(salt: Vec<u8>) -> Result<Vec<u8>, String> {
    if salt.len() != SALT_LENGTH {
        return Err("credential salt must contain exactly 32 bytes".to_string());
    }
    Ok(salt)
}

fn load_or_create_salt(salt_path: &Path) -> Result<Vec<u8>, String> {
    match fs::read(salt_path) {
        Ok(salt) => validate_salt(salt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_salt(salt_path),
        Err(error) => Err(format!("failed to read credential salt: {error}")),
    }
}

fn create_salt(salt_path: &Path) -> Result<Vec<u8>, String> {
    let mut salt = [0_u8; SALT_LENGTH];
    getrandom(&mut salt).map_err(|error| format!("failed to generate credential salt: {error}"))?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(salt_path) {
        Ok(mut file) => {
            file.write_all(&salt)
                .map_err(|error| format!("failed to write credential salt: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("failed to sync credential salt: {error}"))?;
            Ok(salt.to_vec())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_salt(salt_path),
        Err(error) => Err(format!("failed to create credential salt: {error}")),
    }
}

pub(crate) fn write_private_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    write_private_atomic_with_dir_sync(path, content, sync_directory)
}

fn sync_directory(directory: &Path) -> std::io::Result<()> {
    File::open(directory).and_then(|file| file.sync_all())
}

/// Atomically replaces `path` with `content`, treating the rename as the commit
/// point. `sync_directory` hardens the rename against a crash and is injectable
/// for tests. See [`write_private_atomic`].
fn write_private_atomic_with_dir_sync<F>(
    path: &Path,
    content: &[u8],
    sync_directory: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let directory = path
        .parent()
        .ok_or_else(|| "config path has no parent directory".to_string())?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "config path has no file name".to_string())?;
    let temporary = directory.join(format!(
        "{}.tmp.{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Everything up to and including the rename is the "pre-commit" phase: a
    // failure here leaves the target untouched, so the caller may safely treat
    // it as "not written" and roll back in-memory state.
    let pre_commit = (|| -> Result<(), String> {
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("failed to create temporary config: {error}"))?;
        file.write_all(content)
            .map_err(|error| format!("failed to write temporary config: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync temporary config: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("failed to replace config: {error}"))?;
        Ok(())
    })();
    if let Err(error) = pre_commit {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    // The rename has committed: the new content is already visible at `path`.
    // Syncing the directory only hardens that rename against a crash. If it
    // fails we must NOT report a failed write — doing so would roll back the
    // in-memory state and diverge it from what is durably on disk. Surface it
    // as a durability warning instead.
    if let Err(error) = sync_directory(directory) {
        tracing::warn!(
            path = %path.display(),
            "config replaced but directory sync failed; the change is written but may not survive a crash: {error}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_sync_failure_after_commit_keeps_written_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, b"old-content").unwrap();

        // The rename has already replaced the target when the directory sync
        // fails, so the write is committed: report success, not a failed write.
        let result = write_private_atomic_with_dir_sync(&path, b"new-content", |_| {
            Err(std::io::Error::other("simulated directory sync failure"))
        });

        assert!(result.is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), b"new-content");
        // The temporary file must not linger after a committed write.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.tmp.")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary file should be renamed away"
        );
    }

    #[test]
    fn pre_commit_failure_leaves_target_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A directory as the target makes the rename (pre-commit) fail, so the
        // existing target must be reported as unchanged.
        let path = tmp.path().join("occupied");
        std::fs::create_dir(&path).unwrap();

        let result = write_private_atomic_with_dir_sync(&path, b"new-content", |_| Ok(()));

        assert!(result.is_err());
        assert!(
            path.is_dir(),
            "target must be untouched on pre-commit failure"
        );
    }
}
