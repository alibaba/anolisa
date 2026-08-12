//! Shared-key authentication primitives for private Unix socket channels.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const AUTH_VERSION: &str = "1";
const AUTH_FRAME_LIMIT: usize = 4096;
const NONCE_LEN: usize = 32;
const MIN_SECRET_LEN: usize = 32;
const MAX_SECRET_LEN: usize = 4096;
const AUTH_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const CONTROL_CLIENT_DOMAIN: &str = "anolisa.skillfs.control.client.v1";
pub(crate) const CONTROL_SERVER_DOMAIN: &str = "anolisa.skillfs.control.server.v1";
pub(crate) const NOTIFY_CLIENT_DOMAIN: &str = "anolisa.skillfs.notify.client.v1";
pub(crate) const NOTIFY_SERVER_DOMAIN: &str = "anolisa.skillfs.notify.server.v1";

/// Raw shared secret loaded once at startup.
#[derive(Clone)]
pub(crate) struct SharedSecret(std::sync::Arc<[u8]>);

impl std::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedSecret([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuthError {
    #[error("authentication key file path must be absolute")]
    RelativePath,
    #[error("failed to open authentication key file: {0}")]
    Open(std::io::Error),
    #[error("authentication key must be a regular file")]
    NotRegular,
    #[error("authentication key file must not grant group or other permissions")]
    InsecurePermissions,
    #[error("authentication key file must be owned by the effective user")]
    WrongOwner,
    #[error("authentication key must contain between {MIN_SECRET_LEN} and {MAX_SECRET_LEN} bytes")]
    InvalidLength,
    #[error("authentication transport failed: {0}")]
    Io(std::io::Error),
    #[error("authentication handshake timed out")]
    Timeout,
    #[error("invalid authentication frame")]
    InvalidFrame,
    #[error("authentication proof verification failed")]
    VerificationFailed,
}

impl SharedSecret {
    /// Loads raw key bytes without following a final-component symlink.
    pub(crate) fn load(path: &Path) -> Result<Self, AuthError> {
        if !path.is_absolute() {
            return Err(AuthError::RelativePath);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(AuthError::Open)?;
        let metadata = file.metadata().map_err(AuthError::Open)?;
        validate_metadata(&metadata, unsafe { libc::geteuid() })?;

        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_SECRET_LEN + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(AuthError::Open)?;
        if !(MIN_SECRET_LEN..=MAX_SECRET_LEN).contains(&bytes.len()) {
            return Err(AuthError::InvalidLength);
        }
        Ok(Self(bytes.into()))
    }
}

fn validate_metadata(metadata: &std::fs::Metadata, expected_uid: u32) -> Result<(), AuthError> {
    if !metadata.is_file() {
        return Err(AuthError::NotRegular);
    }
    if metadata.uid() != expected_uid {
        return Err(AuthError::WrongOwner);
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(AuthError::InsecurePermissions);
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthFrame {
    auth_version: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<String>,
}

fn handshake_deadline(original_timeout: Option<Duration>) -> Instant {
    let timeout = original_timeout.map_or(AUTH_HANDSHAKE_TIMEOUT, |configured| {
        configured.min(AUTH_HANDSHAKE_TIMEOUT)
    });
    Instant::now() + timeout
}

fn restore_read_timeout(
    stream: &UnixStream,
    original_timeout: Option<Duration>,
    result: Result<(), AuthError>,
) -> Result<(), AuthError> {
    let restored = stream
        .set_read_timeout(original_timeout)
        .map_err(AuthError::Io);
    match result {
        Err(error) => Err(error),
        Ok(()) => restored,
    }
}

fn read_frame(stream: &mut UnixStream, deadline: Instant) -> Result<AuthFrame, AuthError> {
    let mut bytes = Vec::new();
    for _ in 0..=AUTH_FRAME_LIMIT {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(AuthError::Timeout)?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(AuthError::Io)?;
        let mut byte = [0_u8; 1];
        match stream.read(&mut byte) {
            Ok(0) => return Err(AuthError::InvalidFrame),
            Ok(_) if byte[0] == b'\n' => {
                return serde_json::from_slice(&bytes).map_err(|_| AuthError::InvalidFrame);
            }
            Ok(_) => bytes.push(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(AuthError::Timeout);
            }
            Err(error) => return Err(AuthError::Io(error)),
        }
    }
    Err(AuthError::InvalidFrame)
}

fn write_frame(stream: &mut UnixStream, frame: &AuthFrame) -> Result<(), AuthError> {
    serde_json::to_writer(&mut *stream, frame)
        .map_err(|e| AuthError::Io(std::io::Error::other(e)))?;
    stream.write_all(b"\n").map_err(AuthError::Io)?;
    stream.flush().map_err(AuthError::Io)
}

fn mac(secret: &SharedSecret, domain: &str, nonce: &[u8]) -> [u8; 32] {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&secret.0).expect("HMAC accepts keys of every valid length");
    mac.update(domain.as_bytes());
    mac.update(&[0]);
    mac.update(nonce);
    mac.finalize().into_bytes().into()
}

fn decode_32(value: &str) -> Result<[u8; 32], AuthError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| AuthError::InvalidFrame)?;
    if encode(&bytes) != value {
        return Err(AuthError::InvalidFrame);
    }
    bytes.try_into().map_err(|_| AuthError::InvalidFrame)
}

fn encode(value: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(value)
}

fn validate_frame(
    frame: &AuthFrame,
    kind: &str,
    has_nonce: bool,
    has_proof: bool,
) -> Result<(), AuthError> {
    if frame.auth_version != AUTH_VERSION
        || frame.kind != kind
        || frame.nonce.is_some() != has_nonce
        || frame.proof.is_some() != has_proof
    {
        return Err(AuthError::InvalidFrame);
    }
    Ok(())
}

/// Performs the server half of the four-frame challenge-response protocol.
pub(crate) fn authenticate_server(
    stream: &mut UnixStream,
    secret: &SharedSecret,
    client_domain: &str,
    server_domain: &str,
) -> Result<(), AuthError> {
    let original_timeout = stream.read_timeout().map_err(AuthError::Io)?;
    let deadline = handshake_deadline(original_timeout);
    let result = (|| {
        let init = read_frame(stream, deadline)?;
        validate_frame(&init, "auth.init", false, false)?;

        let mut nonce = [0_u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        write_frame(
            stream,
            &AuthFrame {
                auth_version: AUTH_VERSION.to_string(),
                kind: "auth.challenge".to_string(),
                nonce: Some(encode(&nonce)),
                proof: None,
            },
        )?;

        let proof = read_frame(stream, deadline)?;
        validate_frame(&proof, "auth.proof", false, true)?;
        let actual = decode_32(proof.proof.as_deref().ok_or(AuthError::InvalidFrame)?)?;
        let mut verifier = Hmac::<Sha256>::new_from_slice(&secret.0)
            .expect("HMAC accepts keys of every valid length");
        verifier.update(client_domain.as_bytes());
        verifier.update(&[0]);
        verifier.update(&nonce);
        verifier
            .verify_slice(&actual)
            .map_err(|_| AuthError::VerificationFailed)?;

        write_frame(
            stream,
            &AuthFrame {
                auth_version: AUTH_VERSION.to_string(),
                kind: "auth.ok".to_string(),
                nonce: None,
                proof: Some(encode(&mac(secret, server_domain, &nonce))),
            },
        )
    })();
    restore_read_timeout(stream, original_timeout, result)
}

/// Performs the client half and verifies the server knows the same secret.
pub(crate) fn authenticate_client(
    stream: &mut UnixStream,
    secret: &SharedSecret,
    client_domain: &str,
    server_domain: &str,
) -> Result<(), AuthError> {
    let original_timeout = stream.read_timeout().map_err(AuthError::Io)?;
    let deadline = handshake_deadline(original_timeout);
    let result = (|| {
        write_frame(
            stream,
            &AuthFrame {
                auth_version: AUTH_VERSION.to_string(),
                kind: "auth.init".to_string(),
                nonce: None,
                proof: None,
            },
        )?;
        let challenge = read_frame(stream, deadline)?;
        validate_frame(&challenge, "auth.challenge", true, false)?;
        let nonce = decode_32(challenge.nonce.as_deref().ok_or(AuthError::InvalidFrame)?)?;
        write_frame(
            stream,
            &AuthFrame {
                auth_version: AUTH_VERSION.to_string(),
                kind: "auth.proof".to_string(),
                nonce: None,
                proof: Some(encode(&mac(secret, client_domain, &nonce))),
            },
        )?;
        let ok = read_frame(stream, deadline)?;
        validate_frame(&ok, "auth.ok", false, true)?;
        let actual = decode_32(ok.proof.as_deref().ok_or(AuthError::InvalidFrame)?)?;
        let mut verifier = Hmac::<Sha256>::new_from_slice(&secret.0)
            .expect("HMAC accepts keys of every valid length");
        verifier.update(server_domain.as_bytes());
        verifier.update(&[0]);
        verifier.update(&nonce);
        verifier
            .verify_slice(&actual)
            .map_err(|_| AuthError::VerificationFailed)
    })();
    restore_read_timeout(stream, original_timeout, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn secret_loader_rejects_short_and_permissive_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        std::fs::write(&path, b"short").expect("write key");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(matches!(
            SharedSecret::load(&path),
            Err(AuthError::InvalidLength)
        ));

        std::fs::write(&path, [7_u8; 32]).expect("write key");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        assert!(matches!(
            SharedSecret::load(&path),
            Err(AuthError::InsecurePermissions)
        ));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let metadata = std::fs::metadata(&path).expect("metadata");
        assert!(matches!(
            validate_metadata(&metadata, metadata.uid().wrapping_add(1)),
            Err(AuthError::WrongOwner)
        ));
    }

    #[test]
    fn secret_loader_rejects_relative_path() {
        assert!(matches!(
            SharedSecret::load(Path::new("relative.key")),
            Err(AuthError::RelativePath)
        ));
    }

    #[test]
    fn secret_loader_rejects_symlink_directory_and_oversize() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("key");
        std::fs::write(&key, [1_u8; 32]).expect("write key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&key, &link).expect("symlink");
        assert!(matches!(SharedSecret::load(&link), Err(AuthError::Open(_))));
        assert!(matches!(
            SharedSecret::load(dir.path()),
            Err(AuthError::NotRegular)
        ));

        std::fs::write(&key, vec![2_u8; MAX_SECRET_LEN + 1]).expect("write oversized key");
        assert!(matches!(
            SharedSecret::load(&key),
            Err(AuthError::InvalidLength)
        ));
    }

    #[test]
    fn shared_cross_language_mac_vector_is_stable() {
        let secret = SharedSecret(std::sync::Arc::from(
            (0_u8..32).collect::<Vec<_>>().into_boxed_slice(),
        ));
        let nonce: Vec<u8> = (32_u8..64).collect();
        assert_eq!(
            encode(&mac(&secret, CONTROL_CLIENT_DOMAIN, &nonce)),
            "pqaSiunq07XWqMvQ8xSiSLi6dsLEy5iaCEF3md04AVI="
        );
        assert_eq!(
            encode(&mac(&secret, CONTROL_SERVER_DOMAIN, &nonce)),
            "naSgjgOT+Zs71EytW6byhJMCkfek2sGmK+CDqHmDsas="
        );
        assert_eq!(
            encode(&mac(&secret, NOTIFY_CLIENT_DOMAIN, &nonce)),
            "aFcVadTie7FrVTYOjk1OOjBpoQZ6LUvnLGC6stiqt6M="
        );
        assert_eq!(
            encode(&mac(&secret, NOTIFY_SERVER_DOMAIN, &nonce)),
            "F22J+ua0Pmha2dPyTMmTQNtKjcmed59Mo8FKgdcgBOc="
        );
    }

    #[test]
    fn client_and_server_complete_mutual_authentication() {
        let secret = SharedSecret(std::sync::Arc::from([9_u8; 32]));
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let configured_timeout = Duration::from_millis(250);
        client
            .set_read_timeout(Some(configured_timeout))
            .expect("set client timeout");
        server
            .set_read_timeout(Some(configured_timeout))
            .expect("set server timeout");
        let server_secret = secret.clone();
        let join = std::thread::spawn(move || {
            let result = authenticate_server(
                &mut server,
                &server_secret,
                CONTROL_CLIENT_DOMAIN,
                CONTROL_SERVER_DOMAIN,
            );
            (result, server.read_timeout().expect("server timeout"))
        });
        authenticate_client(
            &mut client,
            &secret,
            CONTROL_CLIENT_DOMAIN,
            CONTROL_SERVER_DOMAIN,
        )
        .expect("client auth");
        assert_eq!(
            client.read_timeout().expect("client timeout"),
            Some(configured_timeout)
        );
        let (server_result, server_timeout) = join.join().expect("server thread");
        server_result.expect("server auth");
        assert_eq!(server_timeout, Some(configured_timeout));
    }

    #[test]
    fn server_rejects_oversized_or_timed_out_auth_init() {
        let secret = SharedSecret(std::sync::Arc::from([9_u8; 32]));
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        client
            .write_all(&vec![b'x'; AUTH_FRAME_LIMIT + 1])
            .expect("write oversized frame");
        assert!(matches!(
            authenticate_server(
                &mut server,
                &secret,
                CONTROL_CLIENT_DOMAIN,
                CONTROL_SERVER_DOMAIN,
            ),
            Err(AuthError::InvalidFrame)
        ));

        let (_idle_client, mut timed_server) = UnixStream::pair().expect("socket pair");
        timed_server
            .set_read_timeout(Some(std::time::Duration::from_millis(10)))
            .expect("set timeout");
        assert!(matches!(
            authenticate_server(
                &mut timed_server,
                &secret,
                CONTROL_CLIENT_DOMAIN,
                CONTROL_SERVER_DOMAIN,
            ),
            Err(AuthError::Timeout)
        ));
    }

    #[test]
    fn server_applies_total_deadline_to_slow_auth_frame() {
        let secret = SharedSecret(std::sync::Arc::from([9_u8; 32]));
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        server
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("set timeout");
        let writer = std::thread::spawn(move || {
            for byte in br#"{\"authVersion\":\"1\""# {
                if client.write_all(&[*byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let started = Instant::now();
        assert!(matches!(
            authenticate_server(
                &mut server,
                &secret,
                CONTROL_CLIENT_DOMAIN,
                CONTROL_SERVER_DOMAIN,
            ),
            Err(AuthError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(server);
        writer.join().expect("writer thread");
    }
}
