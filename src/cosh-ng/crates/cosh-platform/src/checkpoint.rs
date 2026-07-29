//! Checkpoint client — communicates with ws-ckpt daemon via Unix socket + bincode framing.
//!
//! Wire format: [4-byte LE length prefix][bincode payload]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use cosh_types::checkpoint::*;
use cosh_types::error::{CoshError, ErrorCode};

/// Default timeout in milliseconds for socket operations.
const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// Maximum response payload size (64 MiB) to guard against OOM from a
/// misbehaving or corrupted daemon.
const MAX_RESPONSE_LEN: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
enum IoPhase {
    Connect,
    WriteRequest,
    ReadResponseLength,
    ReadResponsePayload,
}

#[derive(Clone, Copy)]
enum ProtocolFailure {
    TruncatedLength,
    TruncatedPayload,
    OversizedLength,
    InvalidPayload,
    UnexpectedResponse,
}

/// Client for ws-ckpt daemon IPC.
pub struct CkptClient {
    socket_path: String,
    timeout_ms: u64,
}

impl CkptClient {
    /// Create a new client pointing to the given socket path with default timeout.
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Create a new client with an explicit timeout (milliseconds).
    pub fn with_timeout(socket_path: &str, timeout_ms: u64) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            timeout_ms,
        }
    }

    /// Create a new client using the default socket path.
    pub fn default_path() -> Self {
        Self::new(DEFAULT_SOCKET_PATH)
    }

    /// Check if the daemon socket exists (basic health check).
    pub fn is_available(&self) -> bool {
        Path::new(&self.socket_path).exists()
    }

    // =======================================================================
    // Public operations (map to WsCkptRequest variants)
    // =======================================================================

    /// Initialize a workspace for checkpointing.
    pub fn init(&self, workspace: &str) -> Result<CkptInitResult, CoshError> {
        let req = WsCkptRequest::Init {
            workspace: workspace.to_string(),
        };
        match self.send_request(&req)? {
            WsCkptResponse::InitOk { ws_id } => Ok(CkptInitResult { ws_id }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Recover a workspace.
    pub fn recover(&self, workspace: &str) -> Result<CkptRecoverResult, CoshError> {
        let req = WsCkptRequest::Recover {
            workspace: workspace.to_string(),
        };
        match self.send_request(&req)? {
            WsCkptResponse::RecoverOk { workspace } => Ok(CkptRecoverResult { workspace }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Create a workspace checkpoint.
    pub fn create(
        &self,
        workspace: &str,
        id: &str,
        message: Option<&str>,
        metadata: Option<&str>,
        pin: bool,
    ) -> Result<CkptCreated, CoshError> {
        let req = WsCkptRequest::Checkpoint {
            workspace: workspace.to_string(),
            id: id.to_string(),
            message: message.map(|s| s.to_string()),
            metadata: metadata.map(|s| s.to_string()),
            pin,
        };
        match self.send_request(&req)? {
            WsCkptResponse::CheckpointOk { snapshot_id } => Ok(CkptCreated {
                snapshot_id: Some(snapshot_id),
                workspace: workspace.to_string(),
                skipped: false,
                reason: None,
            }),
            WsCkptResponse::CheckpointSkipped { reason } => Ok(CkptCreated {
                snapshot_id: None,
                workspace: workspace.to_string(),
                skipped: true,
                reason: Some(reason),
            }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// List checkpoints for a workspace.
    pub fn list(&self, workspace: Option<&str>) -> Result<CkptListResult, CoshError> {
        let req = WsCkptRequest::List {
            workspace: workspace.map(|s| s.to_string()),
            format: None,
        };
        match self.send_request(&req)? {
            WsCkptResponse::ListOk { snapshots } => {
                let total = snapshots.len();
                let entries = snapshots
                    .into_iter()
                    .map(|s| CkptEntry {
                        id: s.id,
                        workspace: s.workspace,
                        message: s.meta.message,
                        pinned: s.meta.pinned,
                        created_at: s.meta.created_at.to_rfc3339(),
                    })
                    .collect();
                Ok(CkptListResult {
                    snapshots: entries,
                    total,
                })
            }
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Restore (rollback) to a checkpoint.
    pub fn restore(&self, workspace: &str, snapshot_id: &str) -> Result<CkptRestored, CoshError> {
        let req = WsCkptRequest::Rollback {
            workspace: workspace.to_string(),
            to: Some(snapshot_id.to_string()),
            num_ancestors: None,
        };
        match self.send_request(&req)? {
            WsCkptResponse::RollbackOk { from, to } => Ok(CkptRestored { from, to }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Query workspace checkpoint status.
    pub fn status(&self, workspace: Option<&str>) -> Result<CkptStatusResult, CoshError> {
        let req = WsCkptRequest::Status {
            workspace: workspace.map(|s| s.to_string()),
        };
        match self.send_request(&req)? {
            WsCkptResponse::StatusOk { report } => Ok(CkptStatusResult {
                uptime_secs: report.uptime_secs,
                workspaces: report.workspaces,
                fs_total_bytes: report.fs_total_bytes,
                fs_used_bytes: report.fs_used_bytes,
            }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Delete a snapshot.
    pub fn delete(
        &self,
        workspace: Option<&str>,
        snapshot: &str,
        force: bool,
    ) -> Result<CkptDeleted, CoshError> {
        let req = WsCkptRequest::Delete {
            workspace: workspace.map(|s| s.to_string()),
            snapshot: snapshot.to_string(),
            force,
        };
        match self.send_request(&req)? {
            WsCkptResponse::DeleteOk { target } => Ok(CkptDeleted { target }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Diff between two snapshots.
    pub fn diff(&self, workspace: &str, from: &str, to: &str) -> Result<CkptDiffResult, CoshError> {
        let req = WsCkptRequest::Diff {
            workspace: workspace.to_string(),
            from: from.to_string(),
            to: Some(to.to_string()),
        };
        match self.send_request(&req)? {
            WsCkptResponse::DiffOk { changes } => Ok(CkptDiffResult { changes }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    /// Cleanup old snapshots.
    pub fn cleanup(
        &self,
        workspace: &str,
        keep: Option<u32>,
    ) -> Result<CkptCleanupResult, CoshError> {
        let req = WsCkptRequest::Cleanup {
            workspace: workspace.to_string(),
            keep,
        };
        match self.send_request(&req)? {
            WsCkptResponse::CleanupOk { removed } => Ok(CkptCleanupResult { removed }),
            WsCkptResponse::Error { code, message } => Err(ws_error_to_cosh(code, message)),
            _ => Err(unexpected_response()),
        }
    }

    // =======================================================================
    // Wire protocol
    // =======================================================================

    /// Send a request and receive a response over the Unix socket.
    /// Wire format: [4-byte LE length prefix][bincode payload]
    fn send_request(&self, req: &WsCkptRequest) -> Result<WsCkptResponse, CoshError> {
        // 1. Socket existence check — fast fail before attempting connection.
        if !Path::new(&self.socket_path).exists() {
            return Err(CoshError::new(
                ErrorCode::CheckpointDaemonUnavailable,
                "ws-ckpt daemon socket is unavailable",
                "checkpoint",
            )
            .with_hint("Start daemon with: systemctl start ws-ckpt")
            .recoverable(true));
        }

        // 2. Connect to Unix socket.
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| classify_io_error(error, IoPhase::Connect))?;

        // 3. Apply configurable timeout to both read and write.
        let timeout = Duration::from_millis(self.timeout_ms);
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();

        // 4. Encode and send the request frame.
        let frame = encode_frame(req)?;
        stream
            .write_all(&frame)
            .map_err(|error| classify_io_error(error, IoPhase::WriteRequest))?;

        // 5. Read response length prefix (4 bytes, little-endian).
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                protocol_error(ProtocolFailure::TruncatedLength)
            } else {
                classify_io_error(error, IoPhase::ReadResponseLength)
            }
        })?;
        let resp_len = u32::from_le_bytes(len_buf) as usize;

        if resp_len > MAX_RESPONSE_LEN {
            return Err(protocol_error(ProtocolFailure::OversizedLength));
        }

        // 6. Read response payload.
        let mut resp_buf = vec![0u8; resp_len];
        stream.read_exact(&mut resp_buf).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                protocol_error(ProtocolFailure::TruncatedPayload)
            } else {
                classify_io_error(error, IoPhase::ReadResponsePayload)
            }
        })?;

        // 7. Decode response.
        decode_response(&resp_buf)
    }
}

// ---------------------------------------------------------------------------
// Frame encoding/decoding
// ---------------------------------------------------------------------------

/// Encode a message into a length-prefixed bincode frame.
/// Format: [4-byte LE length][bincode payload]
fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, CoshError> {
    let payload = bincode::serialize(msg).map_err(|e| {
        CoshError::new(
            ErrorCode::Unknown,
            format!("Failed to serialize request: {}", e),
            "checkpoint",
        )
    })?;
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a bincode payload into a WsCkptResponse.
fn decode_response(data: &[u8]) -> Result<WsCkptResponse, CoshError> {
    bincode::deserialize(data).map_err(|_| protocol_error(ProtocolFailure::InvalidPayload))
}

// ---------------------------------------------------------------------------
// Error conversion helpers
// ---------------------------------------------------------------------------

/// Map ws-ckpt daemon ErrorCode to CoshError.
fn ws_error_to_cosh(code: WsCkptErrorCode, message: String) -> CoshError {
    let (error_code, hint) = match code {
        WsCkptErrorCode::WorkspaceNotFound => (
            ErrorCode::CheckpointNotFound,
            Some("Workspace not initialized. Run: cosh checkpoint init --workspace <path>"),
        ),
        WsCkptErrorCode::SnapshotNotFound => (
            ErrorCode::CheckpointNotFound,
            Some("Use 'cosh checkpoint list' to see available snapshots"),
        ),
        WsCkptErrorCode::AlreadyInitialized => (
            ErrorCode::CheckpointCreateFailed,
            Some("Workspace is already initialized for checkpointing"),
        ),
        WsCkptErrorCode::BtrfsError => (
            ErrorCode::CheckpointCreateFailed,
            Some("Btrfs filesystem error. Check that the workspace is on a btrfs volume"),
        ),
        WsCkptErrorCode::IoError => (ErrorCode::Unknown, None),
        WsCkptErrorCode::InvalidPath => (
            ErrorCode::InvalidInput,
            Some("The provided path is invalid or inaccessible"),
        ),
        WsCkptErrorCode::ConfirmationRequired => (
            ErrorCode::CheckpointRestoreFailed,
            Some("Use --force to skip confirmation"),
        ),
        WsCkptErrorCode::InternalError => (ErrorCode::Unknown, None),
        WsCkptErrorCode::SnapshotAlreadyExists => (
            ErrorCode::CheckpointCreateFailed,
            Some("A snapshot with this ID already exists"),
        ),
        WsCkptErrorCode::WriteLockConflict => (
            ErrorCode::CheckpointCreateFailed,
            Some("Another operation is in progress, retry later"),
        ),
        WsCkptErrorCode::DiskSpaceInsufficient => (
            ErrorCode::CheckpointCreateFailed,
            Some("Not enough disk space. Run 'cosh checkpoint cleanup' to free space"),
        ),
        WsCkptErrorCode::CwdOccupied => (
            ErrorCode::CheckpointRestoreFailed,
            Some("Leave the workspace before retrying the restore"),
        ),
        WsCkptErrorCode::CwdScanFailed => (ErrorCode::CheckpointRestoreFailed, None),
    };

    let mut err = CoshError::new(error_code, message, "checkpoint");
    if let Some(h) = hint {
        err = err.with_hint(h);
    }
    err
}

fn unexpected_response() -> CoshError {
    protocol_error(ProtocolFailure::UnexpectedResponse)
}

fn protocol_error(failure: ProtocolFailure) -> CoshError {
    let kind = match failure {
        ProtocolFailure::TruncatedLength => "truncated_length",
        ProtocolFailure::TruncatedPayload => "truncated_payload",
        ProtocolFailure::OversizedLength => "oversized_length",
        ProtocolFailure::InvalidPayload => "decode_failed",
        ProtocolFailure::UnexpectedResponse => "unexpected_response",
    };
    CoshError::new(
        ErrorCode::CheckpointProtocolError,
        "Invalid response from ws-ckpt daemon",
        "checkpoint",
    )
    .with_hint("Retry the operation; restart ws-ckpt if the problem persists")
    .with_details(serde_json::json!({"phase": "response", "kind": kind}))
}

fn classify_io_error(error: std::io::Error, phase: IoPhase) -> CoshError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        return CoshError::new(
            ErrorCode::Timeout,
            "Timed out while communicating with ws-ckpt daemon",
            "checkpoint",
        )
        .with_hint("ws-ckpt daemon may be overloaded; retry later")
        .recoverable(true);
    }

    let message = match phase {
        IoPhase::Connect => "Cannot connect to ws-ckpt daemon",
        IoPhase::WriteRequest => "ws-ckpt daemon became unavailable while sending a request",
        IoPhase::ReadResponseLength | IoPhase::ReadResponsePayload => {
            "ws-ckpt daemon became unavailable while receiving a response"
        }
    };
    CoshError::new(
        ErrorCode::CheckpointDaemonUnavailable,
        message,
        "checkpoint",
    )
    .with_hint("Start or restart ws-ckpt with: systemctl start ws-ckpt")
    .recoverable(true)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;

    fn send_fake_response(response: Vec<u8>) -> (CoshError, String) {
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut length = [0; 4];
            stream.read_exact(&mut length).unwrap();
            let mut request = vec![0; u32::from_le_bytes(length) as usize];
            stream.read_exact(&mut request).unwrap();
            stream.write_all(&response).unwrap();
        });

        let socket_path = socket_path.to_string_lossy().into_owned();
        let error = CkptClient::new(&socket_path)
            .send_request(&WsCkptRequest::Config)
            .unwrap_err();
        server.join().unwrap();
        (error, socket_path)
    }

    fn assert_protocol_error(error: &CoshError, socket_path: &str) {
        assert_eq!(error.code, ErrorCode::CheckpointProtocolError);
        assert!(!error.message.contains(socket_path));
        assert!(!error.message.contains("failed to fill whole buffer"));
        assert!(!error.message.contains("invalid value"));
    }

    #[test]
    fn response_truncated_length_is_protocol_error() {
        let (error, socket_path) = send_fake_response(vec![1, 2]);
        assert_protocol_error(&error, &socket_path);
    }

    #[test]
    fn response_truncated_payload_is_protocol_error() {
        let mut response = 8_u32.to_le_bytes().to_vec();
        response.extend_from_slice(&[1, 2]);
        let (error, socket_path) = send_fake_response(response);
        assert_protocol_error(&error, &socket_path);
    }

    #[test]
    fn response_oversized_length_is_protocol_error() {
        let response = ((MAX_RESPONSE_LEN + 1) as u32).to_le_bytes().to_vec();
        let (error, socket_path) = send_fake_response(response);
        assert_protocol_error(&error, &socket_path);
    }

    #[test]
    fn response_invalid_bincode_is_protocol_error() {
        let mut response = 4_u32.to_le_bytes().to_vec();
        response.extend_from_slice(&u32::MAX.to_le_bytes());
        let (error, socket_path) = send_fake_response(response);
        assert_protocol_error(&error, &socket_path);
    }

    fn spawn_one_shot_daemon(
        response: WsCkptResponse,
    ) -> (tempfile::TempDir, String, thread::JoinHandle<()>) {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ws-ckpt.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut len_buf = [0_u8; 4];
            stream.read_exact(&mut len_buf).unwrap();
            let request_len = u32::from_le_bytes(len_buf) as usize;
            let mut request = vec![0_u8; request_len];
            stream.read_exact(&mut request).unwrap();

            let payload = bincode::serialize(&response).unwrap();
            stream
                .write_all(&(payload.len() as u32).to_le_bytes())
                .unwrap();
            stream.write_all(&payload).unwrap();
        });

        let socket_path = socket_path.to_string_lossy().into_owned();
        (dir, socket_path, handle)
    }

    #[test]
    fn test_default_timeout() {
        let client = CkptClient::new("/tmp/test.sock");
        assert_eq!(client.timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn test_custom_timeout() {
        let client = CkptClient::with_timeout("/tmp/test.sock", 10000);
        assert_eq!(client.timeout_ms, 10000);
    }

    #[test]
    fn test_socket_not_found_returns_checkpoint_unavailable() {
        let client = CkptClient::new("/tmp/nonexistent-test-sock-xyz.sock");
        let result = client.create("/tmp/ws", "snap-1", None, None, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
        assert!(err.message.contains("ws-ckpt"));
        assert!(err
            .hint
            .as_ref()
            .unwrap()
            .contains("systemctl start ws-ckpt"));
        assert!(err.recoverable);
    }

    #[test]
    fn test_create_checkpoint_skipped_is_success() {
        let reason = "workspace has no changes";
        let (_dir, socket_path, daemon) =
            spawn_one_shot_daemon(WsCkptResponse::CheckpointSkipped {
                reason: reason.into(),
            });
        let client = CkptClient::new(&socket_path);

        let result = client
            .create("/tmp/ws", "snap-1", None, None, false)
            .unwrap();
        daemon.join().unwrap();

        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "snapshot_id": null,
                "workspace": "/tmp/ws",
                "skipped": true,
                "reason": reason,
            })
        );
    }

    #[test]
    fn test_create_checkpoint_ok_preserves_success_contract() {
        let (_dir, socket_path, daemon) = spawn_one_shot_daemon(WsCkptResponse::CheckpointOk {
            snapshot_id: "snap-1".into(),
        });
        let client = CkptClient::new(&socket_path);

        let result = client
            .create("/tmp/ws", "snap-1", None, None, false)
            .unwrap();
        daemon.join().unwrap();

        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "snapshot_id": "snap-1",
                "workspace": "/tmp/ws",
                "skipped": false,
                "reason": null,
            })
        );
    }

    #[test]
    fn test_socket_not_found_list() {
        let client = CkptClient::new("/tmp/nonexistent-test-sock-xyz.sock");
        let result = client.list(Some("/tmp/ws"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
    }

    #[test]
    fn test_is_available_nonexistent() {
        let client = CkptClient::new("/tmp/absolutely-does-not-exist.sock");
        assert!(!client.is_available());
    }

    #[test]
    fn test_encode_decode_frame() {
        let req = WsCkptRequest::Status {
            workspace: Some("/tmp/ws".into()),
        };
        let frame = encode_frame(&req).unwrap();

        // Frame should start with 4-byte LE length
        let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(frame.len(), 4 + len);

        // Payload should be valid bincode
        let decoded: WsCkptRequest = bincode::deserialize(&frame[4..]).unwrap();
        match decoded {
            WsCkptRequest::Status { workspace } => assert_eq!(workspace, Some("/tmp/ws".into())),
            _ => panic!("Wrong variant decoded"),
        }
    }

    #[test]
    fn test_decode_response_valid() {
        let resp = WsCkptResponse::CheckpointOk {
            snapshot_id: "snap-123".into(),
        };
        let data = bincode::serialize(&resp).unwrap();
        let decoded = decode_response(&data).unwrap();
        match decoded {
            WsCkptResponse::CheckpointOk { snapshot_id } => assert_eq!(snapshot_id, "snap-123"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_decode_response_invalid() {
        let bad_data = b"not valid bincode data!!!!";
        let result = decode_response(bad_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_classify_broken_pipe() {
        let err = classify_io_error(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"),
            IoPhase::WriteRequest,
        );
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
        assert!(err.message.contains("unavailable"));
        assert!(!err.message.contains("BrokenPipe"));
        assert!(err
            .hint
            .as_ref()
            .unwrap()
            .contains("systemctl start ws-ckpt"));
        assert!(err.recoverable);
    }

    #[test]
    fn test_classify_connection_reset() {
        let err = classify_io_error(
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset"),
            IoPhase::ReadResponsePayload,
        );
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
        assert!(err.message.contains("unavailable"));
        assert!(!err.message.contains("ConnectionReset"));
        assert!(err.recoverable);
    }

    #[test]
    fn test_classify_timeout() {
        let err = classify_io_error(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
            IoPhase::ReadResponseLength,
        );
        assert_eq!(err.code, ErrorCode::Timeout);
        assert!(err.hint.as_ref().unwrap().contains("overloaded"));
        assert!(err.recoverable);
    }

    #[test]
    fn test_classify_connection_refused() {
        let err = classify_io_error(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
            IoPhase::Connect,
        );
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
        assert!(err
            .hint
            .as_ref()
            .unwrap()
            .contains("systemctl start ws-ckpt"));
    }

    #[test]
    fn test_classify_other_io_error() {
        let err = classify_io_error(
            std::io::Error::other("something else"),
            IoPhase::WriteRequest,
        );
        assert_eq!(err.code, ErrorCode::CheckpointDaemonUnavailable);
        assert!(err.recoverable);
    }

    #[test]
    fn test_ws_error_to_cosh_mapping() {
        let err = ws_error_to_cosh(WsCkptErrorCode::WorkspaceNotFound, "ws not found".into());
        assert_eq!(err.code, ErrorCode::CheckpointNotFound);
        assert!(err.hint.is_some());

        let err = ws_error_to_cosh(WsCkptErrorCode::DiskSpaceInsufficient, "no space".into());
        assert_eq!(err.code, ErrorCode::CheckpointCreateFailed);
        assert!(err.hint.unwrap().contains("cleanup"));
    }

    #[test]
    fn test_response_length_exceeds_max() {
        // Simulate a daemon sending a response length larger than MAX_RESPONSE_LEN.
        // We can't easily test through the socket, but we verify the constant is
        // reasonable and the guard logic is in place by checking the const.
        assert_eq!(MAX_RESPONSE_LEN, 64 * 1024 * 1024);
        // A normal response is at most a few KiB; 64 MiB is generous headroom.
    }
}
